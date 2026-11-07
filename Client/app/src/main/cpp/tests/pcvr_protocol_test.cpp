#include "pcvr_protocol.hpp"
#include "input_manager.hpp"
#include <cassert>
#include <cstdio>
using namespace openreality;
static uint32_t le(const std::vector<uint8_t>& p, size_t i) {
    return uint32_t(p[i]) | uint32_t(p[i+1])<<8 | uint32_t(p[i+2])<<16 | uint32_t(p[i+3])<<24;
}
int main() {
    {
        input_detail::MenuGesture menu; bool pulse=false;
        assert(!menu.step(true,true,10,pulse) && !pulse);
        assert(!menu.step(true,true,509,pulse) && !pulse);
        assert(menu.step(true,true,510,pulse) && !pulse);
        assert(!menu.step(true,true,1000,pulse) && !pulse);
        assert(!menu.step(true,false,1001,pulse) && !pulse);
        assert(!menu.step(true,true,2000,pulse) && !pulse);
        assert(!menu.step(true,false,2499,pulse) && pulse);
        assert(!menu.step(true,false,2500,pulse) && !pulse);
        menu.step(true,true,3000,pulse);
        assert(!menu.step(false,true,4000,pulse) && !pulse);
        assert(!menu.step(true,true,4500,pulse) && !pulse);
        menu.step(true,false,4501,pulse);
        menu.step(true,true,4510,pulse);
        assert(menu.step(true,true,5010,pulse) && !pulse);
    }
    TrackingPacket t;
    t.flags=1; t.sample_time_us=0x0102030405060708ULL;
    t.head.position[0]=1; t.head.orientation[3]=1; t.head.valid=1;
    t.hands[1].valid=1; t.trigger[0]=0.5f; t.buttons[1]=15;
    t.fov[7]=-1; t.ipd=0.064f; t.refresh_rate=72;
    auto p=t.serialize();
    assert(p.size()==192 && le(p,0)==1 && le(p,4)==1);
    assert(le(p,8)==0x05060708 && le(p,12)==0x01020304);
    assert(le(p,16)==0x3f800000 && le(p,44)==1 && le(p,108)==1);
    assert(le(p,112)==0x3f000000 && le(p,148)==15 && le(p,180)==0xbf800000 && le(p,188)==72);
    TrackingPacket invalid; auto zero=invalid.serialize();
    assert(le(zero,44)==0 && le(zero,76)==0 && le(zero,108)==0);
    StreamConfig c;
    std::vector<uint8_t> config{0,0,0,1, 0,0,8,0, 0,0,4,0, 0,0,0,1, 0x12,0x34,0x56,0x78};
    assert(StreamConfig::parse(config,c));
    assert(c.codec==1 && c.width==2048 && c.height==1024 && c.flags==1 && c.generation==0x12345678);
    assert((c.ack(true)==std::vector<uint8_t>{1,0x12,0x34,0x56,0x78}));
    assert(c.ack(false)[0]==0);
    config.resize(12); assert(StreamConfig::parse(config,c));
    assert(!c.extended && c.flags==0 && c.generation==0 && c.ack(true)==std::vector<uint8_t>{1});
    for(size_t n: {0u,11u,13u,19u,21u}) { config.resize(n); assert(!StreamConfig::parse(config,c)); }
    std::puts("PCVR tracking serialization and stream config tests passed");
}
