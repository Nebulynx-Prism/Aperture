#include "network_client.hpp"
#include <android/log.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <fcntl.h>
#include <cstring>
#include <chrono>
#include <cerrno>

#define LOG_TAG "OpenReality_Net"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

namespace openreality {

namespace {
constexpr size_t MAX_VIDEO_BYTES = 8 * 1024 * 1024;
constexpr size_t MAX_ASSEMBLY_SLOTS = 8;
constexpr uint16_t MAX_FRAGMENTS = 8192;
uint64_t nowUs() {
    return std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count();
}
bool readAll(int socket, void* data, size_t size) {
    auto* bytes = static_cast<uint8_t*>(data);
    size_t offset = 0;
    while (offset < size) {
        ssize_t n = recv(socket, bytes + offset, size - offset, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        offset += static_cast<size_t>(n);
    }
    return true;
}
bool writeAll(int socket, const void* data, size_t size) {
    const auto* bytes = static_cast<const uint8_t*>(data);
    size_t offset = 0;
    while (offset < size) {
        ssize_t n = send(socket, bytes + offset, size - offset, MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        offset += static_cast<size_t>(n);
    }
    return true;
}
uint32_t readBe32(const uint8_t* p) {
    return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | p[3];
}
bool newer(uint32_t a, uint32_t b) { return static_cast<int32_t>(a - b) > 0; }
}

NetworkClient::NetworkClient(VideoDecoder* decoder, AudioPlayer* audio)
    : m_decoder(decoder), m_audio(audio) {}

NetworkClient::~NetworkClient() {
    disconnect();
}

bool NetworkClient::connect(const std::string& host, uint16_t port) {
    std::lock_guard<std::mutex> connLock(m_connectMutex);
    disconnect();

    m_serverHost = host;
    m_serverControlPort = port;

    LOGI("Connecting to Prism Aperture Server at %s:%u...", host.c_str(), port);

    // 1. Create TCP socket
    m_tcpSocket = socket(AF_INET, SOCK_STREAM, 0);
    if (m_tcpSocket < 0) {
        LOGE("Failed to create TCP socket: errno=%d", errno);
        return false;
    }

    int nodelay = 1;
    setsockopt(m_tcpSocket, IPPROTO_TCP, TCP_NODELAY, &nodelay, sizeof(nodelay));
    int keepalive = 1, idle = 10, interval = 3, probes = 3;
    setsockopt(m_tcpSocket, SOL_SOCKET, SO_KEEPALIVE, &keepalive, sizeof(keepalive));
    setsockopt(m_tcpSocket, IPPROTO_TCP, TCP_KEEPIDLE, &idle, sizeof(idle));
    setsockopt(m_tcpSocket, IPPROTO_TCP, TCP_KEEPINTVL, &interval, sizeof(interval));
    setsockopt(m_tcpSocket, IPPROTO_TCP, TCP_KEEPCNT, &probes, sizeof(probes));

    // Set 3-second socket timeout for send and recv operations
    struct timeval tv{};
    tv.tv_sec = 3;
    tv.tv_usec = 0;
    setsockopt(m_tcpSocket, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    tv.tv_sec = 0; tv.tv_usec = 10000;
    setsockopt(m_tcpSocket, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));

    sockaddr_in serverAddr{};
    serverAddr.sin_family = AF_INET;
    serverAddr.sin_port = htons(port);
    if (inet_pton(AF_INET, host.c_str(), &serverAddr.sin_addr) <= 0) {
        LOGE("Invalid IP address: %s", host.c_str());
        close(m_tcpSocket);
        m_tcpSocket = -1;
        return false;
    }

    // Set non-blocking to enforce a strict 3-second connection timeout
    int flags = fcntl(m_tcpSocket, F_GETFL, 0);
    fcntl(m_tcpSocket, F_SETFL, flags | O_NONBLOCK);

    int res = ::connect(m_tcpSocket, (struct sockaddr*)&serverAddr, sizeof(serverAddr));
    if (res < 0) {
        if (errno == EINPROGRESS) {
            fd_set fdset;
            FD_ZERO(&fdset);
            FD_SET(m_tcpSocket, &fdset);
            struct timeval timeout{};
            timeout.tv_sec = 3; // 3 second timeout
            timeout.tv_usec = 0;
            res = select(m_tcpSocket + 1, nullptr, &fdset, nullptr, &timeout);
            if (res > 0) {
                int so_error = 0;
                socklen_t len = sizeof(so_error);
                getsockopt(m_tcpSocket, SOL_SOCKET, SO_ERROR, &so_error, &len);
                if (so_error != 0) {
                    LOGW("TCP connect to %s:%u failed: error=%d", host.c_str(), port, so_error);
                    close(m_tcpSocket);
                    m_tcpSocket = -1;
                    return false;
                }
            } else {
                LOGW("TCP connect to %s:%u timed out after 3 seconds", host.c_str(), port);
                close(m_tcpSocket);
                m_tcpSocket = -1;
                return false;
            }
        } else {
            LOGW("TCP connect failed to %s:%u: errno=%d", host.c_str(), port, errno);
            close(m_tcpSocket);
            m_tcpSocket = -1;
            return false;
        }
    }

    // Restore flags
    fcntl(m_tcpSocket, F_SETFL, flags);

    // 2. Setup UDP socket
    m_udpSocket = socket(AF_INET, SOCK_DGRAM, 0);
    if (m_udpSocket < 0) {
        LOGE("Failed to create UDP socket");
        close(m_tcpSocket);
        m_tcpSocket = -1;
        return false;
    }

    // Increase UDP receive buffer size (4MB) to prevent packet loss
    int rcvbuf = 4 * 1024 * 1024;
    setsockopt(m_udpSocket, SOL_SOCKET, SO_RCVBUF, &rcvbuf, sizeof(rcvbuf));

    // Also set 1-second receive timeout on UDP socket
    struct timeval udpTv{};
    udpTv.tv_sec = 1;
    udpTv.tv_usec = 0;
    setsockopt(m_udpSocket, SOL_SOCKET, SO_RCVTIMEO, &udpTv, sizeof(udpTv));

    sockaddr_in clientUdpAddr{};
    clientUdpAddr.sin_family = AF_INET;
    clientUdpAddr.sin_addr.s_addr = INADDR_ANY;
    clientUdpAddr.sin_port = 0; // Bind ephemeral port

    if (bind(m_udpSocket, (struct sockaddr*)&clientUdpAddr, sizeof(clientUdpAddr)) < 0) {
        LOGE("Failed to bind UDP socket");
        close(m_tcpSocket);
        close(m_udpSocket);
        m_tcpSocket = -1;
        m_udpSocket = -1;
        return false;
    }

    socklen_t addrLen = sizeof(clientUdpAddr);
    getsockname(m_udpSocket, (struct sockaddr*)&clientUdpAddr, &addrLen);
    m_clientUdpPort = ntohs(clientUdpAddr.sin_port);
    LOGI("UDP data socket bound to local port %u", m_clientUdpPort);

    // 3. Handshake: Read server's UDP port (2 bytes big-endian)
    uint16_t srvUdpPortNet = 0;
    if (!readAll(m_tcpSocket, &srvUdpPortNet, 2)) {
        LOGE("Failed to receive server UDP port (errno=%d)", errno);
        if (m_tcpSocket >= 0) {
            shutdown(m_tcpSocket, SHUT_RDWR);
            close(m_tcpSocket);
            m_tcpSocket = -1;
        }
        if (m_udpSocket >= 0) {
            close(m_udpSocket);
            m_udpSocket = -1;
        }
        return false;
    }
    m_serverUdpPort = ntohs(srvUdpPortNet);
    serverAddr.sin_port = srvUdpPortNet;
    if (::connect(m_udpSocket, reinterpret_cast<sockaddr*>(&serverAddr), sizeof(serverAddr)) < 0) {
        LOGE("Failed to connect UDP endpoint: errno=%d", errno);
        close(m_tcpSocket);
        close(m_udpSocket);
        m_tcpSocket = m_udpSocket = -1;
        return false;
    }
    LOGI("Server assigned UDP data port: %u", m_serverUdpPort);

    // Send our local UDP port back to server (2 bytes big-endian)
    uint16_t cliUdpPortNet = htons(m_clientUdpPort);
    if (!writeAll(m_tcpSocket, &cliUdpPortNet, 2)) {
        LOGE("Failed to send client UDP port");
        if (m_tcpSocket >= 0) {
            shutdown(m_tcpSocket, SHUT_RDWR);
            close(m_tcpSocket);
            m_tcpSocket = -1;
        }
        if (m_udpSocket >= 0) {
            close(m_udpSocket);
            m_udpSocket = -1;
        }
        return false;
    }

    m_connected = true;
    m_running = true;

    // Reset SO_RCVTIMEO to 0 (blocking with TCP keepalive) for the background worker thread
    struct timeval zeroTv{};
    zeroTv.tv_sec = 0;
    zeroTv.tv_usec = 0;
    setsockopt(m_tcpSocket, SOL_SOCKET, SO_RCVTIMEO, &zeroTv, sizeof(zeroTv));

    // Start background threads
    m_tcpThread = std::thread(&NetworkClient::runTcpLoop, this);
    m_udpThread = std::thread(&NetworkClient::runUdpLoop, this);
    m_trackingThread = std::thread(&NetworkClient::runTrackingLoop, this);

    LOGI("Handshake complete. Prism Aperture streaming connection active!");
    return true;
}

void NetworkClient::disconnect() {
    m_running = false;
    m_connected = false;
    m_isStereoStream = false;
    m_trackingWake.notify_all();

    if (m_tcpSocket >= 0) {
        shutdown(m_tcpSocket, SHUT_RDWR);
    }
    if (m_udpSocket >= 0) {
        shutdown(m_udpSocket, SHUT_RDWR);
    }

    if (m_tcpThread.joinable() && m_tcpThread.get_id() != std::this_thread::get_id()) {
        m_tcpThread.join();
    }
    if (m_udpThread.joinable() && m_udpThread.get_id() != std::this_thread::get_id()) {
        m_udpThread.join();
    }
    if(m_trackingThread.joinable() && m_trackingThread.get_id()!=std::this_thread::get_id()) m_trackingThread.join();
    { std::lock_guard<std::mutex> lock(m_trackingMutex); m_pendingTracking.clear(); }
    // Keep descriptors stable until workers exit; prevent fd reuse races.
    {
        std::lock_guard<std::mutex> lock(m_sendMutex);
        if (m_tcpSocket >= 0) close(m_tcpSocket);
        if (m_udpSocket >= 0) close(m_udpSocket);
        m_tcpSocket = m_udpSocket = -1;
    }

    {
        std::lock_guard<std::mutex> lock(m_reasmMutex);
        m_assemblySlots.clear();
        m_lastDeliveredSeq = 0;
        m_haveDeliveredSeq = false;
        m_waitingForKeyframe = true;
        m_lastKeyframeRequestUs = 0;
        m_streamConfigured = false;
    }
}

bool NetworkClient::sendTcpMessage(MessageType type, const std::vector<uint8_t>& payload) {
    std::lock_guard<std::mutex> lock(m_sendMutex);
    if (!m_connected || m_tcpSocket < 0) return false;

    NetMessage netMsg;
    netMsg.message_type = static_cast<uint32_t>(type);
    netMsg.payload = payload;
    if(netMsg.message_type==111 && netMsg.payload.size()==24) {
        const auto sent=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now().time_since_epoch()).count());
        for(int i=0;i<8;++i)netMsg.payload[16+i]=static_cast<uint8_t>(sent>>(i*8));
    }
    netMsg.timestamp = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::system_clock::now().time_since_epoch()).count());
    netMsg.sequence = m_sendSequence++;

    std::vector<uint8_t> protoBytes = netMsg.serialize();

    // Wire format: [4-byte big-endian length][protobuf payload]
    uint32_t len = htonl(static_cast<uint32_t>(protoBytes.size()));
    if (!writeAll(m_tcpSocket, &len, 4) || !writeAll(m_tcpSocket, protoBytes.data(), protoBytes.size())) {
        m_connected = false;
        m_running = false;
        shutdown(m_tcpSocket, SHUT_RDWR);
        return false;
    }
    return true;
}

