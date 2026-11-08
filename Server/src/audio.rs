//! Audio capture and playback module for OpenReality.
//!
//! Provides WASAPI loopback capture for streaming desktop audio to the VR headset,
//! and WASAPI render for playing back headset microphone data on the PC.
//!
//! Inspired by Virtual Desktop Streamer's `LoopbackCapture` and `Microphone` classes.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use tracing::{debug, error, info, warn};
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    IMMDeviceCollection, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

// ─── Public Types ───────────────────────────────────────────────────────────

/// Controls whether and how audio is streamed to the headset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioStreaming {
    /// No audio is streamed.
    Disabled,
    /// Audio plays only in the headset; PC speakers are muted.
    HeadsetOnly,
    /// Audio plays in both the headset and through PC speakers.
    HeadsetAndComputer,
}

impl Default for AudioStreaming {
    fn default() -> Self {
        Self::HeadsetAndComputer
    }
}

/// Describes the PCM format of an audio stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl AudioFormat {
    /// Bytes per single sample frame (all channels).
    pub fn frame_size(&self) -> usize {
        (self.channels as usize) * (self.bits_per_sample as usize / 8)
    }
}

/// A timestamped buffer of captured audio samples (interleaved IEEE float).
#[derive(Debug, Clone)]
pub struct AudioSamples {
    /// Interleaved f32 PCM data.
    pub data: Vec<f32>,
    /// Format of the samples.
    pub format: AudioFormat,
    /// Moment the samples were read.
    pub timestamp: Instant,
}

/// Metadata for an enumerated audio device.
#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    /// Device endpoint ID (pass to [`AudioCapture::set_device`]).
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// Whether this device is the system default for its direction.
    pub is_default: bool,
    /// `true` for capture/input devices, `false` for render/output devices.
    pub is_input: bool,
}

// ─── COM Guard ──────────────────────────────────────────────────────────────

/// RAII guard that calls `CoUninitialize` on drop.
struct ComGuard;

impl ComGuard {
    fn init() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .context("CoInitializeEx failed")?;
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

// ─── Device Enumeration ─────────────────────────────────────────────────────

/// Enumerates all active audio input and output devices.
pub fn enumerate_audio_devices() -> Result<Vec<AudioDeviceInfo>> {
    let _com = ComGuard::init()?;
    let mut devices = Vec::new();

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("Failed to create MMDeviceEnumerator")?;

        // Get default endpoints for comparison.
        let default_render_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|d| d.GetId().ok())
            .map(|id| id.to_string().unwrap_or_default());

        let default_capture_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()
            .and_then(|d| d.GetId().ok())
            .map(|id| id.to_string().unwrap_or_default());

        // Enumerate render devices.
        enumerate_direction(
            &enumerator,
            eRender,
            false,
            &default_render_id,
            &mut devices,
        )?;

        // Enumerate capture devices.
        enumerate_direction(
            &enumerator,
            eCapture,
            true,
            &default_capture_id,
            &mut devices,
        )?;
    }

    Ok(devices)
}

/// Helper: enumerate devices for a single data-flow direction.
unsafe fn enumerate_direction(
    enumerator: &IMMDeviceEnumerator,
    direction: windows::Win32::Media::Audio::EDataFlow,
    is_input: bool,
    default_id: &Option<String>,
    out: &mut Vec<AudioDeviceInfo>,
) -> Result<()> {
    // DEVICE_STATE_ACTIVE = 0x1
    let collection: IMMDeviceCollection = enumerator
        .EnumAudioEndpoints(direction, DEVICE_STATE_ACTIVE)
        .context("EnumAudioEndpoints failed")?;

    let count = collection.GetCount().context("GetCount failed")?;

    for i in 0..count {
        let device: IMMDevice = match collection.Item(i) {
            Ok(d) => d,
            Err(e) => {
                warn!("Skipping device index {i}: {e}");
                continue;
            }
        };

        let pwstr_id = device.GetId().context("GetId failed")?;
        let id = pwstr_id.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pwstr_id.as_ptr() as *const _));

        let name = get_device_friendly_name(&device).unwrap_or_else(|_| id.clone());

        let is_default = default_id.as_deref() == Some(id.as_str());

        out.push(AudioDeviceInfo {
            id,
            name,
            is_default,
            is_input,
        });
    }

    Ok(())
}

