package org.nebulynx.aperture;

import android.content.Context;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.util.Log;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.Inet4Address;
import java.net.UnknownHostException;
import java.net.InetSocketAddress;
import java.net.Socket;

public final class DiscoveryHelper {
    private static final String TAG = "DiscoveryHelper";
    private static final String SERVICE_TYPE_PRISM = "_prismaperture._tcp.";
    private static final int DEFAULT_PORT = 38810;

    public interface ServerFoundCallback {
        void onServerFound(String name, InetAddress host, int port, boolean isWiredUsb);
    }

    private final Context mContext;
    private final ServerFoundCallback mCallback;
    private NsdManager mNsdManager;
    private NsdManager.DiscoveryListener mDiscoveryListener;
    private volatile boolean mIsSearching = false;
    private volatile long mGeneration;
    private DatagramSocket mBroadcastSocket;
    private Thread mBroadcastThread;
    private Thread mUsbProbeThread;

    public DiscoveryHelper(Context context, ServerFoundCallback callback) {
        mContext = context;
        mCallback = callback;
    }

    public synchronized void startDiscovery() {
        if (mIsSearching) return;
        mIsSearching = true;
        final long generation = ++mGeneration;
        Log.i(TAG, "Starting discovery for Prism Aperture servers (Wi-Fi + USB Wired)...");

        // 1. USB Wired option: Probe 127.0.0.1:38810 in background
        mUsbProbeThread = new Thread(() -> {
            while (isActive(generation)) {
                try (Socket testSocket = new Socket()) {
                    testSocket.connect(new InetSocketAddress("127.0.0.1", DEFAULT_PORT), 400);
                    Log.i(TAG, "USB Wired link verified on 127.0.0.1:" + DEFAULT_PORT);
                    report(generation, "Prism Aperture PC (USB Wired)", InetAddress.getByName("127.0.0.1"), DEFAULT_PORT, true);
                    break;
                } catch (Exception ignored) {
                    try { Thread.sleep(2000); } catch (InterruptedException e) { break; }
                }
            }
        }, "PA_UsbProbeThread");
        mUsbProbeThread.start();

        // 2. Android NsdManager mDNS discovery
        try {
            mNsdManager = (NsdManager) mContext.getSystemService(Context.NSD_SERVICE);
            mDiscoveryListener = new NsdManager.DiscoveryListener() {
                @Override
                public void onDiscoveryStarted(String regType) {
                    Log.d(TAG, "NsdManager discovery started: " + regType);
                }

                @Override
                public void onServiceFound(NsdServiceInfo service) {
                    if (isActive(generation) && mNsdManager != null) {
                        try {
                            mNsdManager.resolveService(service, new NsdManager.ResolveListener() {
                                @Override
                                public void onResolveFailed(NsdServiceInfo serviceInfo, int errorCode) {
                                    Log.w(TAG, "Resolve failed for " + serviceInfo.getServiceName() + ": " + errorCode);
                                }

                                @Override
                                public void onServiceResolved(NsdServiceInfo serviceInfo) {
                                    String name = serviceInfo.getServiceName();
                                    Log.i(TAG, "Discovered Wi-Fi server: " + name + " at " + serviceInfo.getHost() + ":" + serviceInfo.getPort());
                                    report(generation, name + " (Wi-Fi)", serviceInfo.getHost(), serviceInfo.getPort(), false);
                                }
                            });
                        } catch (Exception e) {
                            Log.w(TAG, "resolveService exception: " + e.getMessage());
                        }
                    }
                }

                @Override
                public void onServiceLost(NsdServiceInfo service) {
                    Log.d(TAG, "Nsd service lost: " + service.getServiceName());
                }

                @Override
                public void onDiscoveryStopped(String serviceType) {
                    Log.d(TAG, "NsdManager discovery stopped");
                }

                @Override
                public void onStartDiscoveryFailed(String serviceType, int errorCode) {
                    Log.e(TAG, "Start discovery failed: " + errorCode);
                }

                @Override
                public void onStopDiscoveryFailed(String serviceType, int errorCode) {
                    Log.e(TAG, "Stop discovery failed: " + errorCode);
                }
            };

            mNsdManager.discoverServices(SERVICE_TYPE_PRISM, NsdManager.PROTOCOL_DNS_SD, mDiscoveryListener);
        } catch (Exception e) {
            Log.w(TAG, "NsdManager error: " + e.getMessage());
        }

        // 3. Direct UDP broadcast fallback query for Wi-Fi
        mBroadcastThread = new Thread(() -> {
            DatagramSocket socket = null;
            try {
                socket = new DatagramSocket();
                synchronized (DiscoveryHelper.this) {
                    if (!isActive(generation)) { socket.close(); return; }
                    mBroadcastSocket = socket;
                }
                socket.setBroadcast(true);
                socket.setSoTimeout(2500);

                byte[] query = "PA_DISCOVER\n".getBytes("UTF-8");
                InetAddress broadcastAddr = InetAddress.getByName("255.255.255.255");
                DatagramPacket packet = new DatagramPacket(query, query.length, broadcastAddr, DEFAULT_PORT);
                // Send again after every receive window so late-starting servers are found.

                byte[] buffer = new byte[1024];
                DatagramPacket responsePacket = new DatagramPacket(buffer, buffer.length);
                while (isActive(generation)) {
                    socket.send(packet);
                    long deadline = android.os.SystemClock.elapsedRealtime() + 2500;
                    while (isActive(generation) && android.os.SystemClock.elapsedRealtime() < deadline) {
                        try {
                            responsePacket.setLength(buffer.length);
                            socket.setSoTimeout((int)Math.max(1, deadline - android.os.SystemClock.elapsedRealtime()));
                            socket.receive(responsePacket);
                            String msg = new String(responsePacket.getData(), 0, responsePacket.getLength(), "UTF-8").trim();
                            if (msg.equals("PA_SERVER") || msg.startsWith("PA_SERVER;") || msg.equals("OR_SERVER") || msg.startsWith("OR_SERVER;")) {
                                String[] parts = msg.split(";");
                                String name = parts.length > 1 ? parts[1] : "Prism Aperture PC";
                                int port = DEFAULT_PORT;
                                if (parts.length > 2) {
                                    try {
                                        port = Integer.parseInt(parts[2]);
                                    } catch (NumberFormatException ignored) {}
                                }
                                report(generation, name + " (Wi-Fi)", responsePacket.getAddress(), port, false);
                            }
                        } catch (java.net.SocketTimeoutException ignored) {
                            break;
                        }
                    }
                }
            } catch (Exception e) {
                Log.d(TAG, "UDP broadcast scan: " + e.getMessage());
            } finally {
                if (socket != null && !socket.isClosed()) {
                    socket.close();
                }
            }
        }, "PA_DiscoveryThread");
        mBroadcastThread.start();
    }