void NetworkClient::sendMobileSettings(const MobileSettings& settings) {
    // Pack MobileSettings into Protobuf payload
    // tag 1: hmd_type (varint), tag 2: resolution_width, tag 3: resolution_height, etc.
    std::vector<uint8_t> payload;
    // Simple wire encoding matching messages.proto:
    payload.push_back(0x08); // Field 1: hmd_type
    payload.push_back((uint8_t)settings.hmd_type);
    
    // Field 2: resolution_width
    payload.push_back(0x10);
    uint32_t w = settings.resolution_width;
    while (w >= 0x80) { payload.push_back((uint8_t)((w & 0x7F) | 0x80)); w >>= 7; }
    payload.push_back((uint8_t)w);

    // Field 3: resolution_height
    payload.push_back(0x18);
    uint32_t h = settings.resolution_height;
    while (h >= 0x80) { payload.push_back((uint8_t)((h & 0x7F) | 0x80)); h >>= 7; }
    payload.push_back((uint8_t)h);

    // Field 21: refresh_rate
    payload.push_back(0xA8); payload.push_back(0x01);
    payload.push_back((uint8_t)settings.refresh_rate);

    sendTcpMessage(MessageType::MOBILE_SETTINGS_PROPERTY_CHANGED, payload);
}

void NetworkClient::sendInputEvent(const MouseInput& mouse) {
    // Fixed wire layout; do not transmit compiler padding or native bool layout.
    std::vector<uint8_t> payload(17);
    auto writeLe32 = [&payload](size_t offset, uint32_t value) {
        for (int i = 0; i < 4; ++i) payload[offset + i] = uint8_t(value >> (8 * i));
    };
    uint32_t x, y;
    std::memcpy(&x, &mouse.x, sizeof(x));
    std::memcpy(&y, &mouse.y, sizeof(y));
    writeLe32(0, x);
    writeLe32(4, y);
    writeLe32(8, static_cast<uint32_t>(mouse.scroll_delta));
    writeLe32(12, mouse.button_flags);
    payload[16] = mouse.absolute ? 1 : 0;
    sendTcpMessage(MessageType::INPUT_EVENT, payload);
}

