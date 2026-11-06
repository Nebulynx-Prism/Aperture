#include "audio_player.hpp"
#include <android/log.h>
#include <cstring>
#include <algorithm>

#define LOG_TAG "OpenReality_Audio"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

AudioPlayer::AudioPlayer() {
    m_ringBuffer.resize(48000 * 2); // 1-second ring buffer capacity
}

AudioPlayer::~AudioPlayer() {
    release();
}

bool AudioPlayer::init(int32_t sampleRate, int32_t channelCount) {
    release();

    m_sampleRate = sampleRate;
    m_channelCount = channelCount;

    AAudioStreamBuilder* builder = nullptr;
    aaudio_result_t result = AAudio_createStreamBuilder(&builder);
    if (result != AAUDIO_OK) {
        LOGE("Failed to create AAudio stream builder: %d", result);
        return false;
    }

    AAudioStreamBuilder_setSampleRate(builder, m_sampleRate);
    AAudioStreamBuilder_setChannelCount(builder, m_channelCount);
    AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_I16);
    AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
    AAudioStreamBuilder_setSharingMode(builder, AAUDIO_SHARING_MODE_EXCLUSIVE);
    AAudioStreamBuilder_setDataCallback(builder, dataCallback, this);

    result = AAudioStreamBuilder_openStream(builder, &m_stream);
    AAudioStreamBuilder_delete(builder);

    if (result != AAUDIO_OK) {
        LOGE("Failed to open AAudio stream: %d", result);
        return false;
    }

    result = AAudioStream_requestStart(m_stream);
    if (result != AAUDIO_OK) {
        LOGE("Failed to start AAudio stream: %d", result);
        AAudioStream_close(m_stream);
        m_stream = nullptr;
        return false;
    }

    m_running = true;
    LOGI("AAudio stream started successfully: %d Hz, %d channels", m_sampleRate, m_channelCount);
    return true;
}

void AudioPlayer::release() {
    m_running = false;
    if (m_stream) {
        AAudioStream_requestStop(m_stream);
        AAudioStream_close(m_stream);
        m_stream = nullptr;
    }
}

void AudioPlayer::enqueuePcmSamples(const int16_t* samples, size_t sampleCount) {
    std::lock_guard<std::mutex> lock(m_mutex);
    size_t cap = m_ringBuffer.size();
    for (size_t i = 0; i < sampleCount; ++i) {
        m_ringBuffer[m_writePos] = samples[i];
        m_writePos = (m_writePos + 1) % cap;
        if (m_writePos == m_readPos) {
            // Buffer overflow: drop oldest
            m_readPos = (m_readPos + 1) % cap;
        }
    }
}

aaudio_data_callback_result_t AudioPlayer::dataCallback(
    AAudioStream* /*stream*/,
    void* userData,
    void* audioData,
    int32_t numFrames
) {
    auto* player = static_cast<AudioPlayer*>(userData);
    return player->onAudioData(static_cast<int16_t*>(audioData), numFrames);
}

aaudio_data_callback_result_t AudioPlayer::onAudioData(int16_t* audioData, int32_t numFrames) {
    std::lock_guard<std::mutex> lock(m_mutex);
    size_t samplesNeeded = (size_t)numFrames * (size_t)m_channelCount;
    size_t cap = m_ringBuffer.size();

    size_t available = (m_writePos >= m_readPos) ? (m_writePos - m_readPos) : (cap - m_readPos + m_writePos);

    size_t toCopy = std::min(samplesNeeded, available);
    for (size_t i = 0; i < toCopy; ++i) {
        audioData[i] = m_ringBuffer[m_readPos];
        m_readPos = (m_readPos + 1) % cap;
    }

    // Silence if underrun
    if (toCopy < samplesNeeded) {
        std::memset(audioData + toCopy, 0, (samplesNeeded - toCopy) * sizeof(int16_t));
    }

    return AAUDIO_CALLBACK_RESULT_CONTINUE;
}

} // namespace openreality
