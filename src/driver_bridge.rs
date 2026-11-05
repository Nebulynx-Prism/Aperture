//! SteamVR OpenVR Driver Bridge for Prism Aperture.
//!
//! Ensures SteamVR automatically detects the Quest headset without "Error 108: Headset Not Detected".
//! 1. Configures `steamvr.vrsettings` to enable the virtual HMD driver (`driver_null`) with the headset's native resolution.
//! 2. Creates the named shared memory mappings and events expected by OpenVR drivers.

use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
#[cfg(windows)]
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

/// Ensures SteamVR is configured to recognize Prism Aperture HMD
pub fn ensure_steamvr_configured(render_w: u32, render_h: u32, disp_w: u32, disp_h: u32) {
    let settings_candidates = [
        PathBuf::from(r"C:\Program Files (x86)\Steam\config\steamvr.vrsettings"),
        PathBuf::from(r"C:\Program Files\Steam\config\steamvr.vrsettings"),
    ];

    for path in &settings_candidates {
        if path.exists() {
            if let Err(e) = update_steamvr_settings(path, render_w, render_h, disp_w, disp_h) {
                warn!("Failed to update SteamVR settings at {}: {e}", path.display());
            } else {
                info!("Configured SteamVR virtual HMD settings at {}", path.display());
                break;
            }
        }
    }
}

fn update_steamvr_settings(
    path: &Path,
    render_w: u32,
    render_h: u32,
    disp_w: u32,
    disp_h: u32,
) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    // 1. Configure "steamvr" section
    if let Some(steamvr) = root.get_mut("steamvr").and_then(|v| v.as_object_mut()) {
        steamvr.insert("forcedDriver".to_string(), serde_json::Value::String("null".to_string()));
        steamvr.insert("activateMultipleDrivers".to_string(), serde_json::Value::Bool(true));
    } else {
        let mut steamvr_obj = serde_json::Map::new();
        steamvr_obj.insert("forcedDriver".to_string(), serde_json::Value::String("null".to_string()));
        steamvr_obj.insert("activateMultipleDrivers".to_string(), serde_json::Value::Bool(true));
        root.as_object_mut().unwrap().insert("steamvr".to_string(), serde_json::Value::Object(steamvr_obj));
    }

    // 2. Configure "driver_null" section
    let null_driver = serde_json::json!({
        "enable": true,
        "idFacade": "Null",
        "serialNumber": "PrismAperture-HMD",
        "modelNumber": "Quest 2",
        "windowX": 0,
        "windowY": 0,
        "windowWidth": if disp_w > 0 { disp_w } else { 1536 },
        "windowHeight": if disp_h > 0 { disp_h } else { 960 },
        "renderWidth": if render_w > 0 { render_w } else { 1832 },
        "renderHeight": if render_h > 0 { render_h } else { 1920 },
        "secondsFromVsyncToPhotons": 0.01111111,
        "displayFrequency": 90.0
    });

    root.as_object_mut()
        .unwrap()
        .insert("driver_null".to_string(), null_driver);

    let updated = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, updated)?;
    Ok(())
}

/// Holds shared memory and synchronization events for OpenVR driver coordination
pub struct DriverBridge {
    #[cfg(windows)]
    driver_mapping: HANDLE,
    #[cfg(windows)]
    driver_view: MEMORY_MAPPED_VIEW_ADDRESS,
    #[cfg(windows)]
    body_mapping: HANDLE,
    #[cfg(windows)]
    body_view: MEMORY_MAPPED_VIEW_ADDRESS,
    #[cfg(windows)]
    op_event: HANDLE,
    #[cfg(windows)]
    body_event: HANDLE,
}

unsafe impl Send for DriverBridge {}
unsafe impl Sync for DriverBridge {}

