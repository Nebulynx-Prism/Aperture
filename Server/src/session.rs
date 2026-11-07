//! Streaming session coordinator for OpenReality.
//!
//! Orchestrates the end-to-end communication pipeline:
//! 1. OpenReality Service IPC (Named Pipe): Priority boost & power throttling
//! 2. Discovery: mDNS announcement + UDP broadcast discovery responder
//! 3. Connection lifecycle: TCP control + UDP data streaming
//! 4. Video & Audio streaming: Desktop capture + hardware encode + WASAPI loopback
//! 5. Input injection: Translating VR laser pointer & controllers to Windows desktop inputs

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{watch, RwLock};
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

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.stop();
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
    let bind_addr = SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        DEFAULT_CONTROL_PORT,
    ));
    let mut server = match StreamServer::new(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to bind StreamServer on {bind_addr}: {e}");
            telemetry.is_streaming.store(false, Ordering::SeqCst);
            return;
        }
    };

    // Broadcast mDNS
    if let Err(e) = server.broadcast_discovery().await {
        warn!("mDNS broadcast warning: {e}");
    }

    // ADB reverse forwards TCP only. Do not restart adbd into wireless mode:
    // that drops the USB transport and is unrelated to Wi-Fi streaming.
    #[cfg(windows)]
    tokio::task::spawn_blocking(|| {
        let candidates = [
            std::path::PathBuf::from("adb"),
            std::path::PathBuf::from(r"C:\Program Files (x86)\ADB & Fastboot++\adb.exe"),
        ];
        for adb in candidates {
            match std::process::Command::new(&adb)
                .args(["reverse", "tcp:38810", "tcp:38810"])
                .creation_flags(0x08000000)
                .output()
            {
                Ok(out) => {
                    if out.status.success() {
                        info!("USB TCP tunnel configured on port 38810");
                    } else {
                        warn!(
                            "USB tunnel unavailable: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        );
                    }
                    break;
                }
                Err(_) => continue,
            }
        }
    });
    info!("Prism Aperture Server listening for Quest clients on port {DEFAULT_CONTROL_PORT}...");

    // Ensure SteamVR virtual HMD is configured (avoiding Error 108: Headset Not Detected)

    let driver_bridge = Arc::new(crate::driver_bridge::DriverBridge::new());

    // 4. Client Accept Loop
    while !shutdown.load(Ordering::SeqCst) {
        let accepted = tokio::select! {
            result = server.accept() => result,
            _ = async {
                while !shutdown.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } => break,
        };
        match accepted {
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
    telemetry.is_streaming.store(false, Ordering::SeqCst);
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
        let received = tokio::select! {
            result = socket.recv_from(&mut buf) => result,
            _ = tokio::time::sleep(Duration::from_millis(200)) => continue,
        };
        match received {
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
    let session_shutdown = Arc::new(AtomicBool::new(false));
    let (config_ack_tx, config_ack_rx) = watch::channel((0u32, false));
    let force_keyframe = Arc::new(AtomicBool::new(false));

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
    let shut_in = Arc::clone(&session_shutdown);
    let bridge_in = Arc::clone(&driver_bridge);
    let keyframe_in = Arc::clone(&force_keyframe);
    let telem_in = Arc::clone(&telemetry);

    let mut input_task = tokio::spawn(async move {
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
                    if net_msg.message_type == 103 && net_msg.payload.len() == 5 {
                        let generation =
                            u32::from_be_bytes(net_msg.payload[1..5].try_into().unwrap());
                        let _ = config_ack_tx.send((generation, net_msg.payload[0] == 1));
                    } else if net_msg.message_type == 105 {
                        if let Err(e) = bridge_in.update_tracking(&net_msg.payload) {
                            debug!("Tracking packet ignored: {e}");
                        }
                    } else if net_msg.message_type == MessageType::ForceKeyFrame as u32 {
                        keyframe_in.store(true, Ordering::SeqCst);
                    } else if net_msg.message_type == 100 {
                        if let Some(ref mut inj) = injector {
                            handle_input_event(inj, &net_msg.payload);
                        }
                    } else if net_msg.message_type == MessageType::LaunchSteamVr as u32
                        || net_msg.message_type == MessageType::LaunchGame as u32
                    {
                        let bridge = Arc::clone(&bridge_in);
                        let peer = Arc::clone(&peer_in);
                        let app_id = if net_msg.message_type == MessageType::LaunchSteamVr as u32 {
                            "250820".to_string()
                        } else {
                            String::from_utf8_lossy(&net_msg.payload).into_owned()
                        };
                        if app_id.is_empty()
                            || app_id.len() > 20
                            || !app_id.bytes().all(|b| b.is_ascii_digit())
                        {
                            continue;
                        }
                        tokio::spawn(async move {
                            let configured = tokio::task::spawn_blocking(
                                crate::driver_bridge::configure_runtime,
                            )
                            .await;
                            let status = match configured {
                                Ok(Ok(runtime)) => {
                                    bridge.request_vr(true);
                                    info!("PCVR requested using {}", runtime.display());
                                    let _ = std::process::Command::new("explorer.exe")
                                        .arg(format!("steam://rungameid/{app_id}"))
                                        .creation_flags(0x08000000)
                                        .spawn();
                                    "PCVR starting; waiting for real SteamVR compositor frames"
                                        .to_string()
                                }
                                other => format!("PCVR setup failed: {other:?}"),
                            };
                            let _ = peer
                                .send_message(NetMessage::new(104, status.into_bytes(), 0))
                                .await;
                        });
                    } else if net_msg.message_type == MessageType::ExitSteamVr as u32
                        || (net_msg.message_type == MessageType::ShowDesktop as u32
                            && net_msg.payload == [1])
                    {
                        bridge_in.request_vr(false);
                    } else if net_msg.message_type
                        == (MessageType::MobileSettingsPropertyChanged as u32)
                    {
                        if let Err(e) = bridge_in.update_device_info(&net_msg.payload) {
                            warn!("Headset identity update rejected: {e}");
                        }
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
    let shut_vid = Arc::clone(&session_shutdown);
    let telem_vid = Arc::clone(&telemetry);
    let sett_vid = settings.clone();
    let bridge_vid = Arc::clone(&driver_bridge);

    let mut video_task = tokio::spawn(async move {
        run_video_streaming_loop(
            peer_vid,
            sett_vid,
            telem_vid,
            shut_vid,
            bridge_vid,
            config_ack_rx,
            force_keyframe,
        )
        .await;
    });

    // A desktop TCP session is not a PCVR compositor session. Do not infer VR
    // readiness from window titles or detach tasks when any session loop ends.
    tokio::select! {
        _ = &mut input_task => {},
        _ = &mut video_task => {},
        _ = async {
            while !shutdown.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } => {},
    }
    session_shutdown.store(true, Ordering::SeqCst);
    // select! already consumed the completed handle's result. Polling that
    // handle again panics; only abort and await the still-running sibling.
    stop_worker(input_task).await;
    stop_worker(video_task).await;
    info!("Session ended for client {peer_addr}");
    driver_bridge.set_connected(false);
    telemetry.connected_clients.fetch_sub(1, Ordering::SeqCst);
}

async fn stop_worker(task: tokio::task::JoinHandle<()>) {
    if !task.is_finished() {
        task.abort();
        let _ = task.await;
    }
}

/// Parse raw binary input packet and inject into Windows
fn handle_input_event(injector: &mut InputInjector, data: &[u8]) {
    // MouseInput: x(f32), y(f32), scroll_delta(i32), button_flags(u32), absolute(bool)
    if data.len() == 17 || data.len() == 20 {
        let x = f32::from_le_bytes(data[0..4].try_into().unwrap()) as f64;
        let y = f32::from_le_bytes(data[4..8].try_into().unwrap()) as f64;
        let scroll = i32::from_le_bytes(data[8..12].try_into().unwrap());
        let buttons = u32::from_le_bytes(data[12..16].try_into().unwrap());
        if !x.is_finite() || !y.is_finite() || buttons & !3 != 0 || data[16] > 1 {
            return;
        }

        // Inject mouse move
        if let Err(e) = injector.inject_mouse_move(x, y, data[16] != 0) {
            warn!("Mouse move injection failed: {e}");
        }

        // Inject clicks
        let left_pressed = (buttons & 1) != 0;
        let right_pressed = (buttons & 2) != 0;
        if let Err(e) = injector.inject_mouse_button(MouseButton::Left, left_pressed) {
            warn!("Left mouse injection failed: {e}");
        }
        if let Err(e) = injector.inject_mouse_button(MouseButton::Right, right_pressed) {
            warn!("Right mouse injection failed: {e}");
        }

        // Inject scroll
        if scroll != 0 {
            let _ = injector.inject_scroll(0, scroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_accepts_consumed_handle_and_stops_sibling() {
        struct Running(Arc<AtomicBool>);
        impl Drop for Running {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let running = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&running);
        let sibling = tokio::spawn(async move {
            let _guard = Running(Arc::clone(&worker_state));
            worker_state.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        assert!(running.load(Ordering::SeqCst));
        let mut completed = tokio::spawn(async {});
        tokio::select! { _ = &mut completed => {} }
        stop_worker(completed).await;
        stop_worker(sibling).await;
        assert!(!running.load(Ordering::SeqCst));
    }
}

/// Negotiate a stream generation before sending any video for it.
async fn configure_stream(
    peer: &PeerConnection,
    ack: &mut watch::Receiver<(u32, bool)>,
    codec: VideoCodec,
    width: u32,
    height: u32,
    stereo: bool,
    generation: u32,
) -> Result<()> {
    let values = [
        if codec == VideoCodec::Hevc {
            1u32
        } else {
            0u32
        },
        width,
        height,
        u32::from(stereo),
        generation,
    ];
    let payload = values.iter().flat_map(|v| v.to_be_bytes()).collect();
    peer.send_message(NetMessage::new(102, payload, generation))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (id, ok) = *ack.borrow_and_update();
            if id == generation {
                anyhow::ensure!(ok, "Client rejected stream generation {id}");
                return Ok::<(), anyhow::Error>(());
            }
            ack.changed()
                .await
                .context("Client decoder acknowledgement channel closed")?;
        }
    })
    .await
    .context("Client decoder configuration timed out")??;
    info!(
        "Stream generation {generation}: {}x{} codec={codec} stereo={stereo}",
        width, height
    );
    Ok(())
}

/// Capture desktops or real compositor frames, encode and transmit.
async fn run_video_streaming_loop(
    peer: Arc<PeerConnection>,
    settings: StreamerSettings,
    telemetry: Arc<StreamTelemetry>,
    shutdown: Arc<AtomicBool>,
    driver_bridge: Arc<crate::driver_bridge::DriverBridge>,
    mut config_ack: watch::Receiver<(u32, bool)>,
    force_keyframe: Arc<AtomicBool>,
) {
    let result = stream_video(
        peer.clone(),
        settings,
        telemetry,
        shutdown,
        driver_bridge,
        &mut config_ack,
        force_keyframe,
    )
    .await;
    if let Err(e) = result {
        error!("Video session failed: {e:#}");
        let _ = peer
            .send_message(NetMessage::new(
                104,
                format!("Video stopped: {e:#}").into_bytes(),
                0,
            ))
            .await;
    }
}
async fn stream_video(
    peer: Arc<PeerConnection>,
    settings: StreamerSettings,
    telemetry: Arc<StreamTelemetry>,
    shutdown: Arc<AtomicBool>,
    driver_bridge: Arc<crate::driver_bridge::DriverBridge>,
    config_ack: &mut watch::Receiver<(u32, bool)>,
    force_keyframe: Arc<AtomicBool>,
) -> Result<()> {
    let mut capture = DesktopCapture::new(0)?;
    let desktop_size = (capture.width(), capture.height());
    let framerate = settings.framerate.clamp(1, 120);
    let codec = match settings.preferred_codec {
        crate::settings::VideoCodec::Hevc | crate::settings::VideoCodec::Hevc10bit => {
            VideoCodec::Hevc
        }
        _ => VideoCodec::H264,
    };
    let mut generation = 1u32;
    let mut stereo = false;
    let mut encoder_config = EncoderConfig {
        width: desktop_size.0,
        height: desktop_size.1,
        framerate,
        bitrate_kbps: settings.bitrate_limit_mbps.clamp(1, 200) * 1000,
        codec,
        quality_preset: QualityPreset::UltraFast,
        keyframe_interval: framerate * 2,
        use_hardware: true,
    };
    configure_stream(
        &peer,
        config_ack,
        codec,
        desktop_size.0,
        desktop_size.1,
        false,
        generation,
    )
    .await?;
    let mut encoder = VideoEncoder::new(encoder_config.clone())?;
    let mut last_desktop = None;
    let mut last_vr: Option<crate::driver_bridge::VrFrame> = None;
    let mut last_vr_sequence = 0u64;
    let mut last_vr_received = Instant::now();
    let mut encoded_input_index = 0u64;
    let mut pending_frame_poses: BTreeMap<u64, crate::driver_bridge::FramePose> = BTreeMap::new();
    let mut cursor = crate::cursor::CursorOverlay::new().ok();
    let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / framerate as u64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut frames, mut bytes) = (0u32, 0usize);
    let mut stats = Instant::now();
    let mut last_keyframe = Instant::now();
    while !shutdown.load(Ordering::SeqCst) {
        interval.tick().await;
        let mut new_vr_frame = false;
        let ready = driver_bridge.status() == crate::driver_bridge::DriverStatus::Ready;
        let wants_vr = driver_bridge.vr_requested() && ready;
        if wants_vr {
            if let Some(frame) = driver_bridge.read_frame(last_vr_sequence) {
                last_vr_sequence = frame.sequence;
                last_vr_received = Instant::now();
                last_vr = Some(frame);
                new_vr_frame = true;
            }
        }
        let use_vr =
            wants_vr && last_vr.is_some() && last_vr_received.elapsed() < Duration::from_secs(2);
        let dimensions = if use_vr {
            let frame = last_vr.as_ref().unwrap();
            (frame.width, frame.height)
        } else {
            desktop_size
        };
        if use_vr != stereo || (encoder_config.width, encoder_config.height) != dimensions {
            generation = generation
                .checked_add(1)
                .context("Stream generation exhausted")?;
            stereo = use_vr;
            encoder_config.width = dimensions.0;
            encoder_config.height = dimensions.1;
            // Destroy the old encoder and discard its delayed access units.
            drop(encoder);
            configure_stream(
                &peer,
                config_ack,
                codec,
                dimensions.0,
                dimensions.1,
                stereo,
                generation,
            )
            .await?;
            encoder = VideoEncoder::new(encoder_config.clone())?;
            encoded_input_index = 0;
            pending_frame_poses.clear();
            let status = if stereo {
                "PCVR active: real SteamVR compositor stream"
            } else {
                "Desktop active"
            };
            peer.send_message(NetMessage::new(104, status.as_bytes().to_vec(), 0))
                .await?;
            last_keyframe = Instant::now();
        }
        if force_keyframe.swap(false, Ordering::SeqCst)
            && last_keyframe.elapsed() >= Duration::from_secs(2)
        {
            encoder.force_keyframe();
            last_keyframe = Instant::now();
        }
        let encoded = if stereo {
            if new_vr_frame {
                let frame = last_vr.as_ref().unwrap();
                encoded_input_index += 1;
                if let Some(pose) = frame.pose { pending_frame_poses.insert(encoded_input_index, pose); }
                while pending_frame_poses.len() > 128 {
                    if let Some(oldest) = pending_frame_poses.keys().next().copied() { pending_frame_poses.remove(&oldest); }
                }
                encoder.encode_frame(
                    &frame.data,
                    frame.width,
                    frame.height,
                    crate::capture::FrameFormat::Bgra8,
                )?
            } else {
                // Re-encoding the same compositor image at every refresh tick
                // adds codec/network queueing without adding new visual data.
                Vec::new()
            }
        } else {
            if let Some(frame) = capture.capture_frame(5)? {
                if last_desktop.is_none() {
                    let stride = (frame.data.len() / 4 / 256).max(1);
                    let count = frame
                        .data
                        .chunks_exact(4)
                        .step_by(stride)
                        .filter(|p| p[0] > 4 || p[1] > 4 || p[2] > 4)
                        .count();
                    info!(
                        "First capture {}x{}: {count} nonblack samples",
                        frame.width, frame.height
                    );
                }
                last_desktop = Some(frame);
            }
            let Some(frame) = last_desktop.as_mut() else {
                continue;
            };
            if let Some(overlay) = cursor.as_mut() {
                if let Err(e) = overlay.composite(&mut frame.data, frame.width, frame.height) {
                    warn!("Cursor overlay unavailable: {e}");
                    cursor = None;
                }
            }
            let result = encoder.encode_frame(&frame.data, frame.width, frame.height, frame.format);
            if let Some(overlay) = cursor.as_mut() {
                overlay.restore(&mut frame.data, frame.width, frame.height)?;
            }
            result?
        };
        for packet in encoded {
            if packet.data.is_empty() {
                continue;
            }
            if stereo {
                if let Some(pose) = pending_frame_poses.remove(&(packet.pts as u64)) {
                    let mut payload = Vec::with_capacity(72);
                    payload.extend_from_slice(&1u32.to_le_bytes());
                    payload.extend_from_slice(&1u32.to_le_bytes());
                    payload.extend_from_slice(&(packet.pts as u64).to_be_bytes());
                    for value in pose.orientation { payload.extend_from_slice(&value.to_le_bytes()); }
                    for value in pose.fov { payload.extend_from_slice(&value.to_le_bytes()); }
                    payload.extend_from_slice(&pose.sample_time_us.to_le_bytes());
                    peer.send_message(NetMessage::new(106, payload, packet.pts as u32)).await?;
                }
            }
            // TCP preserves mode/config boundaries for SBS on USB and Wi-Fi.
            peer.send_video_frame_reliable(&packet.data, stereo).await?;
            frames += 1;
            bytes += packet.data.len();
        }
        if stats.elapsed() >= Duration::from_secs(1) {
            let seconds = stats.elapsed().as_secs_f32();
            let fps = frames as f32 / seconds;
            let mbps = bytes as f32 * 8.0 / seconds / 1_000_000.0;
            *telemetry.current_fps.write().await = fps;
            *telemetry.current_bitrate_mbps.write().await = mbps;
            *telemetry.current_latency_ms.write().await = peer.latency_ms() as f32;
            info!(
                "Streaming: {fps:.1} FPS {mbps:.2} Mbps stereo={stereo} driver={:?}",
                driver_bridge.status()
            );
            frames = 0;
            bytes = 0;
            stats = Instant::now();
        }
    }
    Ok(())
}
