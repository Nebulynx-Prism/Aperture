/* Standalone shell diagnostic, not part of the APK. No desktop bytes are logged
 * or saved. Build with tools/run_stream_probe.ps1; run with app_process.
 * Args: host [seconds=10] [port=38810] [usb|wifi] [sample=false].
 * Exit 0: hardware output reached SurfaceTexture; 2: no frames/error; 3: watchdog.
 * This tests network + MediaCodec + offscreen GL, not headset/OpenXR rendering.
 */
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.media.MediaFormat;
import android.opengl.EGL14;
import android.opengl.EGLConfig;
import android.opengl.EGLContext;
import android.opengl.EGLDisplay;
import android.opengl.EGLSurface;
import android.opengl.GLES11Ext;
import android.opengl.GLES20;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.Surface;
import java.io.*;
import java.net.*;
import java.nio.*;
import java.util.*;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.atomic.*;

public final class StreamProbe {
    static final int MAX_MESSAGE = 16 * 1024 * 1024, MAX_AU = 8 * 1024 * 1024;
    static final long SECOND = 1_000_000_000L;
    static void require(boolean ok, String why) throws IOException {
        if (!ok) throw new IOException(why);
    }
    static int u16(byte[] b, int p) { return ((b[p] & 255) << 8) | (b[p + 1] & 255); }
    static long u32(byte[] b, int p) {
        return ((long) u16(b, p) << 16) | u16(b, p + 2);
    }
    static final class Message {
        final int type; final byte[] payload; final long sequence;
        Message(int t, byte[] p, long s) { type = t; payload = p; sequence = s; }
    }
    static final class Cursor {
        final byte[] b; int p;
        Cursor(byte[] bytes) { b = bytes; }
        long varint() throws IOException {
            long v = 0;
            for (int i = 0; i < 10; i++) {
                require(p < b.length, "truncated protobuf varint");
                int x = b[p++] & 255;
                require(i != 9 || (x & 254) == 0, "protobuf varint overflow");
                v |= (long) (x & 127) << (i * 7);
                if ((x & 128) == 0) return v;
            }
            throw new IOException("protobuf varint too long");
        }
        int length() throws IOException {
            long n = varint();
            require(n >= 0 && n <= b.length - p, "invalid protobuf length");
            return (int) n;
        }
    }
    static Message parse(byte[] bytes) throws IOException {
        require(bytes.length > 0 && bytes.length <= MAX_MESSAGE, "invalid message size");
        Cursor c = new Cursor(bytes);
        int type = 0; long seq = 0; byte[] payload = new byte[0];
        boolean haveType = false, havePayload = false, haveSeq = false;
        while (c.p < bytes.length) {
            long key = c.varint();
            require(key > 0 && key <= 0xffffffffL && (key >>> 3) != 0, "invalid protobuf tag");
            int field = (int) (key >>> 3), wire = (int) (key & 7);
            if (field == 1) {
                require(wire == 0 && !haveType, "invalid/duplicate message type");
                long value = c.varint();
                require(value >= 0 && value <= Integer.MAX_VALUE, "invalid message type");
                type = (int) value; haveType = true;
            } else if (field == 2) {
                require(wire == 2 && !havePayload, "invalid/duplicate payload");
                int n = c.length(); require(n <= MAX_AU, "payload exceeds AU limit");
                payload = Arrays.copyOfRange(bytes, c.p, c.p + n); c.p += n; havePayload = true;
            } else if (field == 4) {
                require(wire == 0 && !haveSeq, "invalid/duplicate sequence");
                seq = c.varint(); require(seq >= 0 && seq <= 0xffffffffL, "sequence overflow");
                haveSeq = true;
            } else {
                switch (wire) {
                    case 0: c.varint(); break;
                    case 1: require(bytes.length - c.p >= 8, "truncated fixed64"); c.p += 8; break;
                    case 2: { int n = c.length(); c.p += n; break; }
                    case 5: require(bytes.length - c.p >= 4, "truncated fixed32"); c.p += 4; break;
                    default: throw new IOException("unsupported protobuf wire type");
                }
            }
        }
        return new Message(type, payload, seq);
    }
    static void varint(ByteArrayOutputStream out, long n) {
        do { int x = (int) (n & 127); n >>>= 7; out.write(x | (n == 0 ? 0 : 128)); } while (n != 0);
    }
    static byte[] encode(int type, byte[] payload, long sequence) {
        ByteArrayOutputStream b = new ByteArrayOutputStream();
        varint(b, 8); varint(b, type); varint(b, 18); varint(b, payload.length);
        b.write(payload, 0, payload.length); varint(b, 32); varint(b, sequence);
        return b.toByteArray();
    }
    static int[] config(byte[] p) throws IOException {
        require(p.length == 12, "config102 must contain exactly three u32 values");
        long codec = u32(p, 0), w = u32(p, 4), h = u32(p, 8);
        require(codec <= 1, "unsupported codec (only H264/HEVC)");
        require(w >= 2 && h >= 2 && w <= 8192 && h <= 8192 && w * h <= 33554432
                && (w & 1) == 0 && (h & 1) == 0, "invalid configured dimensions");
        return new int[]{(int) codec, (int) w, (int) h};
    }
    static boolean keyframe(byte[] bytes, int codec) {
        for (int i = 0; i + 3 < bytes.length; i++) {
            if (bytes[i] != 0 || bytes[i + 1] != 0) continue;
            int h = bytes[i + 2] == 1 ? i + 3
                : (i + 4 < bytes.length && bytes[i + 2] == 0 && bytes[i + 3] == 1 ? i + 4 : -1);
            if (h < 0) continue;
            int type = codec == 0 ? bytes[h] & 31 : (bytes[h] >> 1) & 63;
            if (codec == 0 ? type == 5 : type >= 16 && type <= 21) return true;
        }
        return false;
    }
    static boolean newer(long a, long b) { long d = (a - b) & 0xffffffffL; return d > 0 && d < 0x80000000L; }
    static final class Slot {
        final byte[][] fragments; final long since; int count, bytes;
        Slot(int n, long now) { fragments = new byte[n][]; since = now; }
    }
    static final class Assembly {
        final LinkedHashMap<Long, Slot> slots = new LinkedHashMap<>();
        long last = -1, expired, duplicates, invalid, gaps; boolean recovery;
        void expire(long now) {
            Iterator<Slot> it = slots.values().iterator();
            while (it.hasNext()) if (now - it.next().since > SECOND / 5) {
                it.remove(); expired++; recovery = true;
            }
        }
        Message accept(byte[] p, int n, long now) throws IOException {
            expire(now);
            if (n < 14 || n > 1200 || p[0] != 'O' || p[1] != 'R' || p[2] != 'V' || p[3] != 'D') {
                invalid++; return null;
            }
            if (p[4] != 1) return null; // Audio has its own sequence space.
            int index = u16(p, 5), total = u16(p, 7); long seq = u32(p, 9);
            if (total == 0 || total > 8192 || index >= total) { invalid++; return null; }
            if (last >= 0 && !newer(seq, last)) return null;
            Slot slot = slots.get(seq);
            if (slot == null) {
                if (slots.size() == 4) { slots.clear(); recovery = true; gaps++; }
                slot = new Slot(total, now); slots.put(seq, slot);
            }
            if (slot.fragments.length != total) {
                slots.remove(seq); invalid++; recovery = true; return null;
            }
            if (slot.fragments[index] != null) {
                duplicates++;
                if (!Arrays.equals(slot.fragments[index], Arrays.copyOfRange(p, 13, n))) {
                    slots.remove(seq); invalid++; recovery = true;
                }
                return null;
            }
            require(slot.bytes + n - 13 <= MAX_AU, "UDP access unit exceeds size limit");
            slot.fragments[index] = Arrays.copyOfRange(p, 13, n); slot.bytes += n - 13; slot.count++;
            if (slot.count != total) return null;
            if (last >= 0 && seq != ((last + 1) & 0xffffffffL)) { gaps++; recovery = true; }
            byte[] au = new byte[slot.bytes]; int pos = 0;
            for (byte[] fragment : slot.fragments) { System.arraycopy(fragment, 0, au, pos, fragment.length); pos += fragment.length; }
            last = seq;
            Iterator<Long> it = slots.keySet().iterator();
            while (it.hasNext()) if (!newer(it.next(), seq)) it.remove();
            return new Message(101, au, seq);
        }
    }

