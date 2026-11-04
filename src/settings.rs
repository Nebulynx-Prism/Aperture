//! Settings module for OpenReality.
//!
//! Loads and saves configuration from %APPDATA%/OpenReality/settings.json.
//! Replaces all legacy branding with OpenReality while maintaining full
//! compatibility with Quest VR clients via the (VD Accounts) feature.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Video codec preference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    Automatic,
    H264,
    Hevc,
    Hevc10bit,
    Av1,
    Av110bit,
}

impl Default for VideoCodec {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Audio streaming mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioStreaming {
    Disabled,
    HeadsetOnly,
    HeadsetAndComputer,
}

impl Default for AudioStreaming {
    fn default() -> Self {
        Self::HeadsetAndComputer
    }
}

/// Gamepad emulation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GamepadEmulation {
    Automatic,
    Xbox,
    Dualshock,
}

impl Default for GamepadEmulation {
    fn default() -> Self {
        Self::Automatic
    }
}

/// OpenXR runtime selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenXRRuntime {
    Automatic,
    SteamVR,
    OpenReality,
}

impl Default for OpenXRRuntime {
    fn default() -> Self {
        Self::Automatic
    }
}

/// Account entry for Virtual Desktop Quest client compatibility
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VdAccount {
    pub username: String,
    pub platform: String, // "OculusQuest", "Pico", "Vive", "Apple"
    pub enabled: bool,
}

/// Main OpenReality streamer settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerSettings {
    // Identity & Discovery
    pub computer_id: String,
    pub computer_name: String,
    pub allow_remote_connections: bool,
    pub encrypt_local_traffic: bool,

    // Quest Client Compatibility (VD Accounts)
    pub vd_compatibility_enabled: bool,
    pub vd_accounts: Vec<VdAccount>,

    // Startup
    pub start_with_windows: bool,
    pub start_minimized_in_tray: bool,

    // Video Streaming
    pub preferred_codec: VideoCodec,
    pub auto_adjust_bitrate: bool,
    pub bitrate_limit_mbps: u32,
    pub framerate: u32,
    pub enable_aq: bool,
    pub enable_yuv444: bool,
    pub enable_2pass: bool,

    // Foveated Rendering & Optics
    pub use_fov_stencil: bool,
    pub horizontal_fov_tangent: f32,
    pub vertical_fov_tangent: f32,
    pub fovea_size: f32,

    // Post-processing
    pub use_sharpening: bool,
    pub sharpening: f32,
    pub render_resolution_override: f32,

    // Audio
    pub audio_streaming: AudioStreaming,
    pub auto_select_microphone: bool,
    pub use_virtual_audio_driver: bool,
    pub voicemeeter_mode: bool,

    // Input & Interaction
    pub use_touch_input: bool,
    pub gamepad_emulation: GamepadEmulation,
    pub lock_computer: bool,
    pub boost_game_priority: bool,

    // VR Platform
    pub openxr_runtime: OpenXRRuntime,
    pub show_pairing_requests: bool,

    // Media & Paths
    pub videos_root_path: String,
    pub screenshots_root_path: String,
    pub launch_game_directory: String,

    // Display & UI
    pub selected_tab: u32,
    pub monitor_count: u32,

    // Hardware encoder status (detected at runtime)
    pub device_name: String,
    pub codec_name: String,

    // Last session
    pub last_connect_date: String,
}

