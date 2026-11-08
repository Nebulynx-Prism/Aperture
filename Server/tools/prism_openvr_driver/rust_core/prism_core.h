#pragma once
#include <cstddef>
#include <cstdint>

struct PrismCorePose {
    float position[3];
    float orientation[4];
    uint32_t valid;
};

struct PrismCoreFramePose {
    uint32_t magic, version;
    uint64_t frame_sequence;
    PrismCorePose head;
    float fov[8];
    uint64_t sample_time_us;
};

struct PrismCoreDriverPose {
    uint32_t connected;
    uint32_t valid;
    double position[3];
    double orientation_xyzw[4];
};

struct PrismRuntimeTick {
    uint32_t connected, connection_changed, device_type, recommendation_changed;
    uint32_t recommended_width, recommended_height, fov_changed;
    float fov[8];
    uint32_t ipd_changed;
    float ipd;
    uint32_t refresh_changed, refresh_rate;
};

struct PrismRuntimeInput { uint32_t buttons; float axes[4]; };
struct PrismControllerConfig { uint32_t roleHint; char controllerType[32]; char profile[64]; char buttons[4][48]; char axes[4][48]; uint32_t scalarUnits[4]; };
struct PrismDisplayProperties { uint32_t hasVirtualDisplay, wireless, isOnDesktop, directMode; uint64_t universeId; float refreshRate, ipd, secondsToPhotons; };

extern "C" bool prism_pose_validate(const PrismCorePose* pose);
extern "C" bool prism_tracking_fresh(const void* sharedHeader);
extern "C" bool prism_tracking_pose(const void* sharedHeader, int32_t hand, PrismCoreDriverPose* out);
extern "C" bool prism_fov_to_projection(const float* fov, float* out);
extern "C" int32_t prism_adapter_rank(uint32_t vendor, bool isArc, uint64_t dedicatedMemory);
extern "C" bool prism_copy_frame_rows(const uint8_t* src, size_t srcPitch, uint8_t* dst,
                                        size_t width, size_t height, bool rgba);
extern "C" bool prism_ipc_open();
extern "C" bool prism_ipc_read(void* out, uint32_t timeoutMs);
extern "C" void prism_ipc_beat(bool ready);
extern "C" uint32_t prism_ipc_device_type();
extern "C" bool prism_ipc_device_resolution(uint32_t* width, uint32_t* height);
extern "C" uint64_t prism_ipc_publish_frame(const uint8_t* src, size_t srcPitch, uint32_t width,
                                               uint32_t height, bool rgba, const PrismCoreFramePose* pose);
extern "C" void prism_ipc_close();
extern "C" void prism_gpu_set_logger(void (*callback)(const char*));
extern "C" bool prism_gpu_preferred_luid(uint64_t* luid);
extern "C" uint64_t prism_gpu_present(uint64_t textureHandle, uint64_t frameId,
                                         const PrismCoreFramePose* pose, uint32_t* width,
                                         uint32_t* height, uint64_t* adapterLuid);
extern "C" void prism_gpu_reset();
extern "C" bool prism_gpu_dimensions(uint32_t* width, uint32_t* height);
extern "C" bool prism_gpu_eye_viewport(uint32_t eye, uint32_t* x, uint32_t* y, uint32_t* width, uint32_t* height);
extern "C" bool prism_runtime_start();
extern "C" void prism_runtime_cleanup();
extern "C" bool prism_runtime_tick(PrismRuntimeTick* result);
extern "C" bool prism_runtime_pose(int32_t hand, PrismCoreDriverPose* result);
extern "C" bool prism_runtime_input(int32_t hand, PrismRuntimeInput* result);
extern "C" bool prism_runtime_projection(uint32_t eye, float* result);
extern "C" bool prism_runtime_recommended(uint32_t* width, uint32_t* height);
extern "C" bool prism_runtime_frame_pose(PrismCoreFramePose* result);
extern "C" uint32_t prism_runtime_device_type();
extern "C" bool prism_runtime_vsync(float* secondsSinceVsync, uint64_t* frameCounter);
extern "C" bool prism_runtime_identity(uint32_t deviceType, int32_t hand, char* model, size_t modelCapacity, char* render, size_t renderCapacity);
extern "C" bool prism_runtime_controller_config(int32_t hand, PrismControllerConfig* config);
extern "C" bool prism_runtime_display_properties(bool redirect, PrismDisplayProperties* properties);
extern "C" bool prism_runtime_distortion(float u, float v, float* rgbCoordinates);
extern "C" bool prism_runtime_inverse_distortion(float u, float v, float* coordinates);
