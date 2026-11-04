//! Screen capture module for OpenReality using DXGI Desktop Duplication API.
//!
//! Mirrors Virtual Desktop Streamer's `DesktopStreamer` class (which used SharpDX.Direct3D11).
//! Captures the Windows desktop at high framerate with low latency directly from GPU memory.

use anyhow::{anyhow, Context, Result};
use std::time::Instant;
use tracing::{debug, error, info, warn};

use windows::core::Interface;
use windows::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};

/// Format of a captured frame
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Bgra8,
    Nv12,
    Rgba8,
}

/// Information about an attached monitor
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub device_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub dpi_scale: f32,
    pub is_primary: bool,
    pub is_portrait: bool,
}

/// A captured desktop frame ready for encoding
#[derive(Clone)]
pub struct CapturedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: FrameFormat,
    pub timestamp: Instant,
    pub frame_number: u64,
    pub monitor_id: u32,
}

/// High-performance desktop screen capture using DXGI Desktop Duplication
pub struct DesktopCapture {
    monitor_id: u32,
    width: u32,
    height: u32,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: Option<IDXGIOutputDuplication>,
    staging_texture: Option<ID3D11Texture2D>,
    frame_counter: u64,
}

impl DesktopCapture {
    /// Create a new capture session for the given monitor ID
    pub fn new(monitor_id: u32) -> Result<Self> {
        info!("Initializing DXGI desktop capture for monitor {monitor_id}");

        let (device, context) = Self::create_d3d_device()?;
        let mut session = Self {
            monitor_id,
            width: 0,
            height: 0,
            device,
            context,
            duplication: None,
            staging_texture: None,
            frame_counter: 0,
        };

        session.init_duplication()?;
        Ok(session)
    }

    /// Create D3D11 device with BGRA support
    fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
        let feature_levels = [D3D_FEATURE_LEVEL_11_0];
        let mut device = None;
        let mut context = None;

        unsafe {
            // First try hardware driver
            let hr = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );

            if hr.is_err() {
                warn!("Hardware D3D11 device creation failed, falling back to WARP driver");
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .context("Failed to create D3D11 WARP device")?;
            }
        }

        let device = device.ok_or_else(|| anyhow!("Failed to obtain D3D11 device"))?;
        let context = context.ok_or_else(|| anyhow!("Failed to obtain D3D11 context"))?;
        Ok((device, context))
    }

    /// Initialize the DXGI Output Duplication interface
    fn init_duplication(&mut self) -> Result<()> {
        unsafe {
            let dxgi_device: IDXGIDevice = self.device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let output: IDXGIOutput = adapter.EnumOutputs(self.monitor_id)?;
            let output1: IDXGIOutput1 = output.cast()?;

            let desc = output.GetDesc()?;
            let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).unsigned_abs();
            let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).unsigned_abs();

            self.width = width;
            self.height = height;

            info!("Duplicating monitor {}: {}x{}", self.monitor_id, width, height);

            let duplication = output1.DuplicateOutput(&self.device)?;
            self.duplication = Some(duplication);

            // Create staging texture for CPU readback
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };

            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            self.staging_texture = staging;

            Ok(())
        }
    }

    /// Capture the next desktop frame. Returns Ok(None) if screen has not changed within timeout.
    pub fn capture_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        if self.duplication.is_none() {
            self.init_duplication()?;
        }

        let duplication = match &self.duplication {
            Some(dup) => dup,
            None => return Ok(None),
        };

        let staging = match &self.staging_texture {
            Some(s) => s,
            None => return Ok(None),
        };

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;

        unsafe {
            let hr = duplication.AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource);

            if let Err(e) = hr {
                if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(None);
                }
                warn!("AcquireNextFrame error ({e}), re-initializing duplication");
                self.duplication = None;
                return Ok(None);
            }

            let resource = match resource {
                Some(res) => res,
                None => {
                    duplication.ReleaseFrame().ok();
                    return Ok(None);
                }
            };

            // Copy captured texture to staging texture
            let texture: ID3D11Texture2D = resource.cast()?;
            self.context.CopyResource(staging, &texture);

            // Release frame back to OS immediately
            duplication.ReleaseFrame().ok();

            // Map staging texture for CPU read
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let row_pitch = mapped.RowPitch as usize;
            let width = self.width as usize;
            let height = self.height as usize;
            let bytes_per_pixel = 4;
            let tight_pitch = width * bytes_per_pixel;

            let mut buffer = vec![0u8; tight_pitch * height];

            let src_slice = std::slice::from_raw_parts(mapped.pData as *const u8, row_pitch * height);

            for y in 0..height {
                let src_offset = y * row_pitch;
                let dst_offset = y * tight_pitch;
                buffer[dst_offset..dst_offset + tight_pitch]
                    .copy_from_slice(&src_slice[src_offset..src_offset + tight_pitch]);
            }

            self.context.Unmap(staging, 0);

            self.frame_counter += 1;

            Ok(Some(CapturedFrame {
                data: buffer,
                width: self.width,
                height: self.height,
                stride: tight_pitch as u32,
                format: FrameFormat::Bgra8,
                timestamp: Instant::now(),
                frame_number: self.frame_counter,
                monitor_id: self.monitor_id,
            }))
        }
    }

    /// Enumerate all active monitors connected to the system
    pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
        let mut monitors = Vec::new();

        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut adapter_idx = 0;

            while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
                let mut output_idx = 0;
                while let Ok(output) = adapter.EnumOutputs(output_idx) {
                    if let Ok(desc) = output.GetDesc() {
                        let name = String::from_utf16_lossy(
                            &desc.DeviceName[..desc.DeviceName.iter().position(|&c| c == 0).unwrap_or(desc.DeviceName.len())],
                        );

                        let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).unsigned_abs();
                        let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).unsigned_abs();

                        monitors.push(MonitorInfo {
                            id: output_idx,
                            name: format!("Monitor {}", output_idx + 1),
                            device_name: name,
                            x: desc.DesktopCoordinates.left,
                            y: desc.DesktopCoordinates.top,
                            width,
                            height,
                            refresh_rate: 60, // Default fallback
                            dpi_scale: 1.0,
                            is_primary: desc.DesktopCoordinates.left == 0 && desc.DesktopCoordinates.top == 0,
                            is_portrait: height > width,
                        });
                    }
                    output_idx += 1;
                }
                adapter_idx += 1;
            }
        }

        if monitors.is_empty() {
            // Fallback: report virtual 1920x1080 display
            monitors.push(MonitorInfo {
                id: 0,
                name: "Primary Display".to_string(),
                device_name: r"\\.\DISPLAY1".to_string(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                refresh_rate: 60,
                dpi_scale: 1.0,
                is_primary: true,
                is_portrait: false,
            });
        }

        Ok(monitors)
    }
}

// ---------------------------------------------------------------------------
// Constants not defined in win32 metadata
// ---------------------------------------------------------------------------
const D3D11_SDK_VERSION: u32 = 7;