/// Read the friendly name or derive from device id.
unsafe fn get_device_friendly_name(device: &IMMDevice) -> Result<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    let store = device.OpenPropertyStore(STGM_READ)?;
    let value = store.GetValue(&PKEY_Device_FriendlyName)?;
    Ok(value.to_string())
}

// ─── AudioCapture (WASAPI Loopback) ─────────────────────────────────────────

/// Captures system audio via WASAPI loopback on the default (or selected) render device.
pub struct AudioCapture {
    _com: ComGuard,
    client: IAudioClient,
    capture_client: IAudioCaptureClient,
    format: AudioFormat,
    block_align: u16,
    capturing: AtomicBool,
}

impl AudioCapture {
    /// Create a new loopback capture on the **default render endpoint**.
    pub fn new() -> Result<Self> {
        let com = ComGuard::init()?;

        let (client, capture_client, format, block_align) = unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance(MMDeviceEnumerator)")?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("GetDefaultAudioEndpoint(eRender)")?;

            Self::init_loopback_client(&device)?
        };

        info!(
            "AudioCapture initialized: {}Hz, {}ch, {}bit",
            format.sample_rate, format.channels, format.bits_per_sample
        );

        Ok(Self {
            _com: com,
            client,
            capture_client,
            format,
            block_align,
            capturing: AtomicBool::new(false),
        })
    }

    /// Switch to capturing from a different device (by endpoint ID).
    ///
    /// If currently capturing, the capture is stopped, the device is switched,
    /// and capture is restarted.
    pub fn set_device(&mut self, device_id: &str) -> Result<()> {
        let was_capturing = self.capturing.load(Ordering::SeqCst);
        if was_capturing {
            self.stop()?;
        }

        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance(MMDeviceEnumerator)")?;

            let wide_id: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
            let device = enumerator
                .GetDevice(windows::core::PCWSTR(wide_id.as_ptr()))
                .context("GetDevice for given device_id")?;

            let (client, capture_client, format, block_align) =
                Self::init_loopback_client(&device)?;

            self.client = client;
            self.capture_client = capture_client;
            self.format = format;
            self.block_align = block_align;
        }

        info!("Audio device switched to {device_id}");

        if was_capturing {
            self.start()?;
        }

        Ok(())
    }

    /// Start the capture stream.
    pub fn start(&mut self) -> Result<()> {
        if self.capturing.load(Ordering::SeqCst) {
            debug!("AudioCapture::start called but already capturing");
            return Ok(());
        }
        unsafe {
            self.client.Start().context("IAudioClient::Start")?;
        }
        self.capturing.store(true, Ordering::SeqCst);
        info!("Audio capture started");
        Ok(())
    }

    /// Stop the capture stream.
    pub fn stop(&mut self) -> Result<()> {
        if !self.capturing.load(Ordering::SeqCst) {
            debug!("AudioCapture::stop called but not capturing");
            return Ok(());
        }
        unsafe {
            self.client.Stop().context("IAudioClient::Stop")?;
        }
        self.capturing.store(false, Ordering::SeqCst);
        info!("Audio capture stopped");
        Ok(())
    }

    /// Non-blocking read of available captured samples.
    ///
    /// Returns `Ok(None)` when no frames are available yet.
    pub fn read_samples(&mut self) -> Result<Option<AudioSamples>> {
        if !self.capturing.load(Ordering::SeqCst) {
            return Ok(None);
        }

        let mut all_samples: Vec<f32> = Vec::new();

        unsafe {
            loop {
                let packet_length = self
                    .capture_client
                    .GetNextPacketSize()
                    .context("GetNextPacketSize")?;

                if packet_length == 0 {
                    break;
                }

                let mut buffer_ptr: *mut u8 = ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;
                let mut _device_position: u64 = 0;
                let mut _qpc_position: u64 = 0;

                self.capture_client
                    .GetBuffer(
                        &mut buffer_ptr,
                        &mut num_frames,
                        &mut flags,
                        Some(&mut _device_position),
                        Some(&mut _qpc_position),
                    )
                    .context("IAudioCaptureClient::GetBuffer")?;

                if num_frames > 0 {
                    let is_silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;

                    if is_silent {
                        // Push zeros for silent buffers.
                        let sample_count = num_frames as usize * self.format.channels as usize;
                        all_samples.extend(std::iter::repeat(0.0f32).take(sample_count));
                    } else {
                        // Interpret buffer as f32 samples (WASAPI shared mode delivers IEEE float).
                        let total_bytes = num_frames as usize * self.block_align as usize;
                        let float_slice = std::slice::from_raw_parts(
                            buffer_ptr as *const f32,
                            total_bytes / std::mem::size_of::<f32>(),
                        );
                        all_samples.extend_from_slice(float_slice);
                    }
                }

                self.capture_client
                    .ReleaseBuffer(num_frames)
                    .context("ReleaseBuffer")?;
            }
        }

        if all_samples.is_empty() {
            Ok(None)
        } else {
            Ok(Some(AudioSamples {
                data: all_samples,
                format: self.format,
                timestamp: Instant::now(),
            }))
        }
    }

    /// Returns the audio format of the capture stream.
    pub fn format(&self) -> &AudioFormat {
        &self.format
    }

    // ── private helpers ──────────────────────────────────────────────────

    /// Activate an `IAudioClient` in loopback mode on the given device and return
    /// the client, capture client, format, and block alignment.
    unsafe fn init_loopback_client(
        device: &IMMDevice,
    ) -> Result<(IAudioClient, IAudioCaptureClient, AudioFormat, u16)> {
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .context("Activate IAudioClient")?;

        // Query the device's native mix format.
        let wfx_ptr = client.GetMixFormat().context("GetMixFormat")?;
        let wfx = &*wfx_ptr;

        let format = parse_wave_format(wfx)?;
        let block_align = wfx.nBlockAlign;

        // 20 ms buffer duration in 100-ns units.
        let buffer_duration: i64 = 200_000; // 20 ms

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_duration,
                0, // periodicity must be 0 for shared mode
                wfx_ptr,
                None,
            )
            .context("IAudioClient::Initialize (loopback)")?;

        CoTaskMemFree(Some(wfx_ptr as *const _ as *const _));

        let capture_client: IAudioCaptureClient = client
            .GetService()
            .context("GetService::<IAudioCaptureClient>")?;

        Ok((client, capture_client, format, block_align))
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        if self.capturing.load(Ordering::SeqCst) {
            if let Err(e) = self.stop() {
                error!("Error stopping audio capture on drop: {e}");
            }
        }
    }
}

