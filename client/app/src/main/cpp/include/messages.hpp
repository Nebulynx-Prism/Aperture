#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace openreality {

// UDP Fast-path magic header: "ORVD"
constexpr uint8_t UDP_MAGIC[4] = {0x4F, 0x52, 0x56, 0x44};

constexpr uint8_t CHANNEL_VIDEO = 0x01;
constexpr uint8_t CHANNEL_AUDIO = 0x02;

constexpr uint16_t DEFAULT_CONTROL_PORT = 38810;
constexpr uint16_t DEFAULT_DATA_PORT = 38811;

enum class MessageType : uint32_t {
    PEER_INFO = 0,
    CRASH_CALLSTACK = 1,
    UPDATE_REQUIRED = 2,
    UPDATE_UNAVAILABLE = 3,
    UPDATE_IN_PROGRESS = 4,
    USER_SETTINGS_RELOAD = 5,
    USER_SETTINGS_PROPERTY_CHANGED = 6,
    MOBILE_SETTINGS_RELOAD = 7,
    MOBILE_SETTINGS_PROPERTY_CHANGED = 8,
    STREAMER_SETTINGS_RELOAD = 9,
    STREAMER_SETTINGS_PROPERTY_CHANGED = 10,
    BOUNDARY_DATA_CHANGED = 11,
    SKELETON_DATA_CHANGED = 12,
    RESTART_STREAM = 13,
    FORCE_KEY_FRAME = 14,
    SWITCH_MONITOR = 15,
    TOGGLE_DICTATION = 16,
    LAUNCH_STEAM_VR = 17,
    EXIT_STEAM_VR = 18,
    RECENTER = 19,
    REQUEST_GAMES = 20,
    GAMES = 21,
    REQUEST_GAME_THUMBNAILS = 22,
    GAME_THUMBNAIL = 23,
    LAUNCH_GAME = 24,
    EXIT_GAME = 25,
    GAME_EXITED = 26,
    SHOW_DESKTOP = 27,
    DISPOSE_GAMEPAD_EMULATION = 28,
    ADD_MONITOR = 29,
    REMOVE_MONITOR = 30,
    SET_MONITOR_PORTRAIT = 31,
    SCREENSHOT_TO_DESKTOP = 32,
    EXIT = 33,
    TOGGLE_VR_PASSTHROUGH = 34,
    TOGGLE_PERFORMANCE_OVERLAY = 35,
    TOGGLE_FOVEATED_STREAMING = 36,
    DISCONNECT = 37,
    REQUEST_CURSOR = 38,
    CURSOR = 39,
    CLIPBOARD_CHANGED = 40,
    CLIPBOARD_TEXT = 41,
    CLIPBOARD_IMAGE = 42,
    INPUT_EVENT = 100,
    VIDEO_FRAME = 101,
    STREAM_CONFIG = 102,
    STREAM_CONFIG_ACK = 103,
    STREAM_STATUS = 104,
    TRACKING = 105
};

enum class HmdType : uint32_t {
    UNKNOWN = 0,
    QUEST_1 = 1,
    QUEST_2 = 2,
    QUEST_PRO = 3,
    QUEST_3 = 4,
    QUEST_3S = 5
};

enum class VideoCodecType : uint32_t {
    H264 = 0,
    HEVC = 1,
    AV1 = 2
};

struct MobileSettings {
    uint32_t hmd_type{2}; // Quest 2 default, overridden by device detection
    uint32_t resolution_width{1920};
    uint32_t resolution_height{1080};
    bool is_portrait{false};
    float ipd{0.063f};
    float fov_left_up{50.0f};
    float fov_left_down{50.0f};
    float fov_left_left{50.0f};
    float fov_left_right{50.0f};
    float fov_right_up{50.0f};
    float fov_right_down{50.0f};
    float fov_right_left{50.0f};
    float fov_right_right{50.0f};
    uint32_t measured_bandwidth_mbps{150};
    bool use_spacewarp{false};
    uint32_t desktop_framerate{90};
    uint32_t desktop_bitrate_kbps{100000};
    uint32_t vr_bitrate_kbps{150000};
    uint32_t vr_framerate{90};
    bool foveated_streaming{true};
    uint32_t refresh_rate{90};
};

