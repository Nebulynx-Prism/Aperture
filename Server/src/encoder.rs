//! Video encoding module for OpenReality.
//!
//! Provides ultra-low-latency hardware-accelerated video encoding supporting:
//! - NVIDIA NVENC, AMD AMF and Intel QuickSync (H.264 / HEVC)
//! - Windows Media Foundation fallback
//! AV1 is rejected until OBU temporal-unit framing is implemented.
//!
//! Automatically locates the encoder runtime (checking adjacent directories,
//! standard locations, and system PATH) to stream NAL units directly to VR clients.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

use crate::capture::FrameFormat;

/// Supported video codecs for VR streaming
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::H264 => write!(f, "H.264"),
            Self::Hevc => write!(f, "HEVC"),
            Self::Av1 => write!(f, "AV1"),
        }
    }
}

/// Encoding quality preset
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QualityPreset {
    UltraFast,
    VeryFast,
    Fast,
    Medium,
    Slow,
}

/// Configuration parameters for the video encoder
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
    pub codec: VideoCodec,
    pub keyframe_interval: u32,
    pub use_hardware: bool,
    pub quality_preset: QualityPreset,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            framerate: 90,
            bitrate_kbps: 80_000,
            codec: VideoCodec::H264,
            keyframe_interval: 180,
            use_hardware: true,
            quality_preset: QualityPreset::UltraFast,
        }
    }
}

/// An encoded video packet ready for network transmission
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub is_keyframe: bool,
    pub codec: VideoCodec,
}

/// Video encoder orchestrator
pub struct VideoEncoder {
    config: EncoderConfig,
    active_encoder_name: String,
    process: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout_rx: Option<Receiver<Result<Vec<u8>>>>,
    binary: PathBuf,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    force_keyframe_flag: Arc<AtomicBool>,
    frame_index: i64,
    output_index: i64,
}

impl VideoEncoder {
    /// Create a new video encoder with the specified configuration
    pub fn new(config: EncoderConfig) -> Result<Self> {
        if config.width == 0
            || config.height == 0
            || config.width % 2 != 0
            || config.height % 2 != 0
            || config.framerate == 0
            || config.bitrate_kbps == 0
            || config.keyframe_interval == 0
        {
            bail!("Encoder requires even nonzero dimensions, framerate, bitrate and GOP");
        }
        // OBU temporal units are not Annex-B NAL units. Never mislabel them.
        if config.codec == VideoCodec::Av1 {
            bail!("AV1 streaming requires an OBU temporal-unit packetizer; use H.264 or HEVC");
        }
        info!(
            "Creating VideoEncoder: {}x{} @ {}fps, {} kbps, codec: {}",
            config.width, config.height, config.framerate, config.bitrate_kbps, config.codec
        );

        let binary = find_encoder_binary()
            .context("FFmpeg not found; compressed video encoding is required")?;
        let hw_encoders = detect_hardware_encoders();
        let selected_encoder = select_best_encoder(config.codec, config.use_hardware, &hw_encoders);

        info!("Selected encoder backend: {selected_encoder}");

        let mut encoder = Self {
            config,
            active_encoder_name: selected_encoder,
            process: None,
            stdin: None,
            stdout_rx: None,
            binary: binary.clone(),
            stderr_tail: Arc::new(Mutex::new(Vec::new())),
            force_keyframe_flag: Arc::new(AtomicBool::new(true)),
            frame_index: 0,
            output_index: 0,
        };

        encoder.start_process(&binary)?;

        Ok(encoder)
    }