void NetworkClient::sendControllerInput(const ControllerInput& controller) {
    std::vector<uint8_t> payload(sizeof(ControllerInput));
    std::memcpy(payload.data(), &controller, sizeof(ControllerInput));
    sendTcpMessage(MessageType::INPUT_EVENT, payload);
}

void NetworkClient::sendTrackingPacket(const TrackingPacket& packet,const MotionPacket* motion) {
    if(!isConnected()) return;
    auto payload=packet.serialize();
    if(motion&&motion->timingValid){auto extension=motion->serialize();payload.insert(payload.end(),extension.begin(),extension.end());}
    {
        std::lock_guard<std::mutex> lock(m_trackingMutex);
        // Preserve a short-menu event if a newer pose replaces an unsent sample.
        if(m_pendingTracking.size()>=192) payload[144]|=m_pendingTracking[144]&4u;
        m_pendingTracking=std::move(payload);
    }
    m_trackingWake.notify_one();
}
void NetworkClient::runTrackingLoop() {
    while(m_running && m_connected) {
        std::vector<uint8_t> payload;
        {
            std::unique_lock<std::mutex> lock(m_trackingMutex);
            m_trackingWake.wait_for(lock,std::chrono::milliseconds(20),[this]{return !m_pendingTracking.empty() || !m_running || !m_connected;});
            if(!m_running || !m_connected) break;
            if(m_pendingTracking.empty()) continue;
            payload=std::move(m_pendingTracking);
        }
        // TCP sending never stalls xrWaitFrame/rendering; under backpressure
        // retain only the freshest next pose, not a growing latency queue.
        if(payload.size()<192)continue;
        std::vector<uint8_t> tracking(payload.begin(),payload.begin()+192);
        if(!sendTcpMessage(MessageType::TRACKING,tracking)) break;
        if(payload.size()==192+sizeof(MotionPacket)) {
            std::vector<uint8_t> motion(payload.begin()+192,payload.end());
            if(!sendTcpMessage(static_cast<MessageType>(109),motion)) break;
        }
    }
}
void NetworkClient::showDesktop() { sendTcpMessage(MessageType::SHOW_DESKTOP, {1}); }
void NetworkClient::launchSteamVR() {
    LOGI("Sending LAUNCH_STEAM_VR control command to server");
    sendTcpMessage(MessageType::LAUNCH_STEAM_VR, {});
}

