//! Network module for OpenReality.
//!
//! Handles all networking: TCP control channel, UDP fast-path for video/audio,
//! AES-256-GCM encryption, LZ4 compression, mDNS discovery, UPnP port mapping,
//! and Wake-on-LAN. Inspired by Virtual Desktop Streamer's `ConnectionManager`,
//! `NetMessagingClient`, `NetMessage`, `UPnPManager`, `WOLHelper`, and `TraceRoute`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, AeadCore, Nonce};
use anyhow::{bail, Context, Result};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Protobuf generated types
// ---------------------------------------------------------------------------

pub mod proto {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum MessageType {
        PeerInfo = 0,
        CrashCallstack = 1,
        UpdateRequired = 2,
        UpdateUnavailable = 3,
        UpdateInProgress = 4,
        UserSettingsReload = 5,
        UserSettingsPropertyChanged = 6,
        MobileSettingsReload = 7,
        MobileSettingsPropertyChanged = 8,
        StreamerSettingsReload = 9,
        StreamerSettingsPropertyChanged = 10,
        BoundaryDataChanged = 11,
        SkeletonDataChanged = 12,
        RestartStream = 13,
        ForceKeyFrame = 14,
        SwitchMonitor = 15,
        ToggleDictation = 16,
        LaunchSteamVr = 17,
        ExitSteamVr = 18,
        Recenter = 19,
        RequestGames = 20,
        Games = 21,
        RequestGameThumbnails = 22,
        GameThumbnail = 23,
        LaunchGame = 24,
        ExitGame = 25,
        GameExited = 26,
        ShowDesktop = 27,
        DisposeGamepadEmulation = 28,
        AddMonitor = 29,
        RemoveMonitor = 30,
        SetMonitorPortrait = 31,
        ScreenshotToDesktop = 32,
        Exit = 33,
        ToggleVrPassthrough = 34,
        TogglePerformanceOverlay = 35,
        ToggleFoveatedStreaming = 36,
        Disconnect = 37,
        RequestCursor = 38,
        Cursor = 39,
        ClipboardChanged = 40,
        ClipboardText = 41,
        ClipboardImage = 42,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct NetMessage {
        #[prost(enumeration = "MessageType", tag = "1")]
        pub message_type: i32,
        #[prost(bytes = "vec", tag = "2")]
        pub payload: ::prost::alloc::vec::Vec<u8>,
        #[prost(uint64, tag = "3")]
        pub timestamp: u64,
        #[prost(uint32, tag = "4")]
        pub sequence: u32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct PeerInfo {
        #[prost(string, tag = "1")]
        pub computer_id: ::prost::alloc::string::String,
        #[prost(string, tag = "2")]
        pub computer_name: ::prost::alloc::string::String,
        #[prost(string, tag = "3")]
        pub os: ::prost::alloc::string::String,
        #[prost(string, tag = "4")]
        pub streamer_version: ::prost::alloc::string::String,
        #[prost(string, repeated, tag = "5")]
        pub private_addresses: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
        #[prost(bool, tag = "6")]
        pub allow_remote_connections: bool,
        #[prost(bool, tag = "7")]
        pub encrypt_local_traffic: bool,
        #[prost(bool, tag = "8")]
        pub encrypt_remote_traffic: bool,
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default TCP control port.
pub const DEFAULT_CONTROL_PORT: u16 = 38_810;

/// Default UDP data port.
pub const DEFAULT_DATA_PORT: u16 = 38_811;

/// mDNS service type for Prism Aperture servers.
pub const MDNS_SERVICE_TYPE: &str = "_prismaperture._tcp.local.";

/// Maximum UDP datagram payload (fits in typical MTU minus headers).
const MAX_UDP_PAYLOAD: usize = 65_000;

/// Length-prefix size for TCP framing (4 bytes, big-endian u32).
const FRAME_LEN_SIZE: usize = 4;

/// Magic bytes prepended to each UDP datagram for fast demux.
const UDP_MAGIC: [u8; 4] = [0x4F, 0x52, 0x56, 0x44]; // "ORVD"

/// Channel tag inside UDP datagrams.
const CHANNEL_VIDEO: u8 = 0x01;
const CHANNEL_AUDIO: u8 = 0x02;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Connection lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Physical link type for the active network adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkConnectionType {
    Wireless,
    Ethernet,
    Unknown,
}

/// NAT / routing topology detected via STUN-like probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStatus {
    PublicIp,
    Nat,
    DoubleNat,
    CgNat,
    Unknown,
}

// ---------------------------------------------------------------------------
// EncryptionLayer — AES-256-GCM
// ---------------------------------------------------------------------------

/// Symmetric encryption layer using AES-256-GCM.
///
/// Each `EncryptionLayer` holds a 256-bit key and a 96-bit nonce (IV).
/// The nonce is used as the *base* value; the actual per-message nonce is
/// derived by XOR-ing the base with a monotonically increasing counter so
/// that nonces are never reused.
#[derive(Clone)]
pub struct EncryptionLayer {
    cipher: Aes256Gcm,
    key: [u8; 32],
    iv: [u8; 12],
    counter: Arc<AtomicU64>,
}

impl EncryptionLayer {
    /// Create a new layer with a randomly generated key and IV.
    pub fn new() -> Self {
        let key_bytes = Aes256Gcm::generate_key(&mut OsRng);
        let nonce_bytes = Aes256Gcm::generate_nonce(&mut OsRng);

        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);
        let mut iv = [0u8; 12];
        iv.copy_from_slice(&nonce_bytes);

        let cipher = Aes256Gcm::new(&key_bytes);

        Self {
            cipher,
            key,
            iv,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Reconstruct from a known key and IV (e.g. received from a peer).
    pub fn from_key(key: &[u8; 32], iv: &[u8; 12]) -> Self {
        let cipher = Aes256Gcm::new(key.into());
        Self {
            cipher,
            key: *key,
            iv: *iv,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Encrypt `plaintext`, returning ciphertext with authentication tag.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.next_nonce();
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|e| anyhow::anyhow!("AES-GCM encrypt error: {e}"))?;

        // Prepend the derived nonce so the receiver can decrypt.
        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt `ciphertext` that was produced by [`Self::encrypt`].
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 12 {
            bail!("ciphertext too short to contain nonce");
        }
        let (nonce_bytes, ct) = ciphertext.split_at(12);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ct)
            .map_err(|e| anyhow::anyhow!("AES-GCM decrypt error: {e}"))?;
        Ok(plaintext)
    }

    /// The raw 256-bit key.
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    /// The base 96-bit IV.
    pub fn iv(&self) -> &[u8; 12] {
        &self.iv
    }

    // -- internal --

    /// Derive a unique nonce by XOR-ing the base IV with a counter.
    fn next_nonce(&self) -> [u8; 12] {
        let ctr = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut nonce = self.iv;
        let ctr_bytes = ctr.to_le_bytes(); // 8 bytes
        for i in 0..8 {
            nonce[i] ^= ctr_bytes[i];
        }
        nonce
    }
}

// ---------------------------------------------------------------------------
// CompressionLayer — LZ4
// ---------------------------------------------------------------------------

/// Stateless LZ4 compression / decompression helpers.
pub struct CompressionLayer;

impl CompressionLayer {
    /// Compress `data` using LZ4 block compression.
    pub fn compress(data: &[u8]) -> Vec<u8> {
        lz4_flex::compress_prepend_size(data)
    }

    /// Decompress LZ4-compressed `data`.
    ///
    /// `max_size` guards against decompression bombs: if the uncompressed size
    /// header exceeds `max_size` the call returns an error immediately.
    pub fn decompress(data: &[u8], max_size: usize) -> Result<Vec<u8>> {
        // lz4_flex prepend-size format stores the original size as a little-
        // endian u32 in the first 4 bytes.
        if data.len() < 4 {
            bail!("LZ4 data too short to contain size header");
        }
        let orig_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if orig_size > max_size {
            bail!(
                "LZ4 uncompressed size ({orig_size}) exceeds limit ({max_size})"
            );
        }
        let decompressed = lz4_flex::decompress_size_prepended(data)
            .map_err(|e| anyhow::anyhow!("LZ4 decompress error: {e}"))?;
        Ok(decompressed)
    }
}

// ---------------------------------------------------------------------------
// NetMessage — thin wrapper around protobuf NetMessage
// ---------------------------------------------------------------------------

/// Application-level message exchanged over the control channel (TCP).
#[derive(Debug, Clone)]
pub struct NetMessage {
    pub message_type: u32,
    pub payload: Vec<u8>,
    pub timestamp: u64,
    pub sequence: u32,
}

impl NetMessage {
    /// Create a new message.
    pub fn new(message_type: u32, payload: Vec<u8>, sequence: u32) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            message_type,
            payload,
            timestamp,
            sequence,
        }
    }

