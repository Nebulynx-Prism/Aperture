package org.nebulynx.aperture;

import android.os.Build;
import android.util.Log;

public final class DeviceProfile {
    private static final String TAG = "DeviceProfile";

    public enum HeadsetType {
        UNKNOWN(0, "Generic XR Device"),
        QUEST_1(1, "Oculus Quest"),
        QUEST_2(2, "Meta Quest 2"),
        QUEST_PRO(3, "Meta Quest Pro"),
        QUEST_3(4, "Meta Quest 3"),
        QUEST_3S(5, "Meta Quest 3S");

        public final int id;
        public final String displayName;

        HeadsetType(int id, String displayName) {
            this.id = id;
            this.displayName = displayName;
        }

        public boolean isSnapdragonXR2Gen2() {
            return this == QUEST_3 || this == QUEST_3S;
        }

        public boolean supportsAv1Decoding() {
            return isSnapdragonXR2Gen2();
        }

        public boolean supportsColorPassthrough() {
            return this == QUEST_3 || this == QUEST_3S || this == QUEST_PRO;
        }
    }

    public static final class HeadsetCapabilities {
        public final HeadsetType type;
        public final String deviceName;
        public final int defaultRenderWidth;
        public final int defaultRenderHeight;
        public final int recommendedBitrateMbps;
        public final int maxBitrateMbps;
        public final float[] availableRefreshRates;
        public final float defaultRefreshRate;
        public final String preferredCodec;
        public final boolean supportsHandTrackingV2;
        public final boolean foveatedStreamingSupported;

        public HeadsetCapabilities(
            HeadsetType type,
            String deviceName,
            int defaultRenderWidth,
            int defaultRenderHeight,
            int recommendedBitrateMbps,
            int maxBitrateMbps,
            float[] availableRefreshRates,
            float defaultRefreshRate,
            String preferredCodec,
            boolean supportsHandTrackingV2,
            boolean foveatedStreamingSupported
        ) {
            this.type = type;
            this.deviceName = deviceName;
            this.defaultRenderWidth = defaultRenderWidth;
            this.defaultRenderHeight = defaultRenderHeight;
            this.recommendedBitrateMbps = recommendedBitrateMbps;
            this.maxBitrateMbps = maxBitrateMbps;
            this.availableRefreshRates = availableRefreshRates;
            this.defaultRefreshRate = defaultRefreshRate;
            this.preferredCodec = preferredCodec;
            this.supportsHandTrackingV2 = supportsHandTrackingV2;
            this.foveatedStreamingSupported = foveatedStreamingSupported;
        }
    }

    private static volatile HeadsetCapabilities sCurrent = null;

    public static HeadsetCapabilities getCurrent() {
        if (sCurrent == null) {
            synchronized (DeviceProfile.class) {
                if (sCurrent == null) {
                    sCurrent = detectCapabilities();
                }
            }
        }
        return sCurrent;
    }

    private static HeadsetCapabilities detectCapabilities() {
        String model = Build.MODEL != null ? Build.MODEL.toLowerCase() : "";
        String device = Build.DEVICE != null ? Build.DEVICE.toLowerCase() : "";
        String product = Build.PRODUCT != null ? Build.PRODUCT.toLowerCase() : "";

        Log.i(TAG, "Detecting device profile. Model: " + model + ", Device: " + device + ", Product: " + product);

        if (model.contains("quest 3s") || device.contains("panther") || product.contains("eureka_s")) {
            Log.i(TAG, "Identified hardware: Meta Quest 3S (Snapdragon XR2 Gen 2, Fresnel lenses)");
            return new HeadsetCapabilities(
                HeadsetType.QUEST_3S,
                "Meta Quest 3S",
                1832, 1920,
                150, 400,
                new float[]{72.0f, 90.0f, 120.0f},
                90.0f,
                "AV1",
                true,
                true
            );
        }

        if (model.contains("quest 3") || device.contains("eureka") || product.contains("eureka")) {
            Log.i(TAG, "Identified hardware: Meta Quest 3 (Snapdragon XR2 Gen 2, Pancake lenses)");
            return new HeadsetCapabilities(
                HeadsetType.QUEST_3,
                "Meta Quest 3",
                2064, 2208,
                200, 500,
                new float[]{72.0f, 80.0f, 90.0f, 120.0f},
                90.0f,
                "AV1",
                true,
                true
            );
        }

        if (model.contains("quest pro") || device.contains("seacliff") || product.contains("seacliff")) {
            Log.i(TAG, "Identified hardware: Meta Quest Pro (Snapdragon XR2+ Gen 1)");
            return new HeadsetCapabilities(
                HeadsetType.QUEST_PRO,
                "Meta Quest Pro",
                1800, 1920,
                150, 300,
                new float[]{72.0f, 90.0f},
                90.0f,
                "HEVC",
                true,
                true
            );
        }

        if (model.contains("quest 2") || device.contains("hollywood") || product.contains("hollywood")) {
            Log.i(TAG, "Identified hardware: Meta Quest 2 (Snapdragon XR2 Gen 1)");
            return new HeadsetCapabilities(
                HeadsetType.QUEST_2,
                "Meta Quest 2",
                1832, 1920,
                100, 200,
                new float[]{72.0f, 80.0f, 90.0f, 120.0f},
                90.0f,
                "HEVC",
                true,
                true
            );
        }

        Log.i(TAG, "Identified hardware: Generic OpenXR Device (" + Build.MODEL + ")");
        return new HeadsetCapabilities(
            HeadsetType.UNKNOWN,
            Build.MODEL != null ? Build.MODEL : "Generic VR Headset",
            1920, 1920,
            80, 150,
            new float[]{72.0f, 90.0f},
            72.0f,
            "H264",
            false,
            false
        );
    }
}