impl Default for StreamerSettings {
    fn default() -> Self {
        let computer_id = uuid::Uuid::new_v4().to_string();
        let computer_name = whoami_hostname();

        Self {
            computer_id,
            computer_name,
            allow_remote_connections: false,
            encrypt_local_traffic: true,

            vd_compatibility_enabled: true,
            vd_accounts: vec![VdAccount {
                username: "QuestUser".to_string(),
                platform: "OculusQuest".to_string(),
                enabled: true,
            }],

            start_with_windows: false,
            start_minimized_in_tray: false,

            preferred_codec: VideoCodec::Automatic,
            auto_adjust_bitrate: true,
            bitrate_limit_mbps: 100,
            framerate: 90,
            enable_aq: true,
            enable_yuv444: false,
            enable_2pass: false,

            use_fov_stencil: true,
            horizontal_fov_tangent: 1.0,
            vertical_fov_tangent: 1.0,
            fovea_size: 0.5,

            use_sharpening: true,
            sharpening: 0.5,
            render_resolution_override: 1.0,

            audio_streaming: AudioStreaming::HeadsetAndComputer,
            auto_select_microphone: true,
            use_virtual_audio_driver: false,
            voicemeeter_mode: false,

            use_touch_input: true,
            gamepad_emulation: GamepadEmulation::Automatic,
            lock_computer: false,
            boost_game_priority: true,

            openxr_runtime: OpenXRRuntime::Automatic,
            show_pairing_requests: true,

            videos_root_path: String::new(),
            screenshots_root_path: String::new(),
            launch_game_directory: String::new(),

            selected_tab: 0,
            monitor_count: 1,

            device_name: "Auto-detect GPU".to_string(),
            codec_name: "H.264 / HEVC / AV1".to_string(),

            last_connect_date: String::new(),
        }
    }
}

fn whoami_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PrismAperture-PC".to_string())
}

impl StreamerSettings {
    /// Get the configuration file path (%APPDATA%/PrismAperture/settings.json)
    fn settings_path() -> PathBuf {
        let dirs = directories::ProjectDirs::from("org", "Nebulynx", "PrismAperture")
            .expect("Could not determine config directory");
        let config_dir = dirs.config_dir();
        std::fs::create_dir_all(config_dir).ok();
        config_dir.join("settings.json")
    }

    /// Load settings from disk, or create defaults
    pub fn load_or_default() -> Self {
        let path = Self::settings_path();
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str(&content) {
                    tracing::info!("Loaded settings from {}", path.display());
                    return settings;
                }
            }
        }

        let settings = Self::default();
        let _ = settings.save();
        settings
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::settings_path();
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        tracing::debug!("Settings saved to {}", path.display());
        Ok(())
    }

    /// Reset settings to defaults, preserving the existing computer ID
    pub fn reset(&mut self) -> Result<()> {
        let id = self.computer_id.clone();
        let accounts = self.vd_accounts.clone();
        *self = Self::default();
        self.computer_id = id;
        self.vd_accounts = accounts;
        self.save()
    }
}

/// Dynamic runtime state
#[derive(Debug, Clone)]
pub struct DynamicSettings {
    pub is_connected: bool,
    pub connected_peer_name: String,
    pub adapter_name: String,
    pub is_local_traffic_encrypted: bool,
    pub monitors: Vec<MonitorSetting>,
    pub monitor_start_index: usize,
    pub latency_ms: f64,
    pub fps: f32,
    pub bitrate_actual_mbps: f32,
    pub has_hardware_encoder: bool,
    pub supports_yuv444: bool,
    pub supports_2pass: bool,
    pub connection_type: NetworkConnectionType,
    pub routing_status: RoutingStatus,
    pub is_window_visible: bool,
}

#[derive(Debug, Clone)]
pub struct MonitorSetting {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkConnectionType {
    Wireless,
    Ethernet,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStatus {
    PublicIp,
    Nat,
    DoubleNat,
    CgNat,
    Unknown,
}

impl Default for DynamicSettings {
    fn default() -> Self {
        Self {
            is_connected: false,
            connected_peer_name: String::new(),
            adapter_name: "Detecting...".to_string(),
            is_local_traffic_encrypted: true,
            monitors: Vec::new(),
            monitor_start_index: 0,
            latency_ms: 0.0,
            fps: 0.0,
            bitrate_actual_mbps: 0.0,
            has_hardware_encoder: true,
            supports_yuv444: true,
            supports_2pass: false,
            connection_type: NetworkConnectionType::Ethernet,
            routing_status: RoutingStatus::Nat,
            is_window_visible: true,
        }
    }
}