// ─── MicrophonePlayback (WASAPI Render) ──────────────────────────────────────

/// Renders microphone audio received from the VR headset through a local output device.
pub struct MicrophonePlayback {
    _com: ComGuard,
    client: IAudioClient,
    render_client: IAudioRenderClient,
    format: AudioFormat,
    block_align: u16,
    buffer_frame_count: u32,
    playing: AtomicBool,
}

impl MicrophonePlayback {
    /// Create a new render client on the **default capture endpoint** (virtual mic) or
    /// the default render endpoint so headset mic audio is played locally.
    pub fn new() -> Result<Self> {
        let com = ComGuard::init()?;

        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance(MMDeviceEnumerator)")?;

            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("GetDefaultAudioEndpoint(eRender) for mic playback")?;

            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .context("Activate IAudioClient for render")?;

            let wfx_ptr = client.GetMixFormat().context("GetMixFormat (render)")?;
            let wfx = &*wfx_ptr;

            let format = parse_wave_format(wfx)?;
            let block_align = wfx.nBlockAlign;

            // 40 ms buffer for smooth mic playback.
            let buffer_duration: i64 = 400_000; // 40 ms

            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    0, // no special flags for render
                    buffer_duration,
                    0,
                    wfx_ptr,
                    None,
                )
                .context("IAudioClient::Initialize (render)")?;

            CoTaskMemFree(Some(wfx_ptr as *const _ as *const _));

            let buffer_frame_count = client.GetBufferSize().context("GetBufferSize")?;

            let render_client: IAudioRenderClient = client
                .GetService()
                .context("GetService::<IAudioRenderClient>")?;

            info!(
                "MicrophonePlayback initialized: {}Hz, {}ch, {}bit, buffer={}frames",
                format.sample_rate, format.channels, format.bits_per_sample, buffer_frame_count
            );