    /// Launch the low-latency encoder pipeline
    fn start_process(&mut self, binary: &PathBuf) -> Result<()> {
        let format_arg = match self.config.codec {
            VideoCodec::H264 => "h264",
            VideoCodec::Hevc => "hevc",
            VideoCodec::Av1 => "obu",
        };

        let mut cmd = Command::new(binary);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW for server/service use.
        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-probesize".to_string(),
            "32".to_string(),
            "-analyzeduration".to_string(),
            "0".to_string(),
            "-f".to_string(),
            "rawvideo".to_string(),
            "-pix_fmt".to_string(),
            "bgra".to_string(),
            "-s".to_string(),
            format!("{}x{}", self.config.width, self.config.height),
            "-r".to_string(),
            self.config.framerate.to_string(),
            "-i".to_string(),
            "-".to_string(),
            "-pix_fmt".to_string(),
            "nv12".to_string(),
            "-c:v".to_string(),
            self.active_encoder_name.clone(),
            "-b:v".to_string(),
            format!("{}k", self.config.bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", self.config.bitrate_kbps),
            "-bufsize".to_string(),
            format!(
                "{}k",
                (self.config.bitrate_kbps.saturating_mul(2) / self.config.framerate).max(500)
            ),
            "-g".to_string(),
            self.config.keyframe_interval.to_string(),
            "-bf".to_string(),
            "0".to_string(),
        ];

        if self.active_encoder_name.contains("qsv") {
            args.extend([
                "-preset".to_string(),
                "veryfast".to_string(),
                "-scenario".to_string(),
                "displayremoting".to_string(),
                "-forced_idr".to_string(),
                "1".to_string(),
                "-low_delay_brc".to_string(),
                "1".to_string(),
                "-async_depth".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-gpb".to_string(), "0".to_string()]);
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            } else if self.config.codec == VideoCodec::H264 {
                args.extend(["-profile:v".to_string(), "high".to_string()]);
            }
        } else if self.active_encoder_name.contains("nvenc") {
            args.extend([
                "-tune".to_string(),
                "ull".to_string(),
                "-zerolatency".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            } else if self.config.codec == VideoCodec::H264 {
                args.extend(["-profile:v".to_string(), "high".to_string()]);
            }
        } else if self.active_encoder_name.contains("amf") {
            args.extend(["-usage".to_string(), "ultralowlatency".to_string()]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            }
        } else if self.active_encoder_name.contains("mf") {
            args.extend([
                "-scenario".to_string(),
                "display_remoting".to_string(),
                "-rate_control".to_string(),
                "ld_vbr".to_string(),
            ]);
        } else if self.active_encoder_name.contains("x264")
            || self.active_encoder_name.contains("x265")
        {
            args.extend(["-tune".to_string(), "zerolatency".to_string()]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            }
        }

        // Insert AUDs per encoded packet for every backend, including MF.
        // AV1 is rejected in new(): it has no Annex-B AUD representation.
        args.extend([
            "-bsf:v".to_string(),
            match self.config.codec {
                VideoCodec::H264 => "h264_metadata=aud=insert",
                VideoCodec::Hevc => "hevc_metadata=aud=insert",
                VideoCodec::Av1 => unreachable!(),
            }
            .to_string(),
            "-flush_packets".to_string(),
            "1".to_string(),
            "-f".to_string(),
            format_arg.to_string(),
            "-".to_string(),
        ]);

        cmd.args(&args);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn encoder subprocess")?;
        let stderr = child.stderr.take();
        self.stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        self.process = Some(child);
        // Drain stderr concurrently, retaining only the last 8 KiB so FFmpeg
        // cannot deadlock on diagnostics or grow memory without bound.
        self.stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let tail = self.stderr_tail.clone();
        if let Some(mut stderr) = stderr {
            std::thread::Builder::new()
                .name("encoder-stderr-reader".into())
                .spawn(move || {
                    let mut bytes = [0u8; 1024];
                    loop {
                        match stderr.read(&mut bytes) {
                            Ok(0) => break,
                            Ok(n) => {
                                tracing::warn!("FFmpeg: {}", String::from_utf8_lossy(&bytes[..n]));
                                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                                tail.extend_from_slice(&bytes[..n]);
                                let excess = tail.len().saturating_sub(8192);
                                tail.drain(..excess);
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                })
                .context("Failed to spawn encoder stderr thread")?;
        }

        let codec = self.config.codec;
        if let Some(mut out) = stdout.take() {
            // Never block the pipe reader on a full queue: that can deadlock
            // the capture thread writing stdin. Fail instead of losing references.
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            std::thread::Builder::new()
                .name("encoder-stdout-reader".to_string())
                .spawn(move || {
                    let mut read_buf = vec![0u8; 128 * 1024];
                    let mut parser = AccessUnitParser::new(codec);
                    loop {
                        let result = match out.read(&mut read_buf) {
                            Ok(0) => parser.finish(),
                            Ok(n) => parser.push(&read_buf[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => Err(e.into()),
                        };
                        match result {
                            Ok(frames) => {
                                for frame in frames {
                                    if tx.try_send(Ok(frame)).is_err() {
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx.try_send(Err(e));
                                return;
                            }
                        }
                        if parser.finished {
                            return;
                        }
                    }
                })
                .context("Failed to spawn encoder stdout thread")?;
            self.stdout_rx = Some(rx);
        }
        info!(
            "Encoder subprocess started successfully ({})",
            self.active_encoder_name
        );
        Ok(())
    }

    /// Encode tightly packed BGRA matching the configured dimensions.
    /// Output is complete Annex-B access units, delayed by one AUD boundary.
    pub fn encode_frame(
        &mut self,
        frame_data: &[u8],
        width: u32,
        height: u32,
        format: FrameFormat,
    ) -> Result<Vec<EncodedPacket>> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .context("BGRA frame size overflow")?;
        if width != self.config.width
            || height != self.config.height
            || format != FrameFormat::Bgra8
            || frame_data.len() != expected
        {
            bail!(
                "Encoder input must be tightly packed BGRA at {}x{} (got {}x{}, {:?}, {} bytes)",
                self.config.width,
                self.config.height,
                width,
                height,
                format,
                frame_data.len()
            );
        }
        // A pipe cannot carry per-frame force-IDR commands. Restarting produces
        // fresh parameter sets and an actual IDR, rather than changing metadata.
        if self.force_keyframe_flag.swap(false, Ordering::SeqCst) && self.frame_index > 0 {
            self.stop_process();
            self.output_index = self.frame_index;
            self.start_process(&self.binary.clone())?;
        }
        if let Some(status) = self
            .process
            .as_mut()
            .context("Encoder process unavailable")?
            .try_wait()?
        {
            bail!(
                "FFmpeg encoder {} exited: {}: {}",
                self.active_encoder_name,
                status,
                self.stderr_text()
            );
        }
        let stdin = self.stdin.as_mut().context("Encoder stdin unavailable")?;
        let write_result = stdin.write_all(frame_data).and_then(|_| stdin.flush());
        write_result
            .with_context(|| format!("Writing BGRA to FFmpeg failed: {}", self.stderr_text()))?;
        self.frame_index += 1;
        let rx = self
            .stdout_rx
            .as_ref()
            .context("Encoder stdout unavailable")?;
        let mut frames = Vec::new();
        match rx.recv_timeout(Duration::from_millis(4)) {
            Ok(frame) => frames.push(frame?),
            Err(RecvTimeoutError::Timeout) => {
                if self.frame_index - self.output_index > i64::from(self.config.framerate) * 2 {
                    bail!("FFmpeg produced no complete access units for two seconds of input");
                }
                return Ok(Vec::new());
            }
            Err(RecvTimeoutError::Disconnected) => {
                bail!(
                    "FFmpeg output closed or output queue overflowed: {}",
                    self.stderr_text()
                )
            }
        }
        loop {
            match rx.try_recv() {
                Ok(frame) => frames.push(frame?),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    bail!(
                        "FFmpeg output closed or output queue overflowed: {}",
                        self.stderr_text()
                    )
                }
            }
        }
        Ok(frames
            .into_iter()
            .map(|data| {
                self.output_index += 1;
                EncodedPacket {
                    is_keyframe: is_random_access(&data, self.config.codec),
                    data,
                    pts: self.output_index,
                    dts: self.output_index,
                    codec: self.config.codec,
                }
            })
            .collect())
    }

    fn stderr_text(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&tail).trim().to_owned()
    }

    fn stop_process(&mut self) {
        self.stdin.take();
        self.stdout_rx.take();
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    /// Force the next frame to be an IDR keyframe (e.g. on client request or packet loss)
    pub fn force_keyframe(&mut self) {
        info!("Forcing IDR keyframe on next encode");
        self.force_keyframe_flag.store(true, Ordering::SeqCst);
    }

    /// Dynamically update encoding bitrate
    pub fn update_bitrate(&mut self, bitrate_kbps: u32) -> Result<()> {
        if bitrate_kbps == 0 {
            bail!("Bitrate must be nonzero");
        }
        self.config.bitrate_kbps = bitrate_kbps;
        self.force_keyframe_flag.store(true, Ordering::SeqCst);
        info!("Updated video bitrate target to {bitrate_kbps} kbps");
        Ok(())
    }

    /// Name of the currently active encoder
    pub fn active_encoder_name(&self) -> &str {
        &self.active_encoder_name
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        self.stop_process();
    }
}

// Bound both malformed-stream accumulation and memory retained per queued AU.
const MAX_ACCESS_UNIT: usize = 16 * 1024 * 1024;

/// Ordered Annex-B start codes, consuming four-byte prefixes before three-byte
/// prefixes so the same NAL is never reported twice. Partial headers wait for
/// the next read. Emulation-prevention bytes cannot match a start code.
fn nal_headers(data: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos + 3 < data.len() {
            let start = pos;
            let prefix = if data[pos..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if data[pos..].starts_with(&[0, 0, 1]) {
                3
            } else {
                pos += 1;
                continue;
            };
            pos += prefix;
            if pos < data.len() {
                return Some((start, pos));
            }
        }
        None
    })
}

fn nal_type(header: u8, codec: VideoCodec) -> u8 {
    match codec {
        VideoCodec::H264 => header & 0x1f,
        VideoCodec::Hevc => (header >> 1) & 0x3f,
        VideoCodec::Av1 => unreachable!("AV1 is not Annex-B"),
    }
}

fn is_random_access(data: &[u8], codec: VideoCodec) -> bool {
    nal_headers(data).any(|(_, h)| match codec {
        VideoCodec::H264 => nal_type(data[h], codec) == 5,
        // BLA, IDR and CRA pictures are random access points.
        VideoCodec::Hevc => (16..=21).contains(&nal_type(data[h], codec)),
        VideoCodec::Av1 => false,
    })
}

struct AccessUnitParser {
    codec: VideoCodec,
    pending: Vec<u8>,
    finished: bool,
}

impl AccessUnitParser {
    fn new(codec: VideoCodec) -> Self {
        Self {
            codec,
            pending: Vec::new(),
            finished: false,
        }
    }

    fn validate(&self, data: &[u8]) -> Result<()> {
        if data.ends_with(&[0, 0, 1]) {
            bail!("Truncated Annex-B start code without NAL header");
        }
        let mut aud = false;
        let mut picture = false;
        for (_, h) in nal_headers(data) {
            let ty = nal_type(data[h], self.codec);
            let header_len = if self.codec == VideoCodec::Hevc { 2 } else { 1 };
            if h + header_len >= data.len() {
                bail!("Truncated NAL header or missing NAL payload");
            }
            if data[h] & 0x80 != 0 || (self.codec == VideoCodec::Hevc && data[h + 1] & 7 == 0) {
                bail!("Invalid Annex-B NAL header");
            }
            aud |= ty
                == if self.codec == VideoCodec::H264 {
                    9
                } else {
                    35
                };
            picture |= if self.codec == VideoCodec::H264 {
                (1..=5).contains(&ty)
            } else {
                ty <= 31
            };
        }
        if !aud || !picture {
            bail!("FFmpeg access unit missing AUD or coded picture");
        }
        Ok(())
    }

    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
        if self.finished {
            bail!("Access unit parser already finished");
        }
        self.pending.extend_from_slice(bytes);
        let aud_type = if self.codec == VideoCodec::H264 {
            9
        } else {
            35
        };
        let mut frames = Vec::new();
        loop {
            // Headers before the first AUD belong to the first picture.
            // Search in byte order, rather than preferring four-byte AUDs.
            let boundary = nal_headers(&self.pending)
                .filter(|(_, h)| nal_type(self.pending[*h], self.codec) == aud_type)
                .nth(1)
                .map(|(start, _)| start);
            let Some(end) = boundary else {
                break;
            };
            if end > MAX_ACCESS_UNIT {
                bail!("FFmpeg access unit exceeds size limit");
            }
            self.validate(&self.pending[..end])?;
            frames.push(self.pending.drain(..end).collect());
        }
        if self.pending.len() > MAX_ACCESS_UNIT {
            bail!("FFmpeg access unit exceeds size limit (missing AUD?)");
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Vec<u8>>> {
        self.finished = true;
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        self.validate(&self.pending)?;
        Ok(vec![std::mem::take(&mut self.pending)])
    }
}

/// Prefer an explicit runtime, a portable adjacent runtime, then PATH.
/// The installed VD runtime is only an optional development fallback.
fn find_encoder_binary() -> Option<PathBuf> {
    if let Some(override_path) = std::env::var_os("PRISM_FFMPEG_PATH") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return std::fs::canonicalize(&path).ok().or(Some(path));
        }
        tracing::warn!("PRISM_FFMPEG_PATH is not a file: {}", path.display());
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("ffmpeg.exe");
            if path.is_file() {
                return Some(path);
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in ["ffmpeg.exe", "ffmpeg"] {
                let path = dir.join(name);
                if path.is_file() {
                    return std::fs::canonicalize(&path).ok().or(Some(path));
                }
            }
        }
    }

    let reference = PathBuf::from(r"C:\Program Files\Virtual Desktop Streamer\ffmpeg.exe");
    reference.is_file().then_some(reference)
}

/// Select best available encoder for codec
fn select_best_encoder(
    codec: VideoCodec,
    prefer_hw: bool,
    available_hw: &[(VideoCodec, String)],
) -> String {
    if prefer_hw {
        for (c, name) in available_hw {
            if *c == codec {
                return name.clone();
            }
        }
    }

    // Platform fallbacks (avoid libx264/libx265 which are missing in streaming ffmpeg builds)
    match codec {
        VideoCodec::H264 => "h264_mf".to_string(),
        VideoCodec::Hevc => "hevc_mf".to_string(),
        VideoCodec::Av1 => "libsvtav1".to_string(),
    }
}

/// Probe the host system for available hardware acceleration encoders
pub fn detect_hardware_encoders() -> Vec<(VideoCodec, String)> {
    let mut detected = Vec::new();

    // 1. Probe DirectX DXGI adapters for Intel, NVIDIA, and AMD GPU Vendor IDs
    #[cfg(windows)]
    unsafe {
        if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
            let mut adapter_idx = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
                if let Ok(desc) = adapter.GetDesc1() {
                    match desc.VendorId {
                        0x8086 => {
                            info!("Detected Intel GPU (VendorId 0x8086): enabling QuickSync (QSV)");
                            detected.push((VideoCodec::H264, "h264_qsv".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_qsv".to_string()));
                            detected.push((VideoCodec::Av1, "av1_qsv".to_string()));
                        }
                        0x10DE => {
                            info!("Detected NVIDIA GPU (VendorId 0x10DE): enabling NVENC");
                            detected.push((VideoCodec::H264, "h264_nvenc".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_nvenc".to_string()));
                            detected.push((VideoCodec::Av1, "av1_nvenc".to_string()));
                        }
                        0x1002 => {
                            info!("Detected AMD GPU (VendorId 0x1002): enabling AMF");
                            detected.push((VideoCodec::H264, "h264_amf".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_amf".to_string()));
                            detected.push((VideoCodec::Av1, "av1_amf".to_string()));
                        }
                        _ => {}
                    }
                }
                adapter_idx += 1;
            }
        }
    }

    // 2. Probe NVIDIA NVENC DLL
    if !detected.iter().any(|(_, n)| n.contains("nvenc")) {
        if std::path::Path::new(r"C:\Windows\System32\nvEncodeAPI64.dll").exists() {
            info!("Detected NVIDIA NVENC DLL");
            detected.push((VideoCodec::H264, "h264_nvenc".to_string()));
            detected.push((VideoCodec::Hevc, "hevc_nvenc".to_string()));
            detected.push((VideoCodec::Av1, "av1_nvenc".to_string()));
        }
    }

    // 3. Probe AMD AMF DLL
    if !detected.iter().any(|(_, n)| n.contains("amf")) {
        if std::path::Path::new(r"C:\Windows\System32\amfrt64.dll").exists() {
            info!("Detected AMD AMF DLL");
            detected.push((VideoCodec::H264, "h264_amf".to_string()));
            detected.push((VideoCodec::Hevc, "hevc_amf".to_string()));
            detected.push((VideoCodec::Av1, "av1_amf".to_string()));
        }
    }

    // 4. Always add Windows MediaFoundation as robust system fallback
    detected.push((VideoCodec::H264, "h264_mf".to_string()));
    detected.push((VideoCodec::Hevc, "hevc_mf".to_string()));

    detected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(prefix: usize, header: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; prefix - 1];
        bytes.push(1);
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn fixture(codec: VideoCodec) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (aud, parameter, idr, predicted): (&[u8], &[u8], &[u8], &[u8]) = match codec {
            VideoCodec::H264 => (&[0x09], &[0x67], &[0x65], &[0x41]),
            VideoCodec::Hevc => (&[0x46, 1], &[0x40, 1], &[0x26, 1], &[0x02, 1]),
            _ => unreachable!(),
        };
        // Parameter sets before first AUD, escaped start-code-like payload,
        // two slices per picture and mixed AUD lengths in chronological order.
        let mut first = nal(4, parameter, &[0xab]);
        first.extend(nal(4, aud, &[0x50]));
        first.extend(nal(3, idr, &[0x80, 0, 0, 3, 1, 0x42]));
        first.extend(nal(3, idr, &[0x40]));
        let mut second = nal(3, aud, &[0x50]);
        second.extend(nal(4, predicted, &[0x80]));
        let mut third = nal(4, aud, &[0x50]);
        third.extend(nal(3, predicted, &[0x80]));
        (first, second, third)
    }

    #[test]
    fn access_units_survive_every_chunk_boundary() {
        for codec in [VideoCodec::H264, VideoCodec::Hevc] {
            let (first, second, third) = fixture(codec);
            let stream = [first.clone(), second.clone(), third.clone()].concat();
            for split in 0..=stream.len() {
                let mut parser = AccessUnitParser::new(codec);
                let mut actual = parser.push(&stream[..split]).unwrap();
                actual.extend(parser.push(&stream[split..]).unwrap());
                actual.extend(parser.finish().unwrap());
                assert_eq!(
                    actual,
                    [first.clone(), second.clone(), third.clone()],
                    "{codec:?}, split {split}"
                );
            }
            let mut parser = AccessUnitParser::new(codec);
            let mut actual = Vec::new();
            for byte in &stream {
                actual.extend(parser.push(&[*byte]).unwrap());
            }
            actual.extend(parser.finish().unwrap());
            assert_eq!(actual, [first, second, third]);
        }
    }

    #[test]
    fn keyframe_metadata_comes_from_coded_picture() {
        for codec in [VideoCodec::H264, VideoCodec::Hevc] {
            let (idr, predicted, _) = fixture(codec);
            assert!(is_random_access(&idr, codec));
            assert!(!is_random_access(&predicted, codec));
        }
        for ty in 16..=21 {
            assert!(is_random_access(
                &nal(3, &[ty << 1, 1], &[0x80]),
                VideoCodec::Hevc
            ));
        }
    }

    #[test]
    fn incomplete_or_unframed_output_is_an_error() {
        let mut parser = AccessUnitParser::new(VideoCodec::H264);
        parser.push(&nal(4, &[0x65], &[0x80])).unwrap();
        assert!(parser.finish().is_err());
        let mut parser = AccessUnitParser::new(VideoCodec::H264);
        parser.push(&nal(4, &[0x09], &[0x50])).unwrap();
        assert!(parser.finish().is_err());
        let mut parser = AccessUnitParser::new(VideoCodec::Hevc);
        let mut truncated = nal(4, &[0x46, 1], &[0x50]);
        truncated.extend(nal(3, &[0x26], &[]));
        parser.push(&truncated).unwrap();
        assert!(parser.finish().is_err());
        assert!(AccessUnitParser::new(VideoCodec::H264)
            .finish()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn missing_delimiters_cannot_grow_memory_without_bound() {
        let mut parser = AccessUnitParser::new(VideoCodec::H264);
        assert!(parser.push(&vec![0xff; MAX_ACCESS_UNIT + 1]).is_err());
    }

    #[test]
    fn invalid_configs_and_av1_fail_before_launch() {
        let mut config = EncoderConfig::default();
        config.keyframe_interval = 0;
        assert!(VideoEncoder::new(config).is_err());
        let config = EncoderConfig {
            codec: VideoCodec::Av1,
            ..Default::default()
        };
        assert!(VideoEncoder::new(config).is_err());
    }

    /// Explicit opt-in: runs only the installed FFmpeg, never downloads tools.
    /// Both encode and decode children are killed/reaped after ten seconds.
    #[test]
    #[ignore]
    fn installed_ffmpeg_roundtrip() {
        use std::time::Instant;
        let binary = PathBuf::from(r"C:\Program Files\Virtual Desktop Streamer\ffmpeg.exe");
        assert!(binary.is_file());
        for codec in [VideoCodec::H264, VideoCodec::Hevc] {
            let mut encoder = VideoEncoder::new(EncoderConfig {
                width: 320,
                height: 240,
                framerate: 30,
                bitrate_kbps: 1000,
                keyframe_interval: 30,
                codec,
                ..Default::default()
            })
            .unwrap();
            // Exercise the actual encoder command and stdout parser. The writer
            // runs separately so a stalled driver cannot hang this test thread.
            let mut stdin = encoder.stdin.take().unwrap();
            let (live_tx, live_rx) = std::sync::mpsc::channel();
            let writer = std::thread::spawn(move || {
                let frame: Vec<u8> = (0..320 * 240).flat_map(|_| [40, 100, 220, 255]).collect();
                for _ in 0..4 {
                    stdin.write_all(&frame)?;
                }
                // Keep stdin open until the parser emits a complete live AU.
                // EOF alone must not be what makes the desktop visible.
                live_rx
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::TimedOut, e))?;
                Ok::<_, std::io::Error>(())
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut units = Vec::new();
            loop {
                match encoder
                    .stdout_rx
                    .as_ref()
                    .unwrap()
                    .recv_timeout(Duration::from_millis(20))
                {
                    Ok(unit) => {
                        units.push(unit.unwrap());
                        if units.len() == 1 {
                            live_tx.send(()).unwrap();
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if Instant::now() >= deadline {
                    encoder.stop_process();
                    panic!("{codec} encoding exceeded ten seconds");
                }
            }
            // Even after EOF, wait with a deadline instead of blocking wait().
            loop {
                if let Some(status) = encoder.process.as_mut().unwrap().try_wait().unwrap() {
                    assert!(status.success(), "{codec}: {status}");
                    break;
                }
                if Instant::now() >= deadline {
                    encoder.stop_process();
                    panic!("{codec} encoder did not exit");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            writer.join().unwrap().unwrap();
            assert_eq!(units.len(), 4, "{codec} access unit count");
            assert!(is_random_access(&units[0], codec));
            assert!(units[1..].iter().all(|u| !is_random_access(u, codec)));
            let compressed = units.concat();
            let mut child = Command::new(&binary)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    if codec == VideoCodec::H264 {
                        "h264"
                    } else {
                        "hevc"
                    },
                    "-i",
                    "pipe:0",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "bgra",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            let input_thread = std::thread::spawn(move || input.write_all(&compressed));
            let mut output = child.stdout.take().unwrap();
            let reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                output.read_to_end(&mut bytes).unwrap();
                bytes
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "{codec} decode: {status}");
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{codec} decode exceeded ten seconds");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            input_thread.join().unwrap().unwrap();
            let decoded = reader.join().unwrap();
            assert_eq!(decoded.len(), 320 * 240 * 4 * 4);
            // Known nonblack source: verify color channels rather than alpha.
            for pixel in decoded.chunks_exact(4) {
                assert!((pixel[0] as i16 - 40).abs() < 20);
                assert!((pixel[1] as i16 - 100).abs() < 20);
                assert!((pixel[2] as i16 - 220).abs() < 20);
            }
            eprintln!("{codec}: 4 complete access units decoded to expected BGRA colors");
        }
    }
}