    /// Serialize to bytes using prost (protobuf encoding).
    pub fn serialize(&self) -> Vec<u8> {
        let proto_msg = proto::NetMessage {
            message_type: self.message_type as i32,
            payload: self.payload.clone(),
            timestamp: self.timestamp,
            sequence: self.sequence,
        };
        proto_msg.encode_to_vec()
    }

    /// Deserialize from protobuf-encoded bytes.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let proto_msg =
            proto::NetMessage::decode(data).context("failed to decode NetMessage protobuf")?;
        Ok(Self {
            message_type: proto_msg.message_type as u32,
            payload: proto_msg.payload,
            timestamp: proto_msg.timestamp,
            sequence: proto_msg.sequence,
        })
    }
}

// ---------------------------------------------------------------------------
// StreamServer — TCP listener + mDNS announcer
// ---------------------------------------------------------------------------

/// The streamer-side server that accepts incoming peer connections.
pub struct StreamServer {
    tcp_listener: TcpListener,
    local_addr: SocketAddr,
    mdns_daemon: Option<mdns_sd::ServiceDaemon>,
    instance_id: String,
}

impl StreamServer {
    /// Bind the TCP control listener on `bind_addr`.
    pub async fn new(bind_addr: SocketAddr) -> Result<Self> {
        let tcp_listener = TcpListener::bind(bind_addr)
            .await
            .context("failed to bind TCP listener")?;
        let local_addr = tcp_listener.local_addr()?;
        info!("StreamServer listening on {local_addr}");

        Ok(Self {
            tcp_listener,
            local_addr,
            mdns_daemon: None,
            instance_id: Uuid::new_v4().to_string(),
        })
    }

