#pragma once
#include <cstdint>
struct AperturePose {
    uint32_t connected,valid;
    double position[3],orientation[4],linearVelocity[3],angularVelocity[3],timeOffset;
};
struct ApertureDisplay {
    uint32_t panelWidth,panelHeight,eyeWidth,eyeHeight,refresh,headset;
    float ipd,projection[8];
    uint64_t adapterLuid;
};
struct ApertureInput {uint32_t buttons;float trigger,grip,stick[2];};
static_assert(sizeof(AperturePose)==120,"Rust pose ABI");
static_assert(sizeof(ApertureDisplay)==72,"Rust display ABI");
static_assert(sizeof(ApertureInput)==20,"Rust input ABI");
extern "C" {
bool aperture_start(void (*log)(const char*));
void aperture_stop();
bool aperture_tick();
bool aperture_pose(int32_t hand,AperturePose* out);
bool aperture_display(ApertureDisplay* out);
bool aperture_input(uint32_t hand,ApertureInput* out);
void aperture_present(uint64_t sharedTexture,uint64_t frame);
bool aperture_vsync(float* seconds,uint64_t* frame);
}
