#pragma once
#include <cmath>
#include <cstdint>
#include <cstring>
#include <vector>

namespace openreality {
struct VideoFramePose {
    bool valid{};
    uint64_t ptsUs{}, sampleTimeUs{};
    float orientation[4]{}, fov[8]{};
    static bool parse(const std::vector<uint8_t>& b, VideoFramePose& out) {
        out={}; if(b.size()!=72) return false;
        auto le32=[&](size_t i){return uint32_t(b[i])|uint32_t(b[i+1])<<8|uint32_t(b[i+2])<<16|uint32_t(b[i+3])<<24;};
        if(le32(0)!=1 || le32(4)!=1) return false;
        for(size_t i=8;i<16;i++) out.ptsUs=(out.ptsUs<<8)|b[i];
        auto f32=[&](size_t i){uint32_t u=le32(i);float f;std::memcpy(&f,&u,4);return f;};
        float norm=0;for(int i=0;i<4;i++){out.orientation[i]=f32(16+4*i);norm+=out.orientation[i]*out.orientation[i];}
        if(!std::isfinite(norm)||norm<.25f||norm>4.f)return false;
        for(float& v:out.orientation)v/=std::sqrt(norm);
        for(int i=0;i<8;i++){out.fov[i]=f32(32+4*i);if(!std::isfinite(out.fov[i])||std::abs(out.fov[i])>=1.57f)return false;}
        for(int e=0;e<2;e++)if(out.fov[e*4]>=0||out.fov[e*4+1]<=0||out.fov[e*4+2]<=0||out.fov[e*4+3]>=0)return false;
        out.sampleTimeUs=uint64_t(le32(64))|uint64_t(le32(68))<<32;
        out.valid=out.ptsUs!=0 && out.sampleTimeUs!=0;return out.valid;
    }
};
}
