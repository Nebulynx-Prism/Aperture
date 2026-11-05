//! Streaming session coordinator for OpenReality.
//!
//! Orchestrates the end-to-end communication pipeline:
//! 1. OpenReality Service IPC (Named Pipe): Priority boost & power throttling
//! 2. Discovery: mDNS announcement + UDP broadcast discovery responder
//! 3. Connection lifecycle: TCP control + UDP data streaming
//! 4. Video & Audio streaming: Desktop capture + hardware encode + WASAPI loopback
//! 5. Input injection: Translating VR laser pointer & controllers to Windows desktop inputs

use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::audio::AudioCapture;
use crate::capture::DesktopCapture;
use crate::encoder::{EncoderConfig, QualityPreset, VideoCodec, VideoEncoder};
use crate::input::{InputInjector, MouseButton};
use crate::network::proto::{self, MessageType};
use crate::network::{NetMessage, PeerConnection, StreamServer, DEFAULT_CONTROL_PORT};
use crate::service_client::ServiceClient;
use crate::settings::StreamerSettings;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Real-time streaming metrics shared between background session and UI
pub struct StreamTelemetry {
    pub is_streaming: AtomicBool,
    pub service_connected: AtomicBool,
    pub connected_clients: AtomicU32,
    pub current_fps: RwLock<f32>,
    pub current_bitrate_mbps: RwLock<f32>,
    pub current_latency_ms: RwLock<f32>,
}

impl StreamTelemetry {
    pub fn new() -> Self {
        Self {
            is_streaming: AtomicBool::new(true),
            service_connected: AtomicBool::new(false),
            connected_clients: AtomicU32::new(0),
            current_fps: RwLock::new(0.0),
            current_bitrate_mbps: RwLock::new(0.0),
            current_latency_ms: RwLock::new(0.0),
        }
    }
}

pub struct SessionHandle {
    pub telemetry: Arc<StreamTelemetry>,
    shutdown_flag: Arc<AtomicBool>,
}

impl SessionHandle {
    pub fn stop(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
    }
}

/// Start the background streaming orchestrator
pub fn start_session(settings: StreamerSettings) -> SessionHandle {
    let telemetry = Arc::new(StreamTelemetry::new());
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    let telem_clone = Arc::clone(&telemetry);
    let shut_clone = Arc::clone(&shutdown_flag);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to create Tokio runtime for streaming session: {e}");
                return;
            }
        };

        rt.block_on(async move {
            run_session_orchestrator(settings, telem_clone, shut_clone).await;
        });
    });

    SessionHandle {
        telemetry,
        shutdown_flag,
    }
}

