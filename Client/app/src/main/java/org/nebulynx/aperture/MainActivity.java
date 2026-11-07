package org.nebulynx.aperture;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.graphics.SurfaceTexture;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.view.Surface;
import android.view.View;
import android.view.WindowManager;

import java.net.InetAddress;

public class MainActivity extends Activity {
    private static final String TAG = "MainActivity";
    private static final int PERMISSION_REQUEST_CODE = 1001;

    private long mNativeHandle = 0;
    private DiscoveryHelper mDiscoveryHelper;

    private SurfaceTexture mVideoSurfaceTexture;
    private Surface mVideoSurface;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        DeviceProfile.HeadsetCapabilities caps = DeviceProfile.getCurrent();
        Log.i(TAG, "Prism Aperture MainActivity onCreate on " + caps.deviceName);

        // Keep screen awake and prevent lock while in VR
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        // Empty immersive black view: OpenXR compositor controls the HMD displays directly
        View blackView = new View(this);
        blackView.setBackgroundColor(0xFF000000);
        setContentView(blackView);

        checkPermissions();

        // 1. Initialize OpenXR Native Engine once
        mNativeHandle = nativeInit(
            caps.type.id,
            caps.defaultRenderWidth,
            caps.defaultRenderHeight,
            caps.defaultRefreshRate,
            caps.preferredCodec
        );

        // 2. Start Dual Discovery (USB Wired + Wi-Fi)
        mDiscoveryHelper = new DiscoveryHelper(this, new DiscoveryHelper.ServerFoundCallback() {
            @Override
            public void onServerFound(String name, InetAddress host, int port, boolean isWiredUsb) {
                final String hostAddress = host != null ? host.getHostAddress() : "127.0.0.1";
                Log.i(TAG, "Discovered server: " + name + " at " + hostAddress + ":" + port + " (USB=" + isWiredUsb + ")");
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        if (mNativeHandle != 0) {
                            nativeSetDiscoveredServer(mNativeHandle, name, hostAddress, port, isWiredUsb);
                        }
                    }
                });
            }
        });
    }

    private void checkPermissions() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[]{Manifest.permission.RECORD_AUDIO}, PERMISSION_REQUEST_CODE);
            }
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        Log.i(TAG, "MainActivity onResume");
        if (mNativeHandle != 0) {
            nativeOnResume(mNativeHandle);
        }
        if (mDiscoveryHelper != null) {
            mDiscoveryHelper.startDiscovery();
        }
    }

    @Override
    protected void onPause() {
        super.onPause();
        Log.i(TAG, "MainActivity onPause (VR compositor remains active)");
    }

    @Override
    protected void onDestroy() {
        if (mDiscoveryHelper != null) {
            mDiscoveryHelper.stopDiscovery();
            mDiscoveryHelper = null;
        }
        super.onDestroy();
        Log.i(TAG, "MainActivity onDestroy");
        if (mNativeHandle != 0) {
            nativeDestroy(mNativeHandle);
            mNativeHandle = 0;
        }
        if (mVideoSurface != null) {
            mVideoSurface.release();
            mVideoSurface = null;
        }
        if (mVideoSurfaceTexture != null) {
            mVideoSurfaceTexture.release();
            mVideoSurfaceTexture = null;
        }
    }

    // Called from C++ JNI to create off-screen hardware video surface
    public Surface createVideoSurface(int textureId) {
        Log.i(TAG, "Creating hardware video SurfaceTexture with GL texture ID: " + textureId);
        if (mVideoSurfaceTexture != null) {
            mVideoSurfaceTexture.release();
        }
        if (mVideoSurface != null) {
            mVideoSurface.release();
        }
        mVideoSurfaceTexture = new SurfaceTexture(textureId);
        mVideoSurface = new Surface(mVideoSurfaceTexture);
        return mVideoSurface;
    }

    // Called from C++ render loop to update hardware video frame
    public void updateVideoTexture() {
        if (mVideoSurfaceTexture != null) {
            try {
                mVideoSurfaceTexture.updateTexImage();
            } catch (Exception ignored) {}
        }
    }

    public long getVideoTextureTimestamp() {
        return mVideoSurfaceTexture != null ? mVideoSurfaceTexture.getTimestamp() : 0L;
    }

    // --- JNI Native Methods ---
    private native long nativeInit(
        int hmdType,
        int width,
        int height,
        float refreshRate,
        String preferredCodec
    );

    /** Resolve hostnames away from Android's UI thread before handing IPv4 to JNI. */
    public void connectToServer(String host, int port) {
        if (host == null || host.trim().isEmpty() || port < 1 || port > 65535) return;
        new Thread(() -> {
            try {
                final String address = DiscoveryHelper.resolveIpv4(host.trim()).getHostAddress();
                runOnUiThread(() -> {
                    if (!isDestroyed() && mNativeHandle != 0) nativeConnect(mNativeHandle, address, port);
                });
            } catch (java.net.UnknownHostException e) {
                Log.w(TAG, "IPv4 resolution failed for " + host, e);
            }
        }, "PA_HostResolver").start();
    }

    private native void nativeConnect(long handle, String host, int port);
    private native void nativeSetDiscoveredServer(long handle, String name, String host, int port, boolean isUsb);
    private native void nativeLaunchSteamVR(long handle);
    private native void nativeLaunchGame(long handle, String gameId);
    private native void nativeOnResume(long handle);
    private native void nativeOnPause(long handle);
    private native void nativeDestroy(long handle);
}