impl DriverBridge {
    pub fn new() -> Self {
        #[cfg(windows)]
        unsafe {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;

            fn to_wide(s: &str) -> Vec<u16> {
                OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
            }

            // 1. VirtualDesktop.Oculus.DriverData (64KB)
            let name_driver = to_wide("VirtualDesktop.Oculus.DriverData");
            let driver_mapping = CreateFileMappingW(
                HANDLE::default(),
                None,
                PAGE_READWRITE,
                0,
                65536,
                PCWSTR::from_raw(name_driver.as_ptr()),
            )
            .unwrap_or(HANDLE::default());

            let driver_view = if !driver_mapping.is_invalid() {
                MapViewOfFile(driver_mapping, FILE_MAP_ALL_ACCESS, 0, 0, 65536)
            } else {
                MEMORY_MAPPED_VIEW_ADDRESS { Value: std::ptr::null_mut() }
            };

            // Set connected flag [0x18C] = 1, [0x188] = 1
            if !driver_view.Value.is_null() {
                let p = driver_view.Value as *mut u8;
                std::ptr::write(p.add(0x188) as *mut u32, 1);
                std::ptr::write(p.add(0x18C) as *mut u32, 1);
            }

            // 2. VirtualDesktop.Oculus.OperationStartEvent
            let name_op = to_wide("VirtualDesktop.Oculus.OperationStartEvent");
            let op_event = CreateEventW(
                None,
                false, // auto-reset
                false, // initial state
                PCWSTR::from_raw(name_op.as_ptr()),
            )
            .unwrap_or(HANDLE::default());

            // 3. VirtualDesktop.BodyState (64KB)
            let name_body = to_wide("VirtualDesktop.BodyState");
            let body_mapping = CreateFileMappingW(
                HANDLE::default(),
                None,
                PAGE_READWRITE,
                0,
                65536,
                PCWSTR::from_raw(name_body.as_ptr()),
            )
            .unwrap_or(HANDLE::default());

            let body_view = if !body_mapping.is_invalid() {
                MapViewOfFile(body_mapping, FILE_MAP_ALL_ACCESS, 0, 0, 65536)
            } else {
                MEMORY_MAPPED_VIEW_ADDRESS { Value: std::ptr::null_mut() }
            };

            // 4. VirtualDesktop.BodyStateEvent2
            let name_body_evt = to_wide("VirtualDesktop.BodyStateEvent2");
            let body_event = CreateEventW(
                None,
                false,
                false,
                PCWSTR::from_raw(name_body_evt.as_ptr()),
            )
            .unwrap_or(HANDLE::default());

            info!("SteamVR OpenVR DriverBridge IPC initialized successfully");

            Self {
                driver_mapping,
                driver_view,
                body_mapping,
                body_view,
                op_event,
                body_event,
            }
        }

        #[cfg(not(windows))]
        Self {}
    }

    pub fn set_connected(&self, connected: bool) {
        #[cfg(windows)]
        unsafe {
            if !self.driver_view.Value.is_null() {
                let p = self.driver_view.Value as *mut u8;
                let val: u32 = if connected { 1 } else { 0 };
                std::ptr::write(p.add(0x18C) as *mut u32, val);
                std::ptr::write(p.add(0x188) as *mut u32, 1);
            }
        }
    }

    pub fn update_frame(&self, frame_index: u32) {
        #[cfg(windows)]
        unsafe {
            if !self.driver_view.Value.is_null() {
                let p = self.driver_view.Value as *mut u8;
                std::ptr::write(p.add(0xF0) as *mut u32, frame_index);
            }
            if !self.body_event.is_invalid() {
                let _ = SetEvent(self.body_event);
            }
        }
    }
}

impl Drop for DriverBridge {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            if !self.driver_view.Value.is_null() {
                self.set_connected(false);
                let _ = UnmapViewOfFile(self.driver_view);
            }
            if !self.driver_mapping.is_invalid() {
                let _ = CloseHandle(self.driver_mapping);
            }
            if !self.body_view.Value.is_null() {
                let _ = UnmapViewOfFile(self.body_view);
            }
            if !self.body_mapping.is_invalid() {
                let _ = CloseHandle(self.body_mapping);
            }
            if !self.op_event.is_invalid() {
                let _ = CloseHandle(self.op_event);
            }
            if !self.body_event.is_invalid() {
                let _ = CloseHandle(self.body_event);
            }
            info!("SteamVR OpenVR DriverBridge shut down cleanly");
        }
    }
}