    final ArrayBlockingQueue<Message> queue = new ArrayBlockingQueue<>(8);
    final AtomicReference<Throwable> failure = new AtomicReference<>();
    final AtomicBoolean stopped = new AtomicBoolean(), configured = new AtomicBoolean(), recover = new AtomicBoolean();
    final AtomicLong tcpMessages = new AtomicLong(), tcpVideo = new AtomicLong(), datagrams = new AtomicLong(), udpVideo = new AtomicLong();
    final Assembly assembly = new Assembly();
    Socket tcp; DatagramSocket udp; DataOutputStream output;
    Thread tcpThread, udpThread; long deadline, sendSequence, requests, lastRequest;
    long submitted, decoded, skipped, surfaceFrames, nonblackSamples, sampledPixels, sampledFrames;
    int configuredWidth, configuredHeight, outputWidth, outputHeight;
    String decoderName = "none", mime = "none"; boolean wifi, sample;

    void offer(Message m) throws IOException { require(queue.offer(m), "decode queue overflow; stopping instead of dropping reference frames"); }
    synchronized void send(int type, byte[] data) throws IOException {
        byte[] p = encode(type, data, sendSequence++);
        output.writeInt(p.length); output.write(p); output.flush();
    }
    void requestKeyframe() throws IOException {
        long now = System.nanoTime();
        if (now - lastRequest >= SECOND / 2) { send(14, new byte[0]); lastRequest = now; requests++; }
    }
    byte[] readExact(int n) throws IOException {
        byte[] b = new byte[n]; int pos = 0; long progress = System.nanoTime();
        while (pos < n) {
            require(!stopped.get() && System.nanoTime() < deadline, "TCP read deadline");
            try {
                int r = tcp.getInputStream().read(b, pos, n - pos);
                if (r < 0) throw new EOFException("server closed TCP");
                if (r > 0) { pos += r; progress = System.nanoTime(); }
            } catch (SocketTimeoutException timeout) {
                require(pos == 0 || System.nanoTime() - progress < 2 * SECOND, "stalled partial TCP frame");
            }
        }
        return b;
    }
    void connect(String host, int port) throws IOException {
        tcp = new Socket(); tcp.setTcpNoDelay(true); tcp.setSoTimeout(100);
        tcp.connect(new InetSocketAddress(host, port), 2000);
        udp = new DatagramSocket(); udp.setSoTimeout(100); udp.setReceiveBufferSize(4 * 1024 * 1024);
        int serverUdp = u16(readExact(2), 0); require(serverUdp != 0, "zero server UDP port");
        udp.connect(tcp.getInetAddress(), serverUdp);
        output = new DataOutputStream(tcp.getOutputStream());
        output.writeShort(udp.getLocalPort()); output.flush();
        tcpThread = new Thread(() -> {
            try {
                while (!stopped.get()) {
                    long length = u32(readExact(4), 0);
                    require(length > 0 && length <= MAX_MESSAGE, "TCP frame length out of bounds");
                    Message m = parse(readExact((int) length)); tcpMessages.incrementAndGet();
                    if (m.type == 102) { config(m.payload); offer(m); }
                    else if (m.type == 101) {
                        require(!wifi && configured.get(), "unexpected/unconfigured TCP video");
                        require(m.payload.length > 0, "empty TCP video"); tcpVideo.incrementAndGet(); offer(m);
                    } else if (m.type == 104) throw new IOException("server reported stream failure (104)");
                }
            } catch (Throwable e) { if (!stopped.get() && System.nanoTime() < deadline) failure.compareAndSet(null, e); }
        }, "probe-tcp");
        tcpThread.setDaemon(true); tcpThread.start();
        if (wifi) {
            udpThread = new Thread(() -> {
                byte[] b = new byte[1201]; DatagramPacket packet = new DatagramPacket(b, b.length);
                try {
                    while (!stopped.get() && System.nanoTime() < deadline) {
                        Message frame = null;
                        try {
                            packet.setLength(b.length); udp.receive(packet); datagrams.incrementAndGet();
                            if (configured.get()) frame = assembly.accept(b, packet.getLength(), System.nanoTime());
                        } catch (SocketTimeoutException timeout) { assembly.expire(System.nanoTime()); }
                        if (assembly.recovery) { recover.set(true); assembly.recovery = false; }
                        if (frame != null) { udpVideo.incrementAndGet(); offer(frame); }
                    }
                } catch (Throwable e) { if (!stopped.get()) failure.compareAndSet(null, e); }
            }, "probe-udp");
            udpThread.setDaemon(true); udpThread.start();
        }
    }
    int run(String host, int seconds, int port) throws Exception {
        deadline = System.nanoTime() + seconds * SECOND;
        connect(host, port);
        Sink sink = null; MediaCodec codec = null; boolean started = false, waitingKey = true, awaitingSurface = false;
        Message pending = null; long lastTextureTimestamp = Long.MIN_VALUE;
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        try {
            while (System.nanoTime() < deadline) {
                Throwable error = failure.get(); if (error != null) throw new IOException("network reader failed", error);
                if (recover.getAndSet(false)) { waitingKey = true; pending = null; }
                if (pending == null) pending = queue.poll();
                if (pending != null && pending.type == 102) {
                    require(codec == null, "repeated config102 is unsupported by this bounded probe");
                    int[] cfg = config(pending.payload);
                    configuredWidth = cfg[1]; configuredHeight = cfg[2];
                    mime = cfg[0] == 0 ? "video/avc" : "video/hevc";
                    MediaFormat format = MediaFormat.createVideoFormat(mime, cfg[1], cfg[2]);
                    for (MediaCodecInfo candidate : new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos()) {
                        if (candidate.isEncoder() || !candidate.isHardwareAccelerated() || candidate.isAlias()) continue;
                        for (String type : candidate.getSupportedTypes()) if (type.equalsIgnoreCase(mime)
                                && candidate.getCapabilitiesForType(type).isFormatSupported(format)) {
                            decoderName = candidate.getName(); break;
                        }
                        if (!decoderName.equals("none")) break;
                    }
                    require(!decoderName.equals("none"), "no compatible hardware decoder");
                    sink = new Sink(); codec = MediaCodec.createByCodecName(decoderName);
                    format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, MAX_AU);
                    codec.configure(format, sink.surface, null, 0); codec.start(); started = true;
                    configured.set(true); send(103, new byte[]{1}); pending = null;
                    System.out.println("CONFIG codec=" + mime + " width=" + cfg[1] + " height=" + cfg[2]
                            + " decoder=" + decoderName + " hardware=true ack103=1");
                }
                if (codec == null) { Thread.sleep(2); continue; }
                if (sink.available.getAndSet(false)) {
                    sink.texture.updateTexImage(); long stamp = sink.texture.getTimestamp();
                    if (stamp != lastTextureTimestamp) {
                        lastTextureTimestamp = stamp; surfaceFrames++;
                        if (sample) { nonblackSamples += sink.sample(); sampledPixels += Sink.SIDE * Sink.SIDE; sampledFrames++; }
                    }
                    awaitingSurface = false;
                }
                if (!awaitingSurface) {
                    int index = codec.dequeueOutputBuffer(info, 0);
                    if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                        MediaFormat f = codec.getOutputFormat();
                        outputWidth = f.getInteger(MediaFormat.KEY_WIDTH); outputHeight = f.getInteger(MediaFormat.KEY_HEIGHT);
                        System.out.println("OUTPUT width=" + outputWidth + " height=" + outputHeight);
                    } else if (index >= 0) {
                        boolean render = (info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0;
                        codec.releaseOutputBuffer(index, render);
                        if (render) { decoded++; awaitingSurface = true; }
                    }
                }
                if (pending != null) {
                    if (waitingKey && !keyframe(pending.payload, mime.equals("video/avc") ? 0 : 1)) { skipped++; pending = null; }
                    else {
                        int index = codec.dequeueInputBuffer(0);
                        if (index >= 0) {
                            ByteBuffer input = codec.getInputBuffer(index);
                            require(input != null && pending.payload.length <= input.capacity(), "AU exceeds decoder input buffer");
                            input.clear(); input.put(pending.payload);
                            codec.queueInputBuffer(index, 0, pending.payload.length, (System.nanoTime() / 1000), 0);
                            submitted++; waitingKey = false; pending = null;
                        }
                    }
                }
                if (waitingKey) requestKeyframe();
                Thread.sleep(1);
            }
            return surfaceFrames > 0 && decoded > 0 ? 0 : 2;
        } finally {
            closeNetwork();
            try { if (codec != null) { if (started) codec.stop(); codec.release(); } }
            finally { if (sink != null) sink.close(); }
        }
    }
    void closeNetwork() {
        stopped.set(true);
        try { if (tcp != null) tcp.close(); } catch (IOException ignored) {}
        if (udp != null) udp.close();
        for (Thread t : new Thread[]{tcpThread, udpThread}) if (t != null) {
            try { t.join(300); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        }
    }
    void summary(int status) {
        System.out.println("RESULT status=" + status + " transport=" + (wifi ? "wifi-udp" : "usb-tcp")
                + " tcp_messages=" + tcpMessages.get() + " tcp_video=" + tcpVideo.get()
                + " udp_datagrams=" + datagrams.get() + " udp_video=" + udpVideo.get()
                + " submitted=" + submitted + " decoder_outputs=" + decoded + " surface_frames=" + surfaceFrames
                + " configured=" + configuredWidth + "x" + configuredHeight + " output=" + outputWidth + "x" + outputHeight
                + " expired=" + assembly.expired + " invalid=" + assembly.invalid + " duplicates=" + assembly.duplicates
                + " gaps=" + assembly.gaps + " skipped_until_key=" + skipped + " key_requests=" + requests
                + " sampled_frames=" + sampledFrames + " sampled_pixels=" + sampledPixels
                + " nonblack_sample_pixels=" + nonblackSamples + " headset_rendering_tested=false");
    }

    static final class Sink implements AutoCloseable {
        static final int SIDE = 16;
        EGLDisplay display = EGL14.EGL_NO_DISPLAY; EGLContext context = EGL14.EGL_NO_CONTEXT;
        EGLSurface pbuffer = EGL14.EGL_NO_SURFACE;
        SurfaceTexture texture; Surface surface; HandlerThread callbacks;
        final AtomicBoolean available = new AtomicBoolean();
        int textureId, program; final float[] matrix = new float[16];
        final FloatBuffer quad = ByteBuffer.allocateDirect(8 * 4).order(ByteOrder.nativeOrder()).asFloatBuffer();
        final ByteBuffer pixels = ByteBuffer.allocateDirect(SIDE * SIDE * 4);
        Sink() throws IOException {
            try { init(); } catch (IOException | RuntimeException e) { close(); throw e; }
        }
        void init() throws IOException {
            display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
            int[] version = new int[2]; require(EGL14.eglInitialize(display, version, 0, version, 1), "eglInitialize failed");
            int[] attrs = {EGL14.EGL_RENDERABLE_TYPE, EGL14.EGL_OPENGL_ES2_BIT, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_PBUFFER_BIT,
                    EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8, EGL14.EGL_NONE};
            EGLConfig[] configs = new EGLConfig[1]; int[] count = new int[1];
            require(EGL14.eglChooseConfig(display, attrs, 0, configs, 0, 1, count, 0) && count[0] > 0, "no EGL pbuffer config");
            context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                    new int[]{EGL14.EGL_CONTEXT_CLIENT_VERSION, 2, EGL14.EGL_NONE}, 0);
            require(context != null && !context.equals(EGL14.EGL_NO_CONTEXT), "EGL context creation failed");
            pbuffer = EGL14.eglCreatePbufferSurface(display, configs[0],
                    new int[]{EGL14.EGL_WIDTH, SIDE, EGL14.EGL_HEIGHT, SIDE, EGL14.EGL_NONE}, 0);
            require(pbuffer != null && !pbuffer.equals(EGL14.EGL_NO_SURFACE)
                    && EGL14.eglMakeCurrent(display, pbuffer, pbuffer, context), "EGL make-current failed");
            int[] ids = new int[1]; GLES20.glGenTextures(1, ids, 0); textureId = ids[0];
            GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, textureId);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
            texture = new SurfaceTexture(textureId);
            callbacks = new HandlerThread("probe-surface-callback"); callbacks.start();
            texture.setOnFrameAvailableListener(t -> available.set(true), new Handler(callbacks.getLooper()));
            surface = new Surface(texture);
            String vertex = "attribute vec2 p; uniform mat4 m; varying vec2 uv; void main(){gl_Position=vec4(p,0.,1.);uv=(m*vec4((p+1.)*.5,0.,1.)).xy;}";
            String fragment = "#extension GL_OES_EGL_image_external : require\nprecision mediump float; varying vec2 uv; uniform samplerExternalOES s; void main(){gl_FragColor=texture2D(s,uv);}";
            int vs = shader(GLES20.GL_VERTEX_SHADER, vertex), fs = shader(GLES20.GL_FRAGMENT_SHADER, fragment);
            program = GLES20.glCreateProgram(); GLES20.glAttachShader(program, vs); GLES20.glAttachShader(program, fs);
            GLES20.glLinkProgram(program); GLES20.glDeleteShader(vs); GLES20.glDeleteShader(fs);
            GLES20.glGetProgramiv(program, GLES20.GL_LINK_STATUS, ids, 0); require(ids[0] != 0, "GL program link failed");
            quad.put(new float[]{-1, -1, 1, -1, -1, 1, 1, 1}).position(0);
            require(GLES20.glGetError() == GLES20.GL_NO_ERROR, "GL initialization failed");
        }
        static int shader(int type, String source) throws IOException {
            int id = GLES20.glCreateShader(type); GLES20.glShaderSource(id, source); GLES20.glCompileShader(id);
            int[] ok = new int[1]; GLES20.glGetShaderiv(id, GLES20.GL_COMPILE_STATUS, ok, 0);
            if (ok[0] == 0) { GLES20.glDeleteShader(id); throw new IOException("GL shader compilation failed"); }
            return id;
        }
        int sample() throws IOException {
            texture.getTransformMatrix(matrix); GLES20.glViewport(0, 0, SIDE, SIDE);
            GLES20.glUseProgram(program); GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
            GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, textureId);
            GLES20.glUniform1i(GLES20.glGetUniformLocation(program, "s"), 0);
            GLES20.glUniformMatrix4fv(GLES20.glGetUniformLocation(program, "m"), 1, false, matrix, 0);
            int p = GLES20.glGetAttribLocation(program, "p"); quad.position(0);
            GLES20.glEnableVertexAttribArray(p); GLES20.glVertexAttribPointer(p, 2, GLES20.GL_FLOAT, false, 0, quad);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4); pixels.clear();
            GLES20.glReadPixels(0, 0, SIDE, SIDE, GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, pixels);
            require(GLES20.glGetError() == GLES20.GL_NO_ERROR, "GL sampling failed");
            int nonblack = 0;
            for (int i = 0; i < SIDE * SIDE; i++) if ((pixels.get(i * 4) & 255) > 8
                    || (pixels.get(i * 4 + 1) & 255) > 8 || (pixels.get(i * 4 + 2) & 255) > 8) nonblack++;
            return nonblack;
        }
        public void close() {
            if (surface != null) surface.release();
            if (texture != null) texture.release();
            if (callbacks != null) callbacks.quitSafely();
            if (!display.equals(EGL14.EGL_NO_DISPLAY)) {
                if (program != 0) GLES20.glDeleteProgram(program);
                if (textureId != 0) GLES20.glDeleteTextures(1, new int[]{textureId}, 0);
                EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
                if (!pbuffer.equals(EGL14.EGL_NO_SURFACE)) EGL14.eglDestroySurface(display, pbuffer);
                if (!context.equals(EGL14.EGL_NO_CONTEXT)) EGL14.eglDestroyContext(display, context);
                EGL14.eglTerminate(display); EGL14.eglReleaseThread();
            }
        }
    }

    static byte[] packet(long seq, int index, int count, byte... payload) throws IOException {
        ByteArrayOutputStream b = new ByteArrayOutputStream(); DataOutputStream d = new DataOutputStream(b);
        d.writeBytes("ORVD"); d.writeByte(1); d.writeShort(index); d.writeShort(count); d.writeInt((int) seq); d.write(payload);
        return b.toByteArray();
    }
    interface Checked { void run() throws Exception; }
    static void rejects(Checked op) throws Exception {
        try { op.run(); } catch (IOException expected) { return; }
        throw new AssertionError("malformed input was accepted");
    }
    static void selfTest() throws Exception {
        Message m = parse(encode(103, new byte[]{1}, 0xffffffffL));
        require(m.type == 103 && m.payload[0] == 1 && m.sequence == 0xffffffffL, "protobuf roundtrip");
        rejects(() -> parse(new byte[]{18, 127})); rejects(() -> parse(new byte[]{8, (byte) 128}));
        rejects(() -> parse(new byte[]{8, 1, 8, 2})); rejects(() -> parse(new byte[]{0}));
        rejects(() -> parse(new byte[]{(byte) 0x80,(byte) 0x80,(byte) 0x80,(byte) 0x80,(byte) 0x80,
                (byte) 0x80,(byte) 0x80,(byte) 0x80,(byte) 0x80,2}));
        // Unknown fields must skip their own length prefix and data correctly.
        require(parse(new byte[]{42, 2, 99, 98, 8, 103}).type == 103, "unknown length-delimited field");
        byte[] cfg = ByteBuffer.allocate(12).putInt(1).putInt(1920).putInt(1080).array();
        require(config(cfg)[0] == 1, "HEVC config"); rejects(() -> config(new byte[11]));
        cfg[3] = 2; rejects(() -> config(cfg));
        require(keyframe(new byte[]{0,0,1,0x65,1}, 0) && !keyframe(new byte[]{0,0,0,1,0x41,1}, 0), "H264 NAL types");
        require(keyframe(new byte[]{0,0,0,1,0x26,1,1}, 1), "HEVC NAL types");
        Assembly a = new Assembly();
        byte[] second = packet(0xffffffffL, 1, 2, (byte) 3), first = packet(0xffffffffL, 0, 2, (byte) 1, (byte) 2);
        require(a.accept(second, second.length, 0) == null, "partial UDP");
        a.accept(second, second.length, 1);
        require(a.duplicates == 1, "duplicate UDP fragment");
        require(Arrays.equals(a.accept(first, first.length, 2).payload, new byte[]{1,2,3}), "out-of-order reassembly");
        byte[] wrap = packet(0, 0, 1, (byte) 4);
        require(a.accept(wrap, wrap.length, 3) != null && a.gaps == 0, "sequence wrap");
        require(a.accept(first, first.length, 4) == null, "late UDP frame");
        byte[] gap = packet(2, 0, 1, (byte) 5); a.accept(gap, gap.length, 5);
        require(a.recovery && a.gaps == 1, "gap recovery");
        byte[] partial = packet(3, 0, 2, (byte) 6); a.accept(partial, partial.length, 6);
        a.expire(SECOND); require(a.slots.isEmpty() && a.expired == 1, "assembly timeout");
        byte[] invalid = packet(4, 0, 0, (byte) 7); require(a.accept(invalid, invalid.length, SECOND) == null, "zero fragment count");
        for (int i = 0; i < 5; i++) { byte[] p = packet(10 + i, 0, 2, (byte) 1); a.accept(p, p.length, SECOND); }
        require(a.slots.size() <= 4, "bounded assembly slots");
        System.out.println("SELF_TEST_PASS protobuf/config/NAL/UDP bounds, duplicates, reorder, gaps, wrap, expiry");
    }
    public static void main(String[] args) {
        if (args.length == 1 && args[0].equals("--self-test")) {
            try { selfTest(); return; } catch (Exception e) { e.printStackTrace(); System.exit(2); }
        }
        StreamProbe probe = new StreamProbe(); int status = 2;
        try {
            require(args.length >= 1 && args.length <= 5, "usage: StreamProbe host [seconds] [port] [usb|wifi] [true|false sample]");
            int seconds = args.length > 1 ? Integer.parseInt(args[1]) : 10;
            int port = args.length > 2 ? Integer.parseInt(args[2]) : 38810;
            String transport = args.length > 3 ? args[3] : "usb";
            require(seconds >= 1 && seconds <= 30 && port > 0 && port <= 65535, "duration/port out of bounds");
            require(transport.equals("usb") || transport.equals("wifi"), "transport must be usb or wifi");
            require(args.length < 5 || args[4].equals("true") || args[4].equals("false"), "sample must be true or false");
            probe.wifi = transport.equals("wifi"); probe.sample = args.length > 4 && args[4].equals("true");
            Thread watchdog = new Thread(() -> {
                try { Thread.sleep((seconds + 8L) * 1000); } catch (InterruptedException ignored) { return; }
                System.err.println("WATCHDOG_TIMEOUT (includes socket/codec/GL setup and cleanup)");
                Runtime.getRuntime().halt(3);
            }, "probe-hard-deadline");
            watchdog.setDaemon(true); watchdog.start();
            status = probe.run(args[0], seconds, port);
        } catch (Throwable e) {
            for (Throwable cause = e; cause != null; cause = cause.getCause())
                System.err.println("ERROR " + cause.getClass().getSimpleName() + ": " + cause.getMessage());
        } finally { probe.closeNetwork(); probe.summary(status); }
        System.exit(status);
    }
}