void NetworkClient::launchGame(const std::string& gameId) {
    LOGI("Sending LAUNCH_GAME command for app ID: %s", gameId.c_str());
    std::vector<uint8_t> payload(gameId.begin(), gameId.end());
    sendTcpMessage(MessageType::LAUNCH_GAME, payload);
}

void NetworkClient::requestGames() {
    LOGI("Sending REQUEST_GAMES command to server");
    sendTcpMessage(MessageType::REQUEST_GAMES, {});
}

void NetworkClient::runTcpLoop() {
    uint8_t lenBuf[4];
    while (m_running && m_connected) {
        if (!readAll(m_tcpSocket, lenBuf, sizeof(lenBuf))) {
            LOGW("TCP control channel closed or truncated");
            break;
        }
        uint32_t len = readBe32(lenBuf);
        if (len == 0 || len > 16 * 1024 * 1024) {
            LOGE("Invalid TCP frame size: %u", len);
            break;
        }
        std::vector<uint8_t> buf(len);
        if (!readAll(m_tcpSocket, buf.data(), buf.size())) {
            LOGE("Truncated TCP payload");
            break;
        }
        NetMessage msg;
        if (NetMessage::deserialize(buf.data(), buf.size(), msg)) {
            if (msg.message_type == 102) {
                StreamConfig config;
                m_streamConfigured = false;
                m_isStereoStream = false;
                if (!StreamConfig::parse(msg.payload, config)) {
                    LOGE("Invalid stream configuration length"); break;
                }
                // Discard previous-generation fragments and require a new keyframe.
                std::lock_guard<std::mutex> configLock(m_reasmMutex);
                m_assemblySlots.clear();
                m_haveDeliveredSeq = false;
                m_waitingForKeyframe = true;
                m_lastKeyframeRequestUs = 0;
                m_pendingVideoPose={};
                const bool ok = config.codec <= 2 && config.width > 0 && config.height > 0
                    && config.width <= 8192 && config.height <= 8192
                    && (!(config.flags & 1) || config.width % 2 == 0)
                    && m_decoder && m_decoder->configureStream(static_cast<VideoCodecType>(config.codec), config.width, config.height);
                if (!sendTcpMessage(MessageType::STREAM_CONFIG_ACK, config.ack(ok)) || !ok) break;
                m_isStereoStream = (config.flags & 1) != 0;
                m_streamConfigured = true;
                if (m_onVrModeChanged) m_onVrModeChanged(m_isStereoStream.load());
                LOGI("Stream %ux%u SBS=%d generation=%u", config.width, config.height, int(m_isStereoStream.load()), config.generation);
            } else if (msg.message_type == 110 && msg.payload.size()==8) {
                const auto received=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now().time_since_epoch()).count());
                auto reply=msg.payload;reply.resize(24,0);
                for(int i=0;i<8;++i)reply[8+i]=static_cast<uint8_t>(received>>(i*8));
                if(!sendTcpMessage(static_cast<MessageType>(111),reply))break;
            } else if (msg.message_type == 104) {
                LOGW("Server status: %.*s", static_cast<int>(msg.payload.size()),
                     reinterpret_cast<const char*>(msg.payload.data()));
            } else if (msg.message_type == 106) {
                if(!VideoFramePose::parse(msg.payload,m_pendingVideoPose)) m_pendingVideoPose={};
            } else if (msg.message_type == 101) {
                VideoFramePose pose=m_pendingVideoPose;m_pendingVideoPose={};
                const uint64_t pts=nowUs();if(pose.valid)pose.ptsUs=pts;
                dispatchVideoFrame(msg.payload.data(), msg.payload.size(), pts, pose.valid?&pose:nullptr);
            } else if (msg.message_type == static_cast<uint32_t>(MessageType::SHOW_DESKTOP)) {
                bool dropIntoVr = (!msg.payload.empty() && msg.payload[0] == 0);
                LOGI("Received SHOW_DESKTOP from server: dropIntoVr=%d", dropIntoVr);
                if (m_onVrModeChanged) {
                    m_onVrModeChanged(dropIntoVr);
                }
            } else {
                LOGI("Received NetMessage: type=%u, payload=%zu bytes", msg.message_type, msg.payload.size());
            }
        }
    }

    m_connected = false;
}

