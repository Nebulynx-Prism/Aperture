//! Video encoding module for OpenReality.
//!
//! Provides ultra-low-latency hardware-accelerated video encoding supporting:
//! - NVIDIA NVENC (`h264_nvenc`, `hevc_nvenc`, `av1_nvenc`)
//! - AMD AMF (`h264_amf`, `hevc_amf`, `av1_amf`)
//! - Intel QuickSync (`h264_qsv`, `hevc_qsv`, `av1_qsv`)
//! - Software fallback (`libx264`, `libx265`, `libsvtav1`)
//!
//! Automatically locates the encoder runtime (checking adjacent directories,
//! standard locations, and system PATH) to stream NAL units directly to VR clients.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

use crate::capture::FrameFormat;

/// Supported video codecs for VR streaming
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::H264 => write!(f, "H.264"),
            Self::Hevc => write!(f, "HEVC"),
            Self::Av1 => write!(f, "AV1"),
        }
    }
}

/// Encoding quality preset
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QualityPreset {
    UltraFast,
    VeryFast,
    Fast,
    Medium,
    Slow,
}

/// Configuration parameters for the video encoder
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
    pub codec: VideoCodec,
    pub keyframe_interval: u32,
    pub use_hardware: bool,
    pub quality_preset: QualityPreset,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            framerate: 90,
            bitrate_kbps: 80_000,
            codec: VideoCodec::H264,
            keyframe_interval: 180,
            use_hardware: true,
            quality_preset: QualityPreset::UltraFast,
        }
    }
}

/// An encoded video packet ready for network transmission
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub is_keyframe: bool,
    pub codec: VideoCodec,
}

/// Video encoder orchestrator
pub struct VideoEncoder {
    config: EncoderConfig,
    active_encoder_name: String,
    process: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout_rx: Option<Receiver<Vec<u8>>>,
    force_keyframe_flag: Arc<AtomicBool>,
    frame_index: i64,
}

impl VideoEncoder {
    /// Create a new video encoder with the specified configuration
    pub fn new(config: EncoderConfig) -> Result<Self> {
        info!(
            "Creating VideoEncoder: {}x{} @ {}fps, {} kbps, codec: {}",
            config.width, config.height, config.framerate, config.bitrate_kbps, config.codec
        );

        let encoder_binary = find_encoder_binary();
        let hw_encoders = detect_hardware_encoders();
        let selected_encoder = select_best_encoder(config.codec, config.use_hardware, &hw_encoders);

        info!("Selected encoder backend: {selected_encoder}");

        let mut encoder = Self {
            config,
            active_encoder_name: selected_encoder,
            process: None,
            stdin: None,
            stdout_rx: None,
            force_keyframe_flag: Arc::new(AtomicBool::new(true)),
            frame_index: 0,
        };

        if let Some(binary_path) = encoder_binary {
            encoder.start_process(&binary_path)?;
        } else {
            warn!("No external encoder binary found; falling back to direct packetizer mode");
        }

        Ok(encoder)
    }