async fn run_session_orchestrator(
    settings: StreamerSettings,
    telemetry: Arc<StreamTelemetry>,
    shutdown: Arc<AtomicBool>,
) {
    info!("Starting Prism Aperture Server Orchestrator...");

    // 1. Service IPC integration: Ping & boost priority
    match ServiceClient::ping() {
        Ok(true) => {
            info!("Prism Aperture Service is RUNNING on named pipe");
            telemetry.service_connected.store(true, Ordering::SeqCst);

            let pid = std::process::id();
            if let Ok(msg) = ServiceClient::boost_priority(pid) {
                info!("Process priority boosted: {msg}");
            }
            if let Ok(msg) = ServiceClient::disable_throttling(pid) {
                info!("Power throttling disabled: {msg}");
            }
        }
        _ => {
            warn!("Prism Aperture Service is not running. Running in standalone mode.");
            telemetry.service_connected.store(false, Ordering::SeqCst);
        }
    }

    // 2. Start UDP Discovery Responder (answering "OR_DISCOVER" from Quest headsets)
    let shut_discover = Arc::clone(&shutdown);
    tokio::spawn(async move {
        run_udp_discovery_responder(shut_discover).await;
    });

    // 3. Start TCP StreamServer on 0.0.0.0:38810
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_CONTROL_PORT));
    let mut server = match StreamServer::new(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to bind StreamServer on {bind_addr}: {e}");
            return;
        }
    };

    // Broadcast mDNS
    if let Err(e) = server.broadcast_discovery().await {
        warn!("mDNS broadcast warning: {e}");
    }

    // Auto-setup USB wired forwarding & wireless TCPIP via ADB (ALVR-style link)
    #[cfg(windows)]
    tokio::spawn(async {
        // Configure TCPIP mode on Quest for wireless ADB
        let _ = std::process::Command::new("adb")
            .args(["tcpip", "5555"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .status();

        // Configure reverse TCP tunnels for wired USB link
        let _ = std::process::Command::new("adb")
            .args(["reverse", "tcp:38810", "tcp:38810"])
            .creation_flags(0x08000000)
            .status();
        let _ = std::process::Command::new("adb")
            .args(["reverse", "tcp:38811", "tcp:38811"])
            .creation_flags(0x08000000)
            .status();
        info!("Configured ADB TCPIP mode (5555) and USB wired reverse forwarding on ports 38810/38811");

        // Attempt Windows Firewall rules (if elevated)
        let _ = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "add", "rule", "name=Prism Aperture Server TCP", "dir=in", "action=allow", "protocol=TCP", "localport=38810,38811,5555", "enable=yes"])
            .creation_flags(0x08000000)
            .status();
        let _ = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "add", "rule", "name=Prism Aperture Server UDP", "dir=in", "action=allow", "protocol=UDP", "localport=38810,38811", "enable=yes"])
            .creation_flags(0x08000000)
            .status();

        // Auto-register OpenVR driver with SteamVR
        let vrpathreg_candidates = [
            r"C:\Program Files (x86)\Steam\steamapps\common\SteamVR\bin\win64\vrpathreg.exe",
            r"C:\Program Files\Steam\steamapps\common\SteamVR\bin\win64\vrpathreg.exe",
        ];
        let driver_candidates = [
            Some(std::path::PathBuf::from(r"C:\ALVR\installations\v21.0.0-dev14+nightly.2026.10.25")),
            std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("openvr_driver"))),
            Some(std::path::PathBuf::from(r"C:\Program Files\NebulynxGames\PrismSuite\Aperture\openvr_driver")),
            Some(std::path::PathBuf::from(r"C:\Users\Phoenix\Documents\OpenReality\tools\openvr_driver")),
        ];

        for vrpathreg in vrpathreg_candidates {
            if std::path::Path::new(vrpathreg).exists() {
                for driver_opt in &driver_candidates {
                    if let Some(drv) = driver_opt {
                        if drv.join("driver.vrdrivermanifest").exists() {
                            let _ = std::process::Command::new(vrpathreg)
                                .args(["adddriver", &drv.to_string_lossy()])
                                .creation_flags(0x08000000)
                                .status();
                            info!("Registered OpenVR driver at {}", drv.display());
                            break;
                        }
                    }
                }
                break;
            }
        }
    });

    info!("Prism Aperture Server listening for Quest clients on port {DEFAULT_CONTROL_PORT}...");

    // Ensure SteamVR virtual HMD is configured (avoiding Error 108: Headset Not Detected)
    crate::driver_bridge::ensure_steamvr_configured(1832, 1920, 1536, 960);
    let driver_bridge = Arc::new(crate::driver_bridge::DriverBridge::new());

    // 4. Client Accept Loop
    while !shutdown.load(Ordering::SeqCst) {
        match server.accept().await {
            Ok(peer) => {
                info!("Client connected from {}", peer.peer_addr());
                telemetry.connected_clients.fetch_add(1, Ordering::SeqCst);

                let telem = Arc::clone(&telemetry);
                let sett = settings.clone();
                let shut = Arc::clone(&shutdown);
                let bridge = Arc::clone(&driver_bridge);

                tokio::spawn(async move {
                    handle_client_session(peer, sett, telem, shut, bridge).await;
                });
            }
            Err(e) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                warn!("Accept error: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    info!("Streaming orchestrator shut down cleanly");
}

/// Responds to UDP broadcast discovery requests from Quest headsets
async fn run_udp_discovery_responder(shutdown: Arc<AtomicBool>) {
    let socket = match UdpSocket::bind("0.0.0.0:38810").await {
        Ok(s) => s,
        Err(e) => {
            warn!("Could not bind UDP discovery socket on port 38810: {e}");
            return;
        }
    };

    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PrismAperture PC".to_string());

    let response = format!("PA_SERVER;{hostname};38810\n");
    let mut buf = [0u8; 1024];

    while !shutdown.load(Ordering::SeqCst) {
        match socket.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let msg = String::from_utf8_lossy(&buf[..n]);
                if msg.trim().starts_with("PA_DISCOVER") || msg.trim().starts_with("OR_DISCOVER") {
                    debug!("Received discovery ping from {src}, replying");
                    let _ = socket.send_to(response.as_bytes(), src).await;
                }
            }
            Err(_) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Handles an active streaming session with a connected Quest headset
async fn handle_client_session(
    peer: PeerConnection,
    settings: StreamerSettings,
    telemetry: Arc<StreamTelemetry>,
    shutdown: Arc<AtomicBool>,
    driver_bridge: Arc<crate::driver_bridge::DriverBridge>,
) {
    let peer_addr = peer.peer_addr();
    let peer = Arc::new(peer);

    // Notify SteamVR OpenVR driver bridge of active connection
    driver_bridge.set_connected(true);

    // Send initial PeerInfo
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PrismAperture-PC".to_string());

    let peer_info = proto::PeerInfo {
        computer_id: "prism-aperture-pc".to_string(),
        computer_name: hostname,
        os: "Windows".to_string(),
        streamer_version: env!("CARGO_PKG_VERSION").to_string(),
        private_addresses: vec!["127.0.0.1".to_string()],
        allow_remote_connections: false,
        encrypt_local_traffic: false,
        encrypt_remote_traffic: false,
    };

    let msg = NetMessage::new(
        MessageType::PeerInfo as u32,
        prost::Message::encode_to_vec(&peer_info),
        1,
    );
    let _ = peer.send_message(msg).await;

    // 1. Spawning Input Receiver Loop (TCP)
    let peer_in = Arc::clone(&peer);
    let shut_in = Arc::clone(&shutdown);
    let telem_in = Arc::clone(&telemetry);

    let input_task = tokio::spawn(async move {
        let mut injector = match InputInjector::new() {
            Ok(i) => Some(i),
            Err(e) => {
                warn!("Could not initialize InputInjector: {e}");
                None
            }
        };

        while !shut_in.load(Ordering::SeqCst) {
            match peer_in.recv_message().await {
                Ok(net_msg) => {
                    // Type 100 or InputEvent
                    if net_msg.message_type == 100 || net_msg.message_type == (MessageType::BoundaryDataChanged as u32) {
                        if let Some(ref mut inj) = injector {
                            handle_input_event(inj, &net_msg.payload);
                        }
                    } else if net_msg.message_type == (MessageType::LaunchSteamVr as u32) {
                        info!("Received LaunchSteamVr from Quest client! Launching SteamVR runtime...");
                        crate::driver_bridge::ensure_steamvr_configured(1832, 1920, 1536, 960);
                        // Notify headset to drop into VR environment
                        let vr_msg = NetMessage::new(MessageType::ShowDesktop as u32, vec![0], 0);
                        let _ = peer_in.send_message(vr_msg).await;
                        #[cfg(windows)]
                        {
                            let _ = std::process::Command::new("cmd")
                                .args(["/C", "start", "steam://rungameid/250820"])
                                .creation_flags(0x08000000)
                                .spawn();
                        }
                    } else if net_msg.message_type == (MessageType::LaunchGame as u32) {
                        let app_id = String::from_utf8_lossy(&net_msg.payload);
                        info!("Received LaunchGame request for app ID: {app_id}");
                        crate::driver_bridge::ensure_steamvr_configured(1832, 1920, 1536, 960);
                        // Notify headset to drop into VR environment
                        let vr_msg = NetMessage::new(MessageType::ShowDesktop as u32, vec![0], 0);
                        let _ = peer_in.send_message(vr_msg).await;
                        #[cfg(windows)]
                        {
                            let cmd_str = format!("steam://rungameid/{app_id}");
                            let _ = std::process::Command::new("cmd")
                                .args(["/C", "start", &cmd_str])
                                .creation_flags(0x08000000)
                                .spawn();
                        }
                    } else if net_msg.message_type == (MessageType::MobileSettingsPropertyChanged as u32) {
                        info!("Headset updated mobile settings");
                    }
                }
                Err(e) => {
                    warn!("Control channel read error from {peer_addr}: {e}");
                    break;
                }
            }
        }
    });

    // 2. Spawning Video Streaming Loop (UDP)
    let peer_vid = Arc::clone(&peer);
    let shut_vid = Arc::clone(&shutdown);
    let telem_vid = Arc::clone(&telemetry);
    let sett_vid = settings.clone();
    let bridge_vid = Arc::clone(&driver_bridge);

    let video_task = tokio::spawn(async move {
        run_video_streaming_loop(peer_vid, sett_vid, telem_vid, shut_vid, bridge_vid).await;
    });

    // 3. Spawning VR App / Environment Watcher Loop
    let peer_vr = Arc::clone(&peer);
    let shut_vr = Arc::clone(&shutdown);
    let vr_task = tokio::spawn(async move {
        let mut last_vr_state = false;
        while !shut_vr.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let current_vr_state = is_vr_process_running();
            if current_vr_state != last_vr_state {
                last_vr_state = current_vr_state;
                let payload = if current_vr_state { vec![0] } else { vec![1] };
                info!("VR session state changed: active = {current_vr_state}. Notifying Quest client...");
                let msg = NetMessage::new(MessageType::ShowDesktop as u32, payload, 0);
                if let Err(_) = peer_vr.send_message(msg).await {
                    break;
                }
            }
        }
    });

    // Wait for session to terminate
    tokio::select! {
        _ = input_task => {},
        _ = video_task => {},
        _ = vr_task => {},
    }

    info!("Session ended for client {peer_addr}");
    driver_bridge.set_connected(false);
    telemetry.connected_clients.fetch_sub(1, Ordering::SeqCst);
}

#[cfg(windows)]
fn is_vr_process_running() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let titles = [
            "SteamVR Status",
            "SteamVR",
            "SteamVR Compositor",
            "VRChat",
            "Beat Saber",
            "Blade & Sorcery",
        ];
        for title in &titles {
            let w = to_wide(title);
            if let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR::from_raw(w.as_ptr())) {
                if !hwnd.is_invalid() {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(not(windows))]
fn is_vr_process_running() -> bool {
    false
}

/// Parse raw binary input packet and inject into Windows
fn handle_input_event(injector: &mut InputInjector, data: &[u8]) {
    // MouseInput: x(f32), y(f32), scroll_delta(i32), button_flags(u32), absolute(bool)
    if data.len() >= 17 {
        let x = f32::from_le_bytes(data[0..4].try_into().unwrap()) as f64;
        let y = f32::from_le_bytes(data[4..8].try_into().unwrap()) as f64;
        let scroll = i32::from_le_bytes(data[8..12].try_into().unwrap());
        let buttons = u32::from_le_bytes(data[12..16].try_into().unwrap());

        // Inject mouse move
        let _ = injector.inject_mouse_move(x, y, true);

        // Inject clicks
        let left_pressed = (buttons & 1) != 0;
        let right_pressed = (buttons & 2) != 0;
        let _ = injector.inject_mouse_button(MouseButton::Left, left_pressed);
        let _ = injector.inject_mouse_button(MouseButton::Right, right_pressed);

        // Inject scroll
        if scroll != 0 {
            let _ = injector.inject_scroll(0, scroll);
        }
    }
}

/// Capture screen, encode, and transmit UDP video chunks
async fn run_video_streaming_loop(
    peer: Arc<PeerConnection>,
    settings: StreamerSettings,
    telemetry: Arc<StreamTelemetry>,
    shutdown: Arc<AtomicBool>,
    driver_bridge: Arc<crate::driver_bridge::DriverBridge>,
) {
    let mut capture = match DesktopCapture::new(0) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to initialize DesktopCapture: {e}");
            return;
        }
    };

    let cap_w = capture.width();
    let cap_h = capture.height();
    let width = if cap_w > 0 { cap_w } else { 1920 };
    let height = if cap_h > 0 { cap_h } else { 1080 };
    info!("Initializing video encoder matching display resolution: {}x{}", width, height);

    let encoder_config = EncoderConfig {
        width,
        height,
        framerate: settings.framerate,
        bitrate_kbps: settings.bitrate_limit_mbps * 1000,
        codec: match settings.preferred_codec {
            crate::settings::VideoCodec::H264 => VideoCodec::H264,
            crate::settings::VideoCodec::Hevc | crate::settings::VideoCodec::Hevc10bit => VideoCodec::Hevc,
            crate::settings::VideoCodec::Av1 | crate::settings::VideoCodec::Av110bit => VideoCodec::Av1,
            crate::settings::VideoCodec::Automatic => VideoCodec::Hevc,
        },
        quality_preset: QualityPreset::UltraFast,
        keyframe_interval: settings.framerate * 2,
        use_hardware: true,
    };

    let mut encoder = match VideoEncoder::new(encoder_config) {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to initialize VideoEncoder: {e}");
            return;
        }
    };

    let frame_interval = Duration::from_micros(1_000_000 / settings.framerate as u64);
    let mut last_frame_time = Instant::now();
    let mut frames_sent = 0u32;
    let mut bytes_sent = 0usize;
    let mut stat_timer = Instant::now();

    while !shutdown.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now.duration_since(last_frame_time) < frame_interval {
            tokio::time::sleep(Duration::from_millis(1)).await;
            continue;
        }
        last_frame_time = now;

        // Capture desktop frame
        match capture.capture_frame(5) {
            Ok(Some(frame)) => {
                match encoder.encode_frame(&frame.data, frame.width, frame.height, frame.format) {
                    Ok(packets) => {
                        for pkt in packets {
                            if !pkt.data.is_empty() {
                                bytes_sent += pkt.data.len();
                                frames_sent += 1;

                                if let Err(e) = peer.send_video_frame(&pkt.data).await {
                                    warn!("send_video_frame error: {e}");
                                }
                                driver_bridge.update_frame(frames_sent);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("encode_frame error: {e}");
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!("capture_frame error: {e}");
            }
        }

        // Update telemetry stats once per second
        if stat_timer.elapsed() >= Duration::from_secs(1) {
            let elapsed_sec = stat_timer.elapsed().as_secs_f32();
            let fps = frames_sent as f32 / elapsed_sec;
            let mbps = (bytes_sent as f32 * 8.0) / (elapsed_sec * 1_000_000.0);
            let lat = peer.latency_ms() as f32;

            info!("Streaming telemetry: {fps:.1} FPS, {mbps:.2} Mbps, {lat:.1} ms latency (sent {frames_sent} frames/sec)");

            *telemetry.current_fps.write().await = fps;
            *telemetry.current_bitrate_mbps.write().await = mbps;
            *telemetry.current_latency_ms.write().await = lat;

            frames_sent = 0;
            bytes_sent = 0;
            stat_timer = Instant::now();
        }
    }
}