    private boolean isActive(long generation) {
        return mIsSearching && mGeneration == generation;
    }

    /** Native networking accepts AF_INET only. Call from a worker thread. */
    public static Inet4Address resolveIpv4(String hostname) throws UnknownHostException {
        for (InetAddress address : InetAddress.getAllByName(hostname)) {
            if (address instanceof Inet4Address) return (Inet4Address) address;
        }
        throw new UnknownHostException("No IPv4 address for " + hostname);
    }

    private synchronized void report(long generation, String name, InetAddress host,
                                     int port, boolean usb) {
        if (!isActive(generation) || host == null || port < 1 || port > 65535) return;
        // Reject IPv6-only NSD results rather than passing them to AF_INET native code.
        if (!(host instanceof Inet4Address)) return;
        mCallback.onServerFound(name, host, port, usb);
    }
    public synchronized void stopDiscovery() {
        if (!mIsSearching) return;
        mIsSearching = false;
        ++mGeneration;
        if (mBroadcastSocket != null) {
            mBroadcastSocket.close();
            mBroadcastSocket = null;
        }
        if (mNsdManager != null && mDiscoveryListener != null) {
            try {
                mNsdManager.stopServiceDiscovery(mDiscoveryListener);
            } catch (Exception e) {
                Log.e(TAG, "Error stopping discovery: " + e.getMessage());
            }
            mDiscoveryListener = null;
        }
        if (mBroadcastThread != null) {
            mBroadcastThread.interrupt();
            mBroadcastThread = null;
        }
        if (mUsbProbeThread != null) {
            mUsbProbeThread.interrupt();
            mUsbProbeThread = null;
        }
    }
}