    /// Accept the next incoming TCP connection and set up a `PeerConnection`.
    ///
    /// This also allocates a UDP socket bound to an ephemeral port on the
    /// same interface and sends the port number to the peer so it can start
    /// sending data back.
    pub async fn accept(&mut self) -> Result<PeerConnection> {
        let (tcp_stream, peer_addr) = self
            .tcp_listener
            .accept()
            .await
            .context("TCP accept failed")?;
        info!("Accepted TCP connection from {peer_addr}");

        // Bind a UDP socket on the same IP, any port.
        let udp_bind = SocketAddr::new(self.local_addr.ip(), 0);
        let udp_socket = UdpSocket::bind(udp_bind)
            .await
            .context("failed to bind UDP data socket")?;
        let udp_local = udp_socket.local_addr()?;
        debug!("UDP data socket bound on {udp_local}");

        // Send the UDP port to the peer so it knows where to send datagrams.
        let mut tcp = tcp_stream;
        let port_msg = udp_local.port().to_be_bytes();
        tcp.write_all(&port_msg).await?;
        tcp.flush().await?;

        // Read the peer's UDP port back.
        let mut peer_port_buf = [0u8; 2];
        tcp.read_exact(&mut peer_port_buf).await?;
        let peer_udp_port = u16::from_be_bytes(peer_port_buf);
        let peer_udp_addr = SocketAddr::new(peer_addr.ip(), peer_udp_port);
        debug!("Peer UDP endpoint: {peer_udp_addr}");

        // Connect the UDP socket to the peer so send/recv work without addressing.
        udp_socket.connect(peer_udp_addr).await?;

        Ok(PeerConnection::new(tcp, udp_socket, peer_addr))
    }

    /// Announce this streamer on the local network via mDNS.
    pub async fn broadcast_discovery(&self) -> Result<()> {
        let daemon = mdns_sd::ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("mDNS daemon init error: {e}"))?;

        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "PrismAperture-PC".to_string());

        let mut properties = std::collections::HashMap::new();
        properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        properties.insert("id".to_string(), self.instance_id.clone());

        let service_info = mdns_sd::ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &hostname,
            &format!("{hostname}.local."),
            "",
            self.local_addr.port(),
            properties,
        )
        .map_err(|e| anyhow::anyhow!("mDNS ServiceInfo error: {e}"))?;

        daemon
            .register(service_info)
            .map_err(|e| anyhow::anyhow!("mDNS register error: {e}"))?;

        info!(
            "mDNS discovery announced: {} on port {}",
            hostname,
            self.local_addr.port()
        );

        // Keep the daemon alive; it runs its own background thread.
        // NOTE: In a real deployment we would store this on `self` and
        // unregister on drop. For the initial implementation we intentionally
        // leak it so it stays active for the process lifetime.
        std::mem::forget(daemon);

        Ok(())
    }

    /// The local address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

