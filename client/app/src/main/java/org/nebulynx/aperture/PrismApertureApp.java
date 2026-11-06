package org.nebulynx.aperture;

import android.app.Application;
import android.content.Context;
import android.net.wifi.WifiManager;
import android.os.PowerManager;
import android.util.Log;

public class PrismApertureApp extends Application {
    private static final String TAG = "PrismApertureApp";

    private WifiManager.MulticastLock mMulticastLock;
    private PowerManager.WakeLock mWakeLock;

    static {
        try {
            System.loadLibrary("c++_shared");
            System.loadLibrary("openxr_loader");
            System.loadLibrary("prism_aperture_client");
            Log.i(TAG, "Native OpenXR and Prism Aperture libraries loaded successfully");
        } catch (UnsatisfiedLinkError e) {
            Log.e(TAG, "Failed to load native libraries: " + e.getMessage());
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        Log.i(TAG, "Prism Aperture initialized on " + DeviceProfile.getCurrent().deviceName);

        // Acquire MulticastLock for mDNS discovery on Meta Quest
        try {
            WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                mMulticastLock = wifi.createMulticastLock("PrismApertureMDNS");
                if (mMulticastLock != null) {
                    mMulticastLock.setReferenceCounted(true);
                    mMulticastLock.acquire();
                    Log.i(TAG, "MulticastLock acquired for local network discovery");
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "Could not acquire MulticastLock: " + e.getMessage());
        }

        // Prevent CPU throttling during active VR streaming
        try {
            PowerManager pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
            if (pm != null) {
                mWakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "prismaperture:streaming");
                if (mWakeLock != null) {
                    mWakeLock.acquire(10 * 60 * 1000L); // 10 minutes timeout
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "Could not acquire WakeLock: " + e.getMessage());
        }
    }

    @Override
    public void onTerminate() {
        super.onTerminate();
        try {
            if (mMulticastLock != null && mMulticastLock.isHeld()) {
                mMulticastLock.release();
            }
            if (mWakeLock != null && mWakeLock.isHeld()) {
                mWakeLock.release();
            }
        } catch (Exception e) {
            Log.e(TAG, "Error releasing locks: " + e.getMessage());
        }
    }
}