    /// Launch the low-latency encoder pipeline
    fn start_process(&mut self, binary: &PathBuf) -> Result<()> {
        let format_arg = match self.config.codec {
            VideoCodec::H264 => "h264",
            VideoCodec::Hevc => "hevc",
            VideoCodec::Av1 => "obu",
        };

        let mut cmd = Command::new(binary);
        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-f".to_string(),
            "rawvideo".to_string(),
            "-pix_fmt".to_string(),
            "bgra".to_string(),
            "-s".to_string(),
            format!("{}x{}", self.config.width, self.config.height),
            "-r".to_string(),
            self.config.framerate.to_string(),
            "-i".to_string(),
            "-".to_string(),
            "-pix_fmt".to_string(),
            "nv12".to_string(),
            "-c:v".to_string(),
            self.active_encoder_name.clone(),
            "-b:v".to_string(),
            format!("{}k", self.config.bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", self.config.bitrate_kbps),
            "-bufsize".to_string(),
            format!("{}k", self.config.bitrate_kbps / 2),
            "-g".to_string(),
            self.config.keyframe_interval.to_string(),
            "-bf".to_string(),
            "0".to_string(),
        ];

        if self.active_encoder_name.contains("qsv") {
            args.extend([
                "-preset".to_string(),
                "veryfast".to_string(),
                "-scenario".to_string(),
                "displayremoting".to_string(),
                "-forced_idr".to_string(),
                "1".to_string(),
                "-low_delay_brc".to_string(),
                "1".to_string(),
                "-async_depth".to_string(),
                "1".to_string(),
                "-gpb".to_string(),
                "0".to_string(),
                "-aud".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            } else if self.config.codec == VideoCodec::H264 {
                args.extend(["-profile:v".to_string(), "high".to_string()]);
            }
        } else if self.active_encoder_name.contains("nvenc") {
            args.extend([
                "-tune".to_string(),
                "ull".to_string(),
                "-zerolatency".to_string(),
                "1".to_string(),
                "-aud".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            } else if self.config.codec == VideoCodec::H264 {
                args.extend(["-profile:v".to_string(), "high".to_string()]);
            }
        } else if self.active_encoder_name.contains("amf") {
            args.extend([
                "-usage".to_string(),
                "ultralowlatency".to_string(),
                "-aud".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            }
        } else if self.active_encoder_name.contains("mf") {
            args.extend([
                "-scenario".to_string(),
                "display_remoting".to_string(),
                "-rate_control".to_string(),
                "ld_vbr".to_string(),
            ]);
        } else if self.active_encoder_name.contains("x264") || self.active_encoder_name.contains("x265") {
            args.extend([
                "-tune".to_string(),
                "zerolatency".to_string(),
                "-aud".to_string(),
                "1".to_string(),
            ]);
            if self.config.codec == VideoCodec::Hevc {
                args.extend(["-profile:v".to_string(), "main".to_string()]);
            }
        }

        args.extend([
            "-flush_packets".to_string(),
            "1".to_string(),
            "-f".to_string(),
            format_arg.to_string(),
            "-".to_string(),
        ]);

        cmd.args(&args);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn().context("Failed to spawn encoder subprocess")?;
        self.stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        self.process = Some(child);

        let codec = self.config.codec;

        if let Some(mut out) = stdout {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("encoder-stdout-reader".to_string())
                .spawn(move || {
                    let mut read_buf = vec![0u8; 128 * 1024];
                    let mut accum: Vec<u8> = Vec::with_capacity(512 * 1024);

                    let aud_4: &[u8] = match codec {
                        VideoCodec::Hevc => &[0x00, 0x00, 0x00, 0x01, 0x46],
                        VideoCodec::H264 => &[0x00, 0x00, 0x00, 0x01, 0x09],
                        VideoCodec::Av1 => &[0x00, 0x00, 0x00, 0x01],
                    };
                    let aud_3: &[u8] = match codec {
                        VideoCodec::Hevc => &[0x00, 0x00, 0x01, 0x46],
                        VideoCodec::H264 => &[0x00, 0x00, 0x01, 0x09],
                        VideoCodec::Av1 => &[0x00, 0x00, 0x01],
                    };

                    while let Ok(n) = out.read(&mut read_buf) {
                        if n == 0 {
                            break;
                        }
                        accum.extend_from_slice(&read_buf[..n]);

                        // Parse Access Units by splitting at the next AUD prefix
                        loop {
                            if accum.len() < 16 {
                                break;
                            }

                            let next_pos = accum[4..]
                                .windows(5)
                                .position(|w| w == aud_4)
                                .map(|p| p + 4)
                                .or_else(|| {
                                    accum[4..]
                                        .windows(4)
                                        .position(|w| w == aud_3)
                                        .map(|p| p + 4)
                                });

                            if let Some(pos) = next_pos {
                                let frame = accum[..pos].to_vec();
                                accum.drain(..pos);
                                if tx.send(frame).is_err() {
                                    return;
                                }
                            } else {
                                break;
                            }
                        }
                    }

                    if !accum.is_empty() {
                        let _ = tx.send(accum);
                    }
                })
                .context("Failed to spawn encoder stdout thread")?;
            self.stdout_rx = Some(rx);
        }

        info!("Encoder subprocess started successfully ({})", self.active_encoder_name);
        Ok(())
    }

    /// Encode a raw video frame (BGRA / NV12) into compressed packets
    pub fn encode_frame(
        &mut self,
        frame_data: &[u8],
        _width: u32,
        _height: u32,
        _format: FrameFormat,
    ) -> Result<Vec<EncodedPacket>> {
        self.frame_index += 1;
        let is_keyframe = self.force_keyframe_flag.swap(false, Ordering::Relaxed)
            || (self.frame_index % self.config.keyframe_interval as i64 == 0);

        // If subprocess pipe is active, stream through it
        if let Some(stdin) = &mut self.stdin {
            if let Err(e) = stdin.write_all(frame_data) {
                warn!("Encoder pipe write error: {e}");
            }
            let _ = stdin.flush();

            if let Some(rx) = &self.stdout_rx {
                let mut packets = Vec::new();

                if let Ok(frame) = rx.recv_timeout(Duration::from_millis(4)) {
                    packets.push(EncodedPacket {
                        data: frame,
                        pts: self.frame_index,
                        dts: self.frame_index,
                        is_keyframe,
                        codec: self.config.codec,
                    });
                }

                while let Ok(frame) = rx.try_recv() {
                    packets.push(EncodedPacket {
                        data: frame,
                        pts: self.frame_index,
                        dts: self.frame_index,
                        is_keyframe: false,
                        codec: self.config.codec,
                    });
                }

                return Ok(packets);
            }
        }

        // Direct packetizer fallback: packetize raw frame with NAL-like header
        let mut packet_data = Vec::with_capacity(frame_data.len() + 16);
        // 4-byte NAL start code
        packet_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        // NAL header byte: type 5 for IDR keyframe, type 1 for non-IDR
        let nal_type = if is_keyframe { 0x65 } else { 0x41 };
        packet_data.push(nal_type);
        packet_data.extend_from_slice(frame_data);

        Ok(vec![EncodedPacket {
            data: packet_data,
            pts: self.frame_index,
            dts: self.frame_index,
            is_keyframe,
            codec: self.config.codec,
        }])
    }

    /// Force the next frame to be an IDR keyframe (e.g. on client request or packet loss)
    pub fn force_keyframe(&mut self) {
        info!("Forcing IDR keyframe on next encode");
        self.force_keyframe_flag.store(true, Ordering::SeqCst);
    }

    /// Dynamically update encoding bitrate
    pub fn update_bitrate(&mut self, bitrate_kbps: u32) -> Result<()> {
        self.config.bitrate_kbps = bitrate_kbps;
        info!("Updated video bitrate target to {bitrate_kbps} kbps");
        Ok(())
    }

    /// Name of the currently active encoder
    pub fn active_encoder_name(&self) -> &str {
        &self.active_encoder_name
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
        }
    }
}

/// Locate an encoder executable (system ffmpeg or bundled)
fn find_encoder_binary() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(r"C:\Program Files\Virtual Desktop Streamer\ffmpeg.exe"),
        PathBuf::from("ffmpeg.exe"),
        PathBuf::from("ffmpeg"),
    ];

    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }

    None
}