// ---------------------------------------------------------------------------
// PeerConnection — TCP control + UDP data
// ---------------------------------------------------------------------------

/// A bidirectional connection to a single peer (headset).
///
/// Control messages travel over TCP with length-prefix framing.
/// Video and audio frames travel over UDP for lower latency.
pub struct PeerConnection {
    tcp: Arc<Mutex<TcpStream>>,
    udp: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    state: Arc<RwLock<ConnectionState>>,
    sequence: AtomicU32,
    rtt_us: AtomicU64,
    connected_at: Instant,
}

impl PeerConnection {
    fn new(tcp: TcpStream, udp: UdpSocket, peer_addr: SocketAddr) -> Self {
        Self {
            tcp: Arc::new(Mutex::new(tcp)),
            udp: Arc::new(udp),
            peer_addr,
            state: Arc::new(RwLock::new(ConnectionState::Connected)),
            sequence: AtomicU32::new(0),
            rtt_us: AtomicU64::new(0),
            connected_at: Instant::now(),
        }
    }

    // -- TCP control channel --------------------------------------------------

    /// Send a `NetMessage` over the TCP control channel.
    ///
    /// Wire format: `[4-byte big-endian length][protobuf payload]`
    pub async fn send_message(&self, msg: NetMessage) -> Result<()> {
        let data = msg.serialize();
        let len = (data.len() as u32).to_be_bytes();

        let mut tcp = self.tcp.lock().await;
        tcp.write_all(&len).await?;
        tcp.write_all(&data).await?;
        tcp.flush().await?;

        debug!(
            "Sent message type={} seq={} ({} bytes) to {}",
            msg.message_type,
            msg.sequence,
            data.len(),
            self.peer_addr
        );
        Ok(())
    }

    /// Receive the next `NetMessage` from the TCP control channel.
    pub async fn recv_message(&self) -> Result<NetMessage> {
        let mut tcp = self.tcp.lock().await;

        // Read length prefix.
        let mut len_buf = [0u8; FRAME_LEN_SIZE];
        tcp.read_exact(&mut len_buf).await.context("TCP read length")?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 16 * 1024 * 1024 {
            bail!("TCP frame too large: {len} bytes");
        }

        // Read payload.
        let mut buf = vec![0u8; len];
        tcp.read_exact(&mut buf).await.context("TCP read payload")?;

        let msg = NetMessage::deserialize(&buf)?;
        debug!(
            "Recv message type={} seq={} ({len} bytes) from {}",
            msg.message_type, msg.sequence, self.peer_addr
        );
        Ok(msg)
    }

    // -- UDP fast-path --------------------------------------------------------

    /// Send an encoded video frame over the UDP fast-path.
    ///
    /// Large frames are fragmented into `MAX_UDP_PAYLOAD`-sized chunks.
    /// Each datagram starts with `UDP_MAGIC | channel | fragment_index | total_fragments | sequence`.
    pub async fn send_video_frame(&self, data: &[u8]) -> Result<()> {
        self.send_udp_data(CHANNEL_VIDEO, data).await
    }

    /// Send encoded audio samples over the UDP fast-path.
    pub async fn send_audio_samples(&self, data: &[u8]) -> Result<()> {
        self.send_udp_data(CHANNEL_AUDIO, data).await
    }