            Ok(Self {
                _com: com,
                client,
                render_client,
                format,
                block_align,
                buffer_frame_count,
                playing: AtomicBool::new(false),
            })
        }
    }

    /// Start playback.
    pub fn start(&mut self) -> Result<()> {
        if self.playing.load(Ordering::SeqCst) {
            return Ok(());
        }
        unsafe {
            self.client
                .Start()
                .context("IAudioClient::Start (render)")?;
        }
        self.playing.store(true, Ordering::SeqCst);
        info!("Microphone playback started");
        Ok(())
    }

    /// Stop playback.
    pub fn stop(&mut self) -> Result<()> {
        if !self.playing.load(Ordering::SeqCst) {
            return Ok(());
        }
        unsafe {
            self.client.Stop().context("IAudioClient::Stop (render)")?;
            self.client
                .Reset()
                .context("IAudioClient::Reset (render)")?;
        }
        self.playing.store(false, Ordering::SeqCst);
        info!("Microphone playback stopped");
        Ok(())
    }

    /// Write f32 PCM samples to the render buffer.
    ///
    /// If the source `format` differs from the device format, a basic sample-rate or
    /// channel conversion may be needed externally — this method assumes matching formats.
    pub fn write_samples(&mut self, samples: &[f32], format: &AudioFormat) -> Result<()> {
        if !self.playing.load(Ordering::SeqCst) {
            bail!("MicrophonePlayback is not started");
        }

        if format.sample_rate != self.format.sample_rate || format.channels != self.format.channels
        {
            warn!(
                "Format mismatch: source={}Hz/{}ch, device={}Hz/{}ch — samples may be distorted",
                format.sample_rate, format.channels, self.format.sample_rate, self.format.channels
            );
        }

        let channels = self.format.channels as usize;
        let total_frames = samples.len() / channels;

        unsafe {
            let padding = self
                .client
                .GetCurrentPadding()
                .context("GetCurrentPadding")?;
            let available_frames = self.buffer_frame_count.saturating_sub(padding) as usize;
            let frames_to_write = total_frames.min(available_frames);

            if frames_to_write == 0 {
                return Ok(());
            }

            let buffer_ptr = self
                .render_client
                .GetBuffer(frames_to_write as u32)
                .context("IAudioRenderClient::GetBuffer")?;

            // Copy f32 data into the render buffer.
            let byte_count = frames_to_write * self.block_align as usize;
            let src_bytes = std::slice::from_raw_parts(
                samples.as_ptr() as *const u8,
                frames_to_write * channels * std::mem::size_of::<f32>(),
            );
            ptr::copy_nonoverlapping(src_bytes.as_ptr(), buffer_ptr, byte_count);

            self.render_client
                .ReleaseBuffer(frames_to_write as u32, 0)
                .context("IAudioRenderClient::ReleaseBuffer")?;
        }

        Ok(())
    }
}

