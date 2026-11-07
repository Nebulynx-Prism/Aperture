#pragma once
#include <cstdint>
#include <cstring>
#include <vector>
namespace openreality {
// Explicit wire encoding of tools/prism_openvr_driver/prism_ipc.h.
struct TrackingPose { float position[3]{}; float orientation[4]{}; uint32_t valid{}; };
struct TrackingPacket {
    uint32_t version{1}, flags{}; uint64_t sample_time_us{};
    TrackingPose head{}, hands[2]{};
    float trigger[2]{}, grip[2]{}, stick[4]{};
    uint32_t buttons[2]{}; float fov[8]{}, ipd{}; uint32_t refresh_rate{};
    std::vector<uint8_t> serialize() const {
        std::vector<uint8_t> out; out.reserve(192);
        auto u32 = [&](uint32_t v) { for(int i=0;i<4;++i) out.push_back(uint8_t(v>>(8*i))); };
        auto f32 = [&](float v) { uint32_t bits; std::memcpy(&bits,&v,4); u32(bits); };
        auto pose = [&](const TrackingPose& p) { for(float v:p.position) f32(v); for(float v:p.orientation) f32(v); u32(p.valid); };
        u32(version); u32(flags); u32(uint32_t(sample_time_us)); u32(uint32_t(sample_time_us>>32));
        pose(head); for(const auto& p:hands) pose(p);
        for(float v:trigger) f32(v); for(float v:grip) f32(v); for(float v:stick) f32(v);
        for(uint32_t v:buttons) u32(v); for(float v:fov) f32(v); f32(ipd); u32(refresh_rate);
        return out;
    }
};
static_assert(sizeof(TrackingPose)==32 && sizeof(TrackingPacket)==192, "Prism tracking ABI");
struct StreamConfig {
    uint32_t codec{}, width{}, height{}, flags{}, generation{}; bool extended{};
    static bool parse(const std::vector<uint8_t>& p, StreamConfig& c) {
        if(p.size()!=12 && p.size()!=20) return false;
        auto be = [&](size_t i) { return uint32_t(p[i])<<24 | uint32_t(p[i+1])<<16 | uint32_t(p[i+2])<<8 | p[i+3]; };
        c={}; c.codec=be(0); c.width=be(4); c.height=be(8); c.extended=p.size()==20;
        if(c.extended) { c.flags=be(12); c.generation=be(16); }
        return true;
    }
    std::vector<uint8_t> ack(bool ok) const {
        std::vector<uint8_t> p{uint8_t(ok)};
        if(extended) for(int i=3;i>=0;--i) p.push_back(uint8_t(generation>>(8*i)));
        return p;
    }
};
}
