#pragma once

#include <cstdint>
#include <string>
#include <vector>
#include <mutex>
#include <atomic>
#include <map>
#include <android/native_window.h>
#include <media/NdkMediaCodec.h>
#include <media/NdkMediaFormat.h>
#include "messages.hpp"
#include "video_frame_pose.hpp"

namespace openreality {

class VideoDecoder {
public:
    VideoDecoder();
    ~VideoDecoder();

    bool init(ANativeWindow* surface, VideoCodecType codecType, int width, int height);
    // init binds the borrowed window; type 102 must configure the stream before decoding.
    bool configureStream(VideoCodecType codecType, int width, int height);
    void release();

    bool decodeFrame(const uint8_t* data, size_t size, uint64_t ptsUs, bool isKeyframe, const VideoFramePose* pose=nullptr);
    VideoFramePose framePose(uint64_t ptsUs) const {
        std::unique_lock<std::mutex> lock(m_mutex,std::try_to_lock);
        if(!lock.owns_lock()) return {};
        auto it=m_framePoses.find(ptsUs);return it==m_framePoses.end()?VideoFramePose{}:it->second;
    }
    void flush();
    void drainOutput();
    bool hasOutputFrame() const { return m_hasOutputFrame.load(); }

    bool isInitialized() const { std::lock_guard<std::mutex> lock(m_mutex); return m_initialized; }
    VideoCodecType getCodecType() const { std::lock_guard<std::mutex> lock(m_mutex); return m_codecType; }

    int getDecodedFrameWidth() const { return m_width.load(std::memory_order_relaxed); }
    int getDecodedFrameHeight() const { return m_height.load(std::memory_order_relaxed); }

private:
    bool configureLocked(VideoCodecType codecType, int width, int height);
    int m_streamWidth{0};
    int m_streamHeight{0};
    void releaseLocked();
    bool drainOutputLocked();
    std::atomic<bool> m_hasOutputFrame{false};
    std::map<uint64_t,VideoFramePose> m_framePoses;
    AMediaCodec* m_codec{nullptr};
    ANativeWindow* m_surface{nullptr};
    VideoCodecType m_codecType{VideoCodecType::HEVC};
    std::atomic<int> m_width{1920};
    std::atomic<int> m_height{1080};
    bool m_initialized{false};
    mutable std::mutex m_mutex;

    const char* getMimeType(VideoCodecType type);
};

} // namespace openreality