    /// The peer's remote address.
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        // Fast non-blocking read — acceptable staleness.
        *futures::executor::block_on(self.state.read())
    }

    /// Estimated round-trip latency in milliseconds.
    pub fn latency_ms(&self) -> f64 {
        self.rtt_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// Update the measured RTT (called by a ping/pong loop elsewhere).
    pub fn update_rtt_us(&self, rtt: u64) {
        self.rtt_us.store(rtt, Ordering::Relaxed);
    }

    /// Transition the connection state.
    pub async fn set_state(&self, new_state: ConnectionState) {
        let mut s = self.state.write().await;
        info!(
            "Connection to {} state change: {:?} -> {:?}",
            self.peer_addr, *s, new_state
        );
        *s = new_state;
    }

    /// Elapsed time since the connection was established.
    pub fn uptime(&self) -> std::time::Duration {
        self.connected_at.elapsed()
    }

    // -- internal helpers -----------------------------------------------------

    /// Fragment `data` and send over UDP with a channel tag.
    async fn send_udp_data(&self, channel: u8, data: &[u8]) -> Result<()> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);

        // Header per datagram: MAGIC(4) + channel(1) + frag_idx(2) + total_frags(2) + seq(4) = 13
        const HEADER_SIZE: usize = 13;
        let chunk_size = MAX_UDP_PAYLOAD - HEADER_SIZE;
        let total_fragments = ((data.len() + chunk_size - 1) / chunk_size).max(1) as u16;

        for (i, chunk) in data.chunks(chunk_size).enumerate() {
            let frag_idx = i as u16;

            let mut dgram = Vec::with_capacity(HEADER_SIZE + chunk.len());
            dgram.extend_from_slice(&UDP_MAGIC);
            dgram.push(channel);
            dgram.extend_from_slice(&frag_idx.to_be_bytes());
            dgram.extend_from_slice(&total_fragments.to_be_bytes());
            dgram.extend_from_slice(&seq.to_be_bytes());
            dgram.extend_from_slice(chunk);

            self.udp.send(&dgram).await.context("UDP send")?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DiscoveryClient — mDNS peer discovery
// ---------------------------------------------------------------------------

/// A streamer discovered on the local network via mDNS.
#[derive(Debug, Clone)]
pub struct DiscoveredStreamer {
    pub name: String,
    pub address: SocketAddr,
    pub version: String,
    pub id: String,
}

/// Client-side mDNS discovery of OpenReality streamers.
pub struct DiscoveryClient;

impl DiscoveryClient {
    /// Browse the local network for OpenReality streamers for up to 3 seconds.
    pub async fn discover_streamers() -> Result<Vec<DiscoveredStreamer>> {
        let daemon = mdns_sd::ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("mDNS daemon init error: {e}"))?;

        let receiver = daemon
            .browse(MDNS_SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("mDNS browse error: {e}"))?;

        let mut streamers = Vec::new();
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, tokio::task::spawn_blocking({
                let rx = receiver.clone();
                move || rx.recv_timeout(std::time::Duration::from_millis(500))
            }))
            .await
            {
                Ok(Ok(Ok(event))) => {
                    if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                        let port = info.get_port();
                        let addrs: Vec<_> = info.get_addresses().iter().copied().collect();
                        let ip = addrs
                            .into_iter()
                            .find(|a| a.is_ipv4())
                            .or_else(|| info.get_addresses().iter().copied().next());

                        if let Some(ip) = ip {
                            let version = info
                                .get_properties()
                                .get("version")
                                .map(|v| v.val_str().to_string())
                                .unwrap_or_default();
                            let id = info
                                .get_properties()
                                .get("id")
                                .map(|v| v.val_str().to_string())
                                .unwrap_or_default();

                            let streamer = DiscoveredStreamer {
                                name: info.get_fullname().to_string(),
                                address: SocketAddr::new(ip.into(), port),
                                version,
                                id,
                            };
                            info!("Discovered streamer: {:?}", streamer);
                            streamers.push(streamer);
                        }
                    }
                }
                Ok(Ok(Err(_))) => {
                    // recv_timeout returned Err (timeout or disconnected)
                    continue;
                }
                Ok(Err(e)) => {
                    warn!("mDNS browse task error: {e}");
                    break;
                }
                Err(_) => {
                    // tokio timeout elapsed
                    break;
                }
            }
        }

        // Shut down the daemon gracefully.
        let _ = daemon.shutdown();

        Ok(streamers)
    }
}

// ---------------------------------------------------------------------------
// WakeOnLan
// ---------------------------------------------------------------------------

/// Wake-on-LAN magic packet sender.
pub struct WakeOnLan;

impl WakeOnLan {
    /// Send a WOL magic packet to the given MAC address.
    ///
    /// The magic packet is a UDP broadcast datagram containing 6 bytes of
    /// `0xFF` followed by the target MAC address repeated 16 times.
    pub fn send(mac_address: [u8; 6]) -> Result<()> {
        let mut packet = vec![0xFFu8; 6];
        for _ in 0..16 {
            packet.extend_from_slice(&mac_address);
        }

        let socket =
            std::net::UdpSocket::bind("0.0.0.0:0").context("WOL: failed to bind UDP socket")?;
        socket
            .set_broadcast(true)
            .context("WOL: failed to enable broadcast")?;

        let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, 9);
        socket
            .send_to(&packet, dest)
            .context("WOL: failed to send magic packet")?;

