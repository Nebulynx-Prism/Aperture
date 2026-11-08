#pragma once
#include <cstdint>
#include <cstring>
#include <vector>
namespace openreality {
// Own Aperture motion extension, control message 109. Existing pose/FOV wire
// data remains byte-for-byte unchanged in message 105.
struct MotionPacket {
    uint32_t magic{0x4d505641},version{1};
    uint64_t sampleUs{0};int64_t targetLeadNs{0};
    uint32_t flags[3]{},timingValid{0};float velocity[3][6]{};
    std::vector<uint8_t> serialize()const {
        std::vector<uint8_t> bytes(sizeof(*this));std::memcpy(bytes.data(),this,sizeof(*this));return bytes;
    }
};
static_assert(sizeof(MotionPacket)==112,"Aperture motion wire layout");
}