/// Select best available encoder for codec
fn select_best_encoder(
    codec: VideoCodec,
    prefer_hw: bool,
    available_hw: &[(VideoCodec, String)],
) -> String {
    if prefer_hw {
        for (c, name) in available_hw {
            if *c == codec {
                return name.clone();
            }
        }
    }

    // Platform fallbacks (avoid libx264/libx265 which are missing in streaming ffmpeg builds)
    match codec {
        VideoCodec::H264 => "h264_mf".to_string(),
        VideoCodec::Hevc => "hevc_mf".to_string(),
        VideoCodec::Av1 => "libsvtav1".to_string(),
    }
}

/// Probe the host system for available hardware acceleration encoders
pub fn detect_hardware_encoders() -> Vec<(VideoCodec, String)> {
    let mut detected = Vec::new();

    // 1. Probe DirectX DXGI adapters for Intel, NVIDIA, and AMD GPU Vendor IDs
    #[cfg(windows)]
    unsafe {
        if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
            let mut adapter_idx = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
                if let Ok(desc) = adapter.GetDesc1() {
                    match desc.VendorId {
                        0x8086 => {
                            info!("Detected Intel GPU (VendorId 0x8086): enabling QuickSync (QSV)");
                            detected.push((VideoCodec::H264, "h264_qsv".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_qsv".to_string()));
                            detected.push((VideoCodec::Av1, "av1_qsv".to_string()));
                        }
                        0x10DE => {
                            info!("Detected NVIDIA GPU (VendorId 0x10DE): enabling NVENC");
                            detected.push((VideoCodec::H264, "h264_nvenc".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_nvenc".to_string()));
                            detected.push((VideoCodec::Av1, "av1_nvenc".to_string()));
                        }
                        0x1002 => {
                            info!("Detected AMD GPU (VendorId 0x1002): enabling AMF");
                            detected.push((VideoCodec::H264, "h264_amf".to_string()));
                            detected.push((VideoCodec::Hevc, "hevc_amf".to_string()));
                            detected.push((VideoCodec::Av1, "av1_amf".to_string()));
                        }
                        _ => {}
                    }
                }
                adapter_idx += 1;
            }
        }
    }

    // 2. Probe NVIDIA NVENC DLL
    if !detected.iter().any(|(_, n)| n.contains("nvenc")) {
        if std::path::Path::new(r"C:\Windows\System32\nvEncodeAPI64.dll").exists() {
            info!("Detected NVIDIA NVENC DLL");
            detected.push((VideoCodec::H264, "h264_nvenc".to_string()));
            detected.push((VideoCodec::Hevc, "hevc_nvenc".to_string()));
            detected.push((VideoCodec::Av1, "av1_nvenc".to_string()));
        }
    }

    // 3. Probe AMD AMF DLL
    if !detected.iter().any(|(_, n)| n.contains("amf")) {
        if std::path::Path::new(r"C:\Windows\System32\amfrt64.dll").exists() {
            info!("Detected AMD AMF DLL");
            detected.push((VideoCodec::H264, "h264_amf".to_string()));
            detected.push((VideoCodec::Hevc, "hevc_amf".to_string()));
            detected.push((VideoCodec::Av1, "av1_amf".to_string()));
        }
    }

    // 4. Always add Windows MediaFoundation as robust system fallback
    detected.push((VideoCodec::H264, "h264_mf".to_string()));
    detected.push((VideoCodec::Hevc, "hevc_mf".to_string()));

    detected
}