void NetworkClient::runUdpLoop() {
    std::vector<uint8_t> packetBuf(65536);
    sockaddr_in fromAddr{};
    socklen_t fromLen = sizeof(fromAddr);

    while (m_running && m_connected) {
        ssize_t r = recvfrom(m_udpSocket, packetBuf.data(), packetBuf.size(), 0,
                             (struct sockaddr*)&fromAddr, &fromLen);
        if (r < 0) {
            if (!m_running) break;
            continue;
        }

        if (r >= (ssize_t)sizeof(UdpHeader)) {
            handleUdpDatagram(packetBuf.data(), (size_t)r);
        }
    }
}

void NetworkClient::handleUdpDatagram(const uint8_t* data, size_t size) {
    if (size < 13 || !m_streamConfigured) return;
    // Check "ORVD" magic
    if (std::memcmp(data, UDP_MAGIC, 4) != 0) {
        return; // Ignore foreign packet
    }

    uint8_t channel = data[4];
    uint16_t fragIdx = (static_cast<uint16_t>(data[5]) << 8) | data[6];
    uint16_t totalFrags = (static_cast<uint16_t>(data[7]) << 8) | data[8];
    uint32_t seq = (static_cast<uint32_t>(data[9]) << 24) |
                   (static_cast<uint32_t>(data[10]) << 16) |
                   (static_cast<uint32_t>(data[11]) << 8) |
                   data[12];

    const uint8_t* payload = data + 13;
    size_t payloadSize = size - 13;
    if (totalFrags == 0 || totalFrags > MAX_FRAGMENTS || fragIdx >= totalFrags || payloadSize == 0 || payloadSize > 1187) return;

    if (channel == CHANNEL_AUDIO) {
        // Audio fast delivery directly to player
        if (m_audio && payloadSize >= sizeof(int16_t)) {
            m_audio->enqueuePcmSamples(reinterpret_cast<const int16_t*>(payload),
                                       payloadSize / sizeof(int16_t));
        }
        return;
    }

    if (channel == CHANNEL_VIDEO) {
        // SBS arrives exclusively over framed TCP101, ordered after its config.
        if (m_isStereoStream.load()) return;
        std::lock_guard<std::mutex> lock(m_reasmMutex);
        // Recheck after acquiring the config/reset mutex: mode may have changed
        // while this old desktop UDP datagram was waiting for the lock.
        if (!m_streamConfigured.load() || m_isStereoStream.load()) return;

        // Drop frames older than latest delivered
        if (m_haveDeliveredSeq && !newer(seq, m_lastDeliveredSeq)) {
            return;
        }

        const uint64_t now = nowUs();
        for (auto it = m_assemblySlots.begin(); it != m_assemblySlots.end();) {
            if (now - it->second.timestampUs > 200000) {
                it = m_assemblySlots.erase(it);
                m_waitingForKeyframe = true;
                requestKeyframe();
            } else ++it;
        }
        if (m_assemblySlots.find(seq) == m_assemblySlots.end() && m_assemblySlots.size() >= MAX_ASSEMBLY_SLOTS) {
            m_assemblySlots.clear();
            m_waitingForKeyframe = true;
            requestKeyframe();
        }

        auto& slot = m_assemblySlots[seq];
        if (slot.totalFragments == 0) {
            slot.sequence = seq;
            slot.totalFragments = totalFrags;
            slot.receivedFragments = 0;
            slot.fragments.resize(totalFrags);
            slot.timestampUs = std::chrono::duration_cast<std::chrono::microseconds>(
                std::chrono::steady_clock::now().time_since_epoch()).count();
        }
        if (slot.totalFragments != totalFrags) {
            m_assemblySlots.erase(seq);
            return;
        }

        if (fragIdx < totalFrags && slot.fragments[fragIdx].empty()) {
            slot.fragments[fragIdx].assign(payload, payload + payloadSize);
            slot.receivedFragments++;

            if (slot.receivedFragments == totalFrags) {
                if (m_haveDeliveredSeq && seq != m_lastDeliveredSeq + 1) {
                    m_waitingForKeyframe = true;
                    requestKeyframe();
                }
                assembleAndDispatchFrame(slot);
                m_lastDeliveredSeq = seq;
                m_haveDeliveredSeq = true;
                m_assemblySlots.erase(seq);

                // Prune older unfinished slots
                for (auto it = m_assemblySlots.begin(); it != m_assemblySlots.end();) {
                    if (!newer(it->first, seq)) {
                        it = m_assemblySlots.erase(it);
                    } else {
                        ++it;
                    }
                }
            }
        }
    }
}

