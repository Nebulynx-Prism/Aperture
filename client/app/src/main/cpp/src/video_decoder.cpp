#include "video_decoder.hpp"
#include <android/log.h>
#include <cstring>

#define LOG_TAG "OpenReality_Decoder"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

VideoDecoder::VideoDecoder() = default;

VideoDecoder::~VideoDecoder() {
    release();
}

const char* VideoDecoder::getMimeType(VideoCodecType type) {
    switch (type) {
        case VideoCodecType::H264:
            return "video/avc";
        case VideoCodecType::HEVC:
            return "video/hevc";
        case VideoCodecType::AV1:
            return "video/av01"; // Hardware AV1 on Snapdragon XR2 Gen 2 (Quest 3 & 3S)
    }
    return "video/hevc";
}

bool VideoDecoder::init(ANativeWindow* surface, VideoCodecType codecType, int width, int height) {
    std::lock_guard<std::mutex> lock(m_mutex);
    releaseLocked();
    // Borrowed from ClientEngine, which releases it after decoder shutdown.
    m_surface = surface;
    (void)codecType;
    (void)width;
    (void)height;
    return surface != nullptr;
}

bool VideoDecoder::configureStream(VideoCodecType codecType, int width, int height) {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (!m_surface || width <= 0 || height <= 0 ||
        (codecType != VideoCodecType::H264 && codecType != VideoCodecType::HEVC &&
         codecType != VideoCodecType::AV1)) {
        releaseLocked();
        return false;
    }
    // A new configuration invalidates output from the previous generation.
    return configureLocked(codecType, width, height);
}

bool VideoDecoder::configureLocked(VideoCodecType codecType, int width, int height) {
    // Preserve m_surface across codec replacement.
    releaseLocked();
    m_streamWidth = width;
    m_streamHeight = height;
    m_codecType = codecType;
    m_width = width;
    m_height = height;

    const char* mime = getMimeType(codecType);
    LOGI("Initializing hardware AMediaCodec for MIME: %s (%dx%d)", mime, width, height);

    m_codec = AMediaCodec_createDecoderByType(mime);
    if (!m_codec) {
        LOGE("Failed to create AMediaCodec decoder for %s", mime);
        return false;
    }

    AMediaFormat* format = AMediaFormat_new();
    AMediaFormat_setString(format, AMEDIAFORMAT_KEY_MIME, mime);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_WIDTH, width);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_HEIGHT, height);

    // Standard Android low-latency decoding hint
    AMediaFormat_setInt32(format, "low-latency", 1);
    AMediaFormat_setInt32(format, "priority", 0); // Real-time priority

    media_status_t status = AMediaCodec_configure(m_codec, format, m_surface, nullptr, 0);
    AMediaFormat_delete(format);

    if (status != AMEDIA_OK) {
        LOGE("AMediaCodec_configure failed with status %d", status);
        AMediaCodec_delete(m_codec);
        m_codec = nullptr;
        return false;
    }

    status = AMediaCodec_start(m_codec);
    if (status != AMEDIA_OK) {
        LOGE("AMediaCodec_start failed with status %d", status);
        AMediaCodec_delete(m_codec);
        m_codec = nullptr;
        return false;
    }

    m_initialized = true;
    LOGI("Hardware video decoder started successfully for %s", mime);
    return true;
}

void VideoDecoder::release() {
    std::lock_guard<std::mutex> lock(m_mutex);
    releaseLocked();
}

void VideoDecoder::releaseLocked() {
    m_hasOutputFrame = false;
    m_framePoses.clear();
    if (m_codec) {
        AMediaCodec_stop(m_codec);
        AMediaCodec_delete(m_codec);
        m_codec = nullptr;
    }
    m_initialized = false;
}

void VideoDecoder::flush() {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (m_codec && m_initialized) {
        if (AMediaCodec_flush(m_codec) != AMEDIA_OK) releaseLocked();
        m_hasOutputFrame = false;
    }
}

