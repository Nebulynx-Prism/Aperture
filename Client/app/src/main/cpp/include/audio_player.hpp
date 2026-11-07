#pragma once

#include <cstdint>
#include <vector>
#include <mutex>
#include <aaudio/AAudio.h>

namespace openreality {

class AudioPlayer {
public:
    AudioPlayer();
    ~AudioPlayer();

    bool init(int32_t sampleRate = 48000, int32_t channelCount = 2);
    void release();

    void enqueuePcmSamples(const int16_t* samples, size_t sampleCount);

private:
    AAudioStream* m_stream{nullptr};
    int32_t m_sampleRate{48000};
    int32_t m_channelCount{2};
    bool m_running{false};
    std::mutex m_mutex;

    std::vector<int16_t> m_ringBuffer;
    size_t m_readPos{0};
    size_t m_writePos{0};

    static aaudio_data_callback_result_t dataCallback(
        AAudioStream* stream,
        void* userData,
        void* audioData,
        int32_t numFrames
    );

    aaudio_data_callback_result_t onAudioData(int16_t* audioData, int32_t numFrames);
};

} // namespace openreality
