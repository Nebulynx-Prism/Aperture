#pragma once

#include <string>
#include <vector>
#include <thread>
#include <atomic>
#include <functional>
#include <map>
#include <mutex>
#include <condition_variable>
#include "messages.hpp"
#include "pcvr_protocol.hpp"
#include "video_decoder.hpp"
#include "audio_player.hpp"
#include "video_frame_pose.hpp"

namespace openreality {

struct FrameAssemblySlot {
    uint32_t sequence{0};
    uint16_t totalFragments{0};
    uint16_t receivedFragments{0};
    std::vector<std::vector<uint8_t>> fragments;
    uint64_t timestampUs{0};
};

class NetworkClient {
public:
    NetworkClient(VideoDecoder* decoder, AudioPlayer* audio);
    ~NetworkClient();

    bool connect(const std::string& host, uint16_t port = DEFAULT_CONTROL_PORT);
    void disconnect();

    bool isConnected() const { return m_connected.load(); }

    void sendInputEvent(const MouseInput& mouse);
    void sendControllerInput(const ControllerInput& controller);
    void sendMobileSettings(const MobileSettings& settings);

    void sendTrackingPacket(const TrackingPacket& packet);
    bool isStereoStream() const { return m_streamConfigured.load() && m_isStereoStream.load(); }
    void showDesktop();
    void launchSteamVR();
    void launchGame(const std::string& gameId);
    void requestGames();
    void setVrModeCallback(std::function<void(bool)> cb) { m_onVrModeChanged = cb; }

private:
    VideoDecoder* m_decoder{nullptr};
    AudioPlayer* m_audio{nullptr};
    std::function<void(bool)> m_onVrModeChanged;

    std::string m_serverHost;
    uint16_t m_serverControlPort{DEFAULT_CONTROL_PORT};
    uint16_t m_serverUdpPort{0};
    uint16_t m_clientUdpPort{0};

    int m_tcpSocket{-1};
    int m_udpSocket{-1};

    std::atomic<bool> m_connected{false};
    std::atomic<bool> m_running{false};
    uint32_t m_sendSequence{0};

    std::thread m_tcpThread;
    std::thread m_udpThread;
    std::thread m_trackingThread;
    std::mutex m_trackingMutex;
    std::condition_variable m_trackingWake;
    std::vector<uint8_t> m_pendingTracking;
    VideoFramePose m_pendingVideoPose;
    void runTrackingLoop();

    std::mutex m_connectMutex;
    std::mutex m_sendMutex;
    std::mutex m_reasmMutex;

    // Jitter / reassembly buffer (sequence -> slot)
    std::map<uint32_t, FrameAssemblySlot> m_assemblySlots;
    uint32_t m_lastDeliveredSeq{0};
    bool m_haveDeliveredSeq{false};
    std::atomic<bool> m_streamConfigured{false};
    std::atomic<bool> m_isStereoStream{false};
    bool m_waitingForKeyframe{true};
    uint64_t m_lastKeyframeRequestUs{0};

    void runTcpLoop();
    void runUdpLoop();

    bool sendTcpMessage(MessageType type, const std::vector<uint8_t>& payload);
    void handleUdpDatagram(const uint8_t* data, size_t size);
    void assembleAndDispatchFrame(FrameAssemblySlot& slot);
    void dispatchVideoFrame(const uint8_t* data, size_t size, uint64_t ptsUs, const VideoFramePose* pose=nullptr);
    void requestKeyframe();
};

} // namespace openreality