struct MouseInput {
    float x{0.0f};
    float y{0.0f};
    int32_t scroll_delta{0};
    uint32_t button_flags{0}; // 1 = Left, 2 = Right, 4 = Middle
    bool absolute{true};
};

struct ControllerInput {
    uint32_t button_flags{0};
    uint32_t touch_flags{0};
    float left_trigger{0.0f};
    float right_trigger{0.0f};
    float left_thumb_x{0.0f};
    float left_thumb_y{0.0f};
    float right_thumb_x{0.0f};
    float right_thumb_y{0.0f};
};

struct UdpHeader {
    uint8_t magic[4];       // "ORVD"
    uint8_t channel;        // 1=Video, 2=Audio
    uint16_t frag_idx;      // Big-endian
    uint16_t total_frags;   // Big-endian
    uint32_t sequence;      // Big-endian
} __attribute__((packed));

/// Protobuf wire message matching open_reality.NetMessage
struct NetMessage {
    uint32_t message_type{0};
    std::vector<uint8_t> payload;
    uint64_t timestamp{0};
    uint32_t sequence{0};

    std::vector<uint8_t> serialize() const {
        std::vector<uint8_t> buf;
        // Field 1: message_type (tag = (1 << 3) | 0 = 0x08)
        buf.push_back(0x08);
        writeVarint(buf, message_type);

        // Field 2: payload (tag = (2 << 3) | 2 = 0x12)
        if (!payload.empty()) {
            buf.push_back(0x12);
            writeVarint(buf, payload.size());
            buf.insert(buf.end(), payload.begin(), payload.end());
        }

        // Field 3: timestamp (tag = (3 << 3) | 0 = 0x18)
        if (timestamp > 0) {
            buf.push_back(0x18);
            writeVarint(buf, timestamp);
        }

        // Field 4: sequence (tag = (4 << 3) | 0 = 0x20)
        if (sequence > 0) {
            buf.push_back(0x20);
            writeVarint(buf, sequence);
        }
        return buf;
    }

    static bool deserialize(const uint8_t* data, size_t size, NetMessage& out) {
        size_t offset = 0;
        out.message_type = 0;
        out.payload.clear();
        out.timestamp = 0;
        out.sequence = 0;

        while (offset < size) {
            uint64_t tagAndWire = 0;
            if (!readVarint(data, size, offset, tagAndWire)) return false;
            uint32_t fieldNumber = static_cast<uint32_t>(tagAndWire >> 3);
            uint32_t wireType = static_cast<uint32_t>(tagAndWire & 0x07);

            if (wireType == 0) { // Varint
                uint64_t val = 0;
                if (!readVarint(data, size, offset, val)) return false;
                if (fieldNumber == 1) out.message_type = static_cast<uint32_t>(val);
                else if (fieldNumber == 3) out.timestamp = val;
                else if (fieldNumber == 4) out.sequence = static_cast<uint32_t>(val);
            } else if (wireType == 2) { // Length-delimited
                uint64_t len = 0;
                if (!readVarint(data, size, offset, len)) return false;
                if (offset + len > size) return false;
                if (fieldNumber == 2) {
                    out.payload.assign(data + offset, data + offset + len);
                }
                offset += len;
            } else {
                return false;
            }
        }
        return true;
    }

private:
    static void writeVarint(std::vector<uint8_t>& buf, uint64_t val) {
        while (val >= 0x80) {
            buf.push_back(static_cast<uint8_t>((val & 0x7F) | 0x80));
            val >>= 7;
        }
        buf.push_back(static_cast<uint8_t>(val));
    }

    static bool readVarint(const uint8_t* data, size_t size, size_t& offset, uint64_t& val) {
        val = 0;
        int shift = 0;
        while (offset < size) {
            uint8_t b = data[offset++];
            val |= static_cast<uint64_t>(b & 0x7F) << shift;
            if ((b & 0x80) == 0) return true;
            shift += 7;
            if (shift >= 64) return false;
        }
        return false;
    }
};

} // namespace openreality
