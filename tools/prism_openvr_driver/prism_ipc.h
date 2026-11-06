#pragma once
#include <stdint.h>
#include <stddef.h>

// Prism-owned ABI. No Virtual Desktop or ALVR shared-memory layouts are used.
#define PRISM_MAPPING_NAME L"Local\\PrismAperture.OpenVR.v1"
#define PRISM_MUTEX_NAME L"Local\\PrismAperture.OpenVR.Mutex.v1"
constexpr uint32_t PRISM_MAGIC = 0x52564150; // PAVR
constexpr uint32_t PRISM_ABI_VERSION = 1;
constexpr uint32_t PRISM_PIXEL_OFFSET = 4096;
constexpr uint32_t PRISM_MAX_WIDTH = 4096;
constexpr uint32_t PRISM_MAX_HEIGHT = 2048;
constexpr uint32_t PRISM_MAPPING_BYTES = PRISM_PIXEL_OFFSET + PRISM_MAX_WIDTH * PRISM_MAX_HEIGHT * 4;
constexpr uint32_t PRISM_EYE_WIDTH = 1440;
constexpr uint32_t PRISM_EYE_HEIGHT = 1584;

struct PrismPose {
    float position[3];
    float orientation[4]; // quaternion x,y,z,w
    uint32_t valid;
};
// TCP message type 105, 192-byte little-endian payload. Sent at headset refresh.
struct PrismTracking {
    uint32_t version;
    uint32_t flags; // bit0: stage/floor origin; otherwise LOCAL with head-level origin
    uint64_t sample_time_us; // client monotonic; NOT a Windows timestamp
    PrismPose head;
    PrismPose hands[2]; // grip poses, left then right
    float trigger[2];
    float grip[2];
    float stick[4]; // left x,y, right x,y
    uint32_t buttons[2]; // bit0 primary, bit1 secondary, bit2 menu, bit3 stick-click
    float fov[8]; // left/right eye: angleLeft,angleRight,angleUp,angleDown in radians
    float ipd;
    uint32_t refresh_rate;
};
struct PrismSharedHeader {
    uint32_t magic;
    uint32_t version;
    uint64_t server_heartbeat_ms; // GetTickCount64 on PC
    uint64_t driver_heartbeat_ms;
    uint64_t tracking_received_ms;
    uint64_t frame_sequence;
    uint64_t frame_time_ms;
    uint32_t client_connected;
    uint32_t frame_width;
    uint32_t frame_height;
    uint32_t frame_stride;
    uint32_t driver_ready;
    uint32_t frame_format; // 0: tightly packed BGRA8, SBS left then right
    PrismTracking tracking;
};
static_assert(sizeof(PrismPose) == 32, "pose ABI");
static_assert(sizeof(PrismTracking) == 192, "tracking wire ABI");
static_assert(offsetof(PrismSharedHeader, tracking) == 72, "tracking offset ABI");
static_assert(sizeof(PrismSharedHeader) == 264, "shared header ABI");

// Optional metadata in the reserved area. Existing tracking/header/pixel offsets
// remain ABI v1 compatible. Updated only while holding PRISM_MUTEX_NAME.
constexpr uint32_t PRISM_DEVICE_INFO_OFFSET = 264;
constexpr uint32_t PRISM_DEVICE_INFO_MAGIC = 0x49445650; // PVDI
struct PrismDeviceInfo {
    uint32_t magic;
    uint32_t version;
    uint32_t hmd_type; // Android HmdType: 0 unknown, 1 Quest, 2 Quest2, 3 Pro, 4 Quest3, 5 Quest3S
    uint32_t reserved; // packed uint16 eye_width, eye_height from mobile settings
};
static_assert(sizeof(PrismDeviceInfo) == 16, "device metadata ABI");
static_assert(PRISM_DEVICE_INFO_OFFSET + sizeof(PrismDeviceInfo) <= PRISM_PIXEL_OFFSET, "reserved metadata fits");
constexpr uint32_t PRISM_FRAME_POSE_OFFSET = 280;
constexpr uint32_t PRISM_FRAME_POSE_MAGIC = 0x46505650;
struct PrismFramePose {
    uint32_t magic, version;
    uint64_t frame_sequence;
    PrismPose head;
    float fov[8];
    uint64_t sample_time_us;
};
static_assert(sizeof(PrismFramePose) == 88, "capture pose metadata ABI");
static_assert(PRISM_FRAME_POSE_OFFSET + sizeof(PrismFramePose) <= PRISM_PIXEL_OFFSET, "capture metadata fits");