impl Drop for MicrophonePlayback {
    fn drop(&mut self) {
        if self.playing.load(Ordering::SeqCst) {
            if let Err(e) = self.stop() {
                error!("Error stopping microphone playback on drop: {e}");
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Parse a `WAVEFORMATEX` (potentially `WAVEFORMATEXTENSIBLE`) into an [`AudioFormat`].
unsafe fn parse_wave_format(wfx: &WAVEFORMATEX) -> Result<AudioFormat> {
    let float_format = if wfx.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        anyhow::ensure!(wfx.cbSize >= 22, "Truncated extensible audio format");
        let ext = &*(wfx as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE);
        ptr::read_unaligned(ptr::addr_of!(ext.SubFormat)) == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        wfx.wFormatTag == 3
    };
    anyhow::ensure!(
        float_format
            && wfx.wBitsPerSample == 32
            && wfx.nChannels > 0
            && wfx.nSamplesPerSec > 0
            && u32::from(wfx.nBlockAlign) == u32::from(wfx.nChannels) * 4,
        "WASAPI capture requires a packed 32-bit float mix format"
    );
    let bits = wfx.wBitsPerSample;
    Ok(AudioFormat {
        sample_rate: wfx.nSamplesPerSec,
        channels: wfx.nChannels,
        bits_per_sample: bits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_frame_size() {
        let fmt = AudioFormat {
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 32,
        };
        assert_eq!(fmt.frame_size(), 8); // 2ch * 4 bytes
    }

    #[test]
    fn audio_streaming_default() {
        assert_eq!(
            AudioStreaming::default(),
            AudioStreaming::HeadsetAndComputer
        );
    }

    #[test]
    fn audio_streaming_serde_roundtrip() {
        let val = AudioStreaming::HeadsetOnly;
        let json = serde_json::to_string(&val).unwrap();
        let back: AudioStreaming = serde_json::from_str(&json).unwrap();
        assert_eq!(val, back);
    }
}

/// VB-CABLE's render endpoint is CABLE Input; apps route their sound into it.
pub fn is_vb_cable_render(device: &AudioDeviceInfo) -> bool {
    let name = device.name.to_ascii_lowercase();
    !device.is_input && name.contains("cable input") && name.contains("vb-audio")
}

/// Stateful float PCM to 48 kHz stereo signed little-endian PCM for the Quest player.
#[derive(Default)]
struct StereoPcm {
    frames: Vec<[f32; 2]>,
    phase: f64,
}
impl StereoPcm {
    fn convert(&mut self, samples: &AudioSamples) -> Result<Vec<u8>> {
        let channels = samples.format.channels as usize;
        anyhow::ensure!(
            channels > 0 && samples.format.sample_rate > 0,
            "Invalid audio format"
        );
        for frame in samples.data.chunks_exact(channels) {
            self.frames
                .push([frame[0], frame[if channels > 1 { 1 } else { 0 }]]);
        }
        let step = samples.format.sample_rate as f64 / 48000.0;
        let mut output = Vec::new();
        while self.phase + 1.0 < self.frames.len() as f64 {
            let i = self.phase as usize;
            let fraction = (self.phase - i as f64) as f32;
            for ch in 0..2 {
                let value =
                    self.frames[i][ch] * (1.0 - fraction) + self.frames[i + 1][ch] * fraction;
                let value = if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                output.extend_from_slice(&((value * 32767.0).round() as i16).to_le_bytes());
            }
            self.phase += step;
        }
        let consumed = (self.phase as usize).min(self.frames.len());
        self.frames.drain(..consumed);
        self.phase -= consumed as f64;
        Ok(output)
    }
}

/// Run WASAPI on one dedicated COM thread, separate from video and the UI.
pub fn stream_audio(
    peer: std::sync::Arc<crate::network::PeerConnection>,
    shutdown: std::sync::Arc<AtomicBool>,
    virtual_cable: bool,
    runtime: tokio::runtime::Handle,
) -> Result<()> {
    let mut capture = AudioCapture::new()?;
    if virtual_cable {
        let device = enumerate_audio_devices()?
            .into_iter()
            .find(is_vb_cable_render)
            .context("VB-CABLE not found: install VB-CABLE and route the app to CABLE Input")?;
        capture.set_device(&device.id)?;
        info!("Audio capture using {}", device.name);
    }
    capture.start()?;
    let mut pcm = StereoPcm::default();
    while !shutdown.load(Ordering::SeqCst) {
        if let Some(samples) = capture.read_samples()? {
            let bytes = pcm.convert(&samples)?;
            // Keep stereo sample frames intact and audio packets comfortably below MTU.
            for chunk in bytes.chunks(960) {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                runtime
                    .block_on(async {
                        tokio::time::timeout(
                            std::time::Duration::from_millis(500),
                            peer.send_audio_samples(chunk),
                        )
                        .await
                    })
                    .context("Audio send timeout")??;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    capture.stop()?;
    Ok(())
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    #[test]
    fn cable_direction_is_not_reversed() {
        let mut d = AudioDeviceInfo {
            id: String::new(),
            name: "CABLE Input (VB-Audio Virtual Cable)".into(),
            is_default: false,
            is_input: false,
        };
        assert!(is_vb_cable_render(&d));
        d.is_input = true;
        assert!(!is_vb_cable_render(&d));
        d.is_input = false;
        d.name = "Speakers".into();
        assert!(!is_vb_cable_render(&d));
    }
    #[test]
    fn pcm_conversion_keeps_boundary_and_stereo_alignment() {
        let mut converter = StereoPcm::default();
        let samples = AudioSamples {
            data: vec![1.0, 1.0, 1.0],
            format: AudioFormat {
                sample_rate: 24000,
                channels: 1,
                bits_per_sample: 32,
            },
            timestamp: Instant::now(),
        };
        let first = converter.convert(&samples).unwrap();
        let second = converter.convert(&samples).unwrap();
        assert_eq!(first.len() + second.len(), 10 * 4);
        assert!(first
            .chunks_exact(2)
            .all(|b| i16::from_le_bytes([b[0], b[1]]) == 32767));
    }
}