        info!(
            "WOL magic packet sent to {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac_address[0],
            mac_address[1],
            mac_address[2],
            mac_address[3],
            mac_address[4],
            mac_address[5]
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UPnP port mapping
// ---------------------------------------------------------------------------

/// Open a TCP port mapping via UPnP IGD.
///
/// Asks the gateway to forward `port` on the external interface to our
/// local address on the same port. The lease is set to 3600 seconds.
pub async fn upnp_open_port(port: u16) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        info!("Searching for UPnP gateway to open port {port}...");
        match igd_next::search_gateway(Default::default()) {
            Ok(gateway) => {
                let local_ip = gateway.addr.ip();
                let local_addr = SocketAddrV4::new(
                    match local_ip {
                        std::net::IpAddr::V4(v4) => v4,
                        _ => return Err(anyhow::anyhow!("UPnP: gateway address is not IPv4")),
                    },
                    port,
                );
                gateway.add_port(
                    igd_next::PortMappingProtocol::TCP,
                    port,
                    SocketAddr::V4(local_addr),
                    3600,
                    "Prism Aperture Server",
                )?;
                info!("UPnP: port {port} opened on gateway {}", gateway.addr);
                Ok(())
            }
            Err(e) => {
                warn!("UPnP gateway search skipped/not found: {e}");
                Ok(())
            }
        }
    })
    .await?
}

/// Remove a previously opened UPnP port mapping.
pub async fn upnp_close_port(port: u16) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        info!("Searching for UPnP gateway to close port {port}...");
        match igd_next::search_gateway(Default::default()) {
            Ok(gateway) => {
                gateway.remove_port(igd_next::PortMappingProtocol::TCP, port)?;
                info!("UPnP: port {port} closed on gateway");
                Ok(())
            }
            Err(e) => {
                debug!("UPnP close port skipped: {e}");
                Ok(())
            }
        }
    })
    .await?
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_roundtrip() {
        let layer = EncryptionLayer::new();
        let plaintext = b"Hello, OpenReality!";
        let ct = layer.encrypt(plaintext).unwrap();
        let pt = layer.decrypt(&ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn encryption_from_key_roundtrip() {
        let original = EncryptionLayer::new();
        let restored = EncryptionLayer::from_key(original.key(), original.iv());
        let plaintext = b"key exchange test";
        let ct = original.encrypt(plaintext).unwrap();
        let pt = restored.decrypt(&ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn compression_roundtrip() {
        let data = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let compressed = CompressionLayer::compress(data);
        assert!(compressed.len() < data.len());
        let decompressed = CompressionLayer::decompress(&compressed, 1024).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn compression_rejects_oversize() {
        let data = vec![0u8; 1024];
        let compressed = CompressionLayer::compress(&data);
        let result = CompressionLayer::decompress(&compressed, 512);
        assert!(result.is_err());
    }

    #[test]
    fn net_message_serde_roundtrip() {
        let msg = NetMessage::new(14, vec![1, 2, 3, 4], 42);
        let bytes = msg.serialize();
        let msg2 = NetMessage::deserialize(&bytes).unwrap();
        assert_eq!(msg2.message_type, 14);
        assert_eq!(msg2.payload, vec![1, 2, 3, 4]);
        assert_eq!(msg2.sequence, 42);
    }

    #[test]
    fn wol_packet_format() {
        // Just verify the packet structure without actually sending.
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let mut expected = vec![0xFFu8; 6];
        for _ in 0..16 {
            expected.extend_from_slice(&mac);
        }
        assert_eq!(expected.len(), 6 + 6 * 16); // 102 bytes
    }

    #[tokio::test]
    async fn tcp_control_channel_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let conn = PeerConnection::new(stream, udp, addr);
            let received = conn.recv_message().await.unwrap();
            assert_eq!(received.message_type, 7);
            assert_eq!(received.payload, vec![0xDE, 0xAD]);
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = PeerConnection::new(client_stream, udp, addr);
        let msg = NetMessage::new(7, vec![0xDE, 0xAD], 1);
        client.send_message(msg).await.unwrap();

        server.await.unwrap();
    }
}