void NetworkClient::assembleAndDispatchFrame(FrameAssemblySlot& slot) {
    size_t totalBytes = 0;
    for (const auto& frag : slot.fragments) {
        totalBytes += frag.size();
    }
    if (totalBytes > MAX_VIDEO_BYTES) { requestKeyframe(); return; }

    std::vector<uint8_t> fullFrame;
    fullFrame.reserve(totalBytes);
    for (const auto& frag : slot.fragments) {
        fullFrame.insert(fullFrame.end(), frag.begin(), frag.end());
    }

    dispatchVideoFrame(fullFrame.data(), fullFrame.size(), slot.timestampUs);
}

void NetworkClient::requestKeyframe() {
    const uint64_t now = nowUs();
    if (now - m_lastKeyframeRequestUs > 500000) {
        m_lastKeyframeRequestUs = now;
        sendTcpMessage(MessageType::FORCE_KEY_FRAME, {});
    }
}

void NetworkClient::dispatchVideoFrame(const uint8_t* data, size_t size, uint64_t ptsUs, const VideoFramePose* pose) {
    if (!m_streamConfigured || !m_decoder || size == 0 || size > MAX_VIDEO_BYTES) return;
    bool keyframe = false;
    const auto codec = m_decoder->getCodecType();
    // Annex B permits both three- and four-byte start codes.
    for (size_t i = 0; i + 3 < size; ++i) {
        if (data[i] != 0 || data[i+1] != 0) continue;
        size_t header = 0;
        if (data[i+2] == 1) header = i + 3;
        else if (i + 4 < size && data[i+2] == 0 && data[i+3] == 1) header = i + 4;
        if (!header || header >= size) continue;
        if (codec == VideoCodecType::H264) keyframe |= (data[header] & 0x1f) == 5;
        else if (codec == VideoCodecType::HEVC) {
            const uint8_t nal = (data[header] >> 1) & 0x3f;
            keyframe |= nal >= 16 && nal <= 21;
        }
    }
    if (m_waitingForKeyframe && !keyframe) { requestKeyframe(); return; }
    if (!m_decoder->decodeFrame(data, size, ptsUs, keyframe, pose)) {
        m_waitingForKeyframe = true;
        requestKeyframe();
    } else if (keyframe) {
        m_waitingForKeyframe = false;
    }
}
} // namespace openreality