bool VideoDecoder::decodeFrame(const uint8_t* data, size_t size, uint64_t ptsUs, bool isKeyframe, const VideoFramePose* pose) {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (!m_codec || !m_initialized) {
        return false;
    }

    if (!data || !size || !drainOutputLocked()) return false;
    if(pose && pose->valid) { m_framePoses[ptsUs]=*pose; while(m_framePoses.size()>64)m_framePoses.erase(m_framePoses.begin()); }
    (void)isKeyframe;
    // Keep the TCP receive/decode path from waiting a large fraction of a
    // headset frame for codec input. Output is drained before this call, so a
    // short poll is enough when the hardware decoder is keeping up.
    ssize_t inputBufIdx = AMediaCodec_dequeueInputBuffer(m_codec, 1000);
    if (inputBufIdx >= 0) {
        size_t bufSize = 0;
        uint8_t* inputBuf = AMediaCodec_getInputBuffer(m_codec, (size_t)inputBufIdx, &bufSize);
        if (inputBuf && bufSize >= size) {
            std::memcpy(inputBuf, data, size);
            uint32_t flags = 0; // Standard frame NALU in Annex B format
            media_status_t qStatus = AMediaCodec_queueInputBuffer(m_codec, (size_t)inputBufIdx, 0, size, ptsUs, flags);
            if (qStatus != AMEDIA_OK) {
                LOGW("queueInputBuffer failed (%d), resetting decoder...", qStatus);
                releaseLocked();
                return false;
            }
        } else {
            LOGW("Input buffer too small: required %zu, available %zu", size, bufSize);
            releaseLocked();
            return false;
        }
    } else if (inputBufIdx == AMEDIACODEC_INFO_TRY_AGAIN_LATER) {
        LOGW("Decoder input backpressure: access unit not queued");
        return false;
    } else {
        // Unexpected critical failure from dequeueInputBuffer (e.g. fatal hardware error)
        LOGW("dequeueInputBuffer fatal error (%zd), resetting decoder...", inputBufIdx);
        releaseLocked();
        return false;
    }

    return drainOutputLocked();
}
void VideoDecoder::drainOutput() {
    std::unique_lock<std::mutex> lock(m_mutex,std::try_to_lock);
    if(!lock.owns_lock()) return; // Input-side MediaCodec waits must not stall XR rendering.
    if (m_codec && m_initialized) drainOutputLocked();
}
bool VideoDecoder::drainOutputLocked() {
    ssize_t pending=-1;
    for (;;) {
        AMediaCodecBufferInfo info{};
        ssize_t index = AMediaCodec_dequeueOutputBuffer(m_codec, &info, 0);
        if (index >= 0) {
            // Surface output is opaque: byte-buffer size is not a reliable
            // frame-presence test. Render every ordinary output index, even
            // with size 0. Suppress codec config and empty EOS markers, while
            // retaining a final frame delivered together with EOS.
            const bool codecConfig = info.flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG;
            const bool emptyEos = (info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) && info.size == 0;
            const bool render = !codecConfig && !emptyEos;
            // Keep only the newest decoded image for the next SurfaceTexture
            // latch. Rendering every queued image adds buffer-queue latency.
            if(render) {
                if(pending>=0 && AMediaCodec_releaseOutputBuffer(m_codec,static_cast<size_t>(pending),false)!=AMEDIA_OK) { releaseLocked(); return false; }
                pending=index;
                continue;
            }
            if (AMediaCodec_releaseOutputBuffer(m_codec, static_cast<size_t>(index), false) != AMEDIA_OK) {
                releaseLocked();
                return false;
            }
        } else if (index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            AMediaFormat* format = AMediaCodec_getOutputFormat(m_codec);
            if (format) {
                int outputWidth=m_width.load(std::memory_order_relaxed);
                int outputHeight=m_height.load(std::memory_order_relaxed);
                AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_WIDTH, &outputWidth);
                AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_HEIGHT, &outputHeight);
                m_width.store(outputWidth,std::memory_order_relaxed);
                m_height.store(outputHeight,std::memory_order_relaxed);
                LOGI("Decoder output format: %s", AMediaFormat_toString(format));
                AMediaFormat_delete(format);
            }
        } else if (index == AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED) {
            continue;
        } else if (index == AMEDIACODEC_INFO_TRY_AGAIN_LATER) {
            if(pending>=0) {
                if(AMediaCodec_releaseOutputBuffer(m_codec,static_cast<size_t>(pending),true)!=AMEDIA_OK) { releaseLocked(); return false; }
                m_hasOutputFrame=true;
            }
            return true;
        } else {
            LOGE("Decoder output error: %zd", index);
            releaseLocked();
            return false;
        }
    }
}
} // namespace openreality
