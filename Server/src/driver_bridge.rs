//! Versioned Prism-owned tracking and compositor IPC; see prism_ipc.h.
use anyhow::{bail, ensure, Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Mutex,
    },
};
use tracing::{info, warn};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::{
            Memory::{
                CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
                MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
            },
            SystemInformation::GetTickCount64,
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        },
    },
};

const MAGIC: u32 = 0x52564150;
const PIXEL_OFFSET: usize = 4096;
const MAX_WIDTH: u32 = 4096;
const MAX_HEIGHT: u32 = 2048;
const MAPPING_BYTES: usize = PIXEL_OFFSET + MAX_WIDTH as usize * MAX_HEIGHT as usize * 4;
const DEVICE_INFO_OFFSET: usize = 264;
const DEVICE_INFO_MAGIC: u32 = 0x49445650;
const FRAME_POSE_OFFSET: usize = 280;
const FRAME_POSE_MAGIC: u32 = 0x46505650;
#[repr(C)]
#[derive(Clone, Copy)]
struct DeviceInfo { magic: u32, version: u32, hmd_type: u32, reserved: u32 }
#[derive(Clone, PartialEq, prost::Message)]
struct MobileIdentity {
    #[prost(uint32, tag = "1")]
    hmd_type: u32,
    #[prost(uint32, tag = "2")]
    resolution_width: u32,
    #[prost(uint32, tag = "3")]
    resolution_height: u32,
}
fn decode_mobile_identity(payload: &[u8]) -> Result<(u32, u32, u32)> {
    let identity = <MobileIdentity as prost::Message>::decode(payload)?;
    ensure!(identity.hmd_type <= 5, "Unknown Quest headset type");
    if identity.resolution_width == 0 || identity.resolution_height == 0 {
        return Ok((identity.hmd_type, 0, 0));
    }
    ensure!(
        (512..=4096).contains(&identity.resolution_width)
            && (512..=4096).contains(&identity.resolution_height),
        "Invalid OpenXR eye resolution"
    );
    Ok((identity.hmd_type, identity.resolution_width, identity.resolution_height))
}
fn write_device_info(pixels: *mut u8, hmd_type: u32, eye_width: u32, eye_height: u32) {
    // Derive the reserved metadata address from the mapped-region pixel pointer.
    // Callers hold the existing mapping mutex.
    unsafe {
        pixels.sub(PIXEL_OFFSET - DEVICE_INFO_OFFSET).cast::<DeviceInfo>()
            .write(DeviceInfo {
                magic: DEVICE_INFO_MAGIC,
                version: 1,
                hmd_type,
                // Keep the ABI's 16-byte reserved slot; dimensions are per eye.
                reserved: (eye_width & 0xffff) | ((eye_height & 0xffff) << 16),
            });
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Pose {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub valid: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Tracking {
    pub version: u32,
    pub flags: u32,
    pub sample_time_us: u64,
    pub head: Pose,
    pub hands: [Pose; 2],
    pub trigger: [f32; 2],
    pub grip: [f32; 2],
    pub stick: [f32; 4],
    pub buttons: [u32; 2],
    pub fov: [f32; 8],
    pub ipd: f32,
    pub refresh_rate: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Header {
    magic: u32,
    version: u32,
    server_heartbeat_ms: u64,
    driver_heartbeat_ms: u64,
    tracking_received_ms: u64,
    frame_sequence: u64,
    frame_time_ms: u64,
    client_connected: u32,
    frame_width: u32,
    frame_height: u32,
    frame_stride: u32,
    driver_ready: u32,
    frame_format: u32,
    tracking: Tracking,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SharedFramePose { magic:u32, version:u32, sequence:u64, head:Pose, fov:[f32;8], sample_time_us:u64 }
const _: () = assert!(std::mem::size_of::<Tracking>() == 192);
const _: () = assert!(std::mem::size_of::<Header>() == 264);
const _: () = assert!(std::mem::size_of::<SharedFramePose>() == 88);
fn tick() -> u64 {
    unsafe { GetTickCount64() }
}
fn validate_pose(p: &mut Pose) -> Result<()> {
    ensure!(p.valid <= 1, "Invalid pose-valid flag");
    if p.valid == 0 {
        return Ok(());
    }
    ensure!(
        p.position.iter().all(|v| v.is_finite() && v.abs() < 1000.0),
        "Invalid pose position"
    );
    let norm: f32 = p.orientation.iter().map(|v| v * v).sum();
    ensure!(
        norm.is_finite() && (0.25..4.0).contains(&norm),
        "Invalid pose orientation"
    );
    for v in &mut p.orientation {
        *v /= norm.sqrt();
    }
    Ok(())
}
pub fn decode_tracking(bytes: &[u8]) -> Result<Tracking> {
    ensure!(bytes.len() == 192, "Tracking packet must be 192 bytes");
    // Both endpoints are little-endian. Every bit pattern is valid for these
    // integer/float fields; packet length and the repr(C) layout are checked.
    let mut t = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<Tracking>()) };
    ensure!(
        t.version == 1 && t.flags & !1 == 0,
        "Unsupported tracking version/flags"
    );
    validate_pose(&mut t.head)?;
    for p in &mut t.hands {
        validate_pose(p)?;
    }
    ensure!(
        t.trigger
            .iter()
            .chain(t.grip.iter())
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
        "Invalid analog input"
    );
    ensure!(
        t.stick
            .iter()
            .all(|v| v.is_finite() && (-1.0..=1.0).contains(v)),
        "Invalid thumbstick"
    );
    ensure!(
        t.fov.iter().all(|v| v.is_finite() && v.abs() < 1.57),
        "Invalid eye FOV"
    );
    ensure!(
        t.ipd.is_finite() && (0.04..0.09).contains(&t.ipd),
        "Invalid IPD"
    );
    ensure!((60..=144).contains(&t.refresh_rate), "Invalid refresh rate");
    Ok(t)
}
struct Mapping {
    handle: HANDLE,
    mutex: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    local: Mutex<()>,
}
// All shared access is under local + named mutexes; references never escape.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    fn new() -> Result<Self> {
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\PrismAperture.OpenVR.Mutex.v1"))?;
            let handle = match CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                MAPPING_BYTES as u32,
                w!("Local\\PrismAperture.OpenVR.v1"),
            ) {
                Ok(h) => h,
                Err(e) => {
                    let _ = CloseHandle(mutex);
                    return Err(e.into());
                }
            };
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, MAPPING_BYTES);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                let _ = CloseHandle(mutex);
                bail!("Could not map Prism IPC");
            }
            let mapping = Self {
                handle,
                mutex,
                view,
                local: Mutex::new(()),
            };
            mapping
                .with(100, |h, pixels| {
                    let valid = h.magic == MAGIC && h.version == 1;
                    *h = Header {
                        magic: MAGIC,
                        version: 1,
                        server_heartbeat_ms: tick(),
                        driver_heartbeat_ms: if valid { h.driver_heartbeat_ms } else { 0 },
                        driver_ready: if valid { h.driver_ready } else { 0 },
                        ..Default::default()
                    };
                    write_device_info(pixels, 0, 0, 0);
                })
                .context("Prism IPC busy during initialization")?;
            Ok(mapping)
        }
    }
    fn with<T>(&self, timeout: u32, f: impl FnOnce(&mut Header, *mut u8) -> T) -> Option<T> {
        let _local = self.local.lock().ok()?;
        let wait = unsafe { WaitForSingleObject(self.mutex, timeout) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            return None;
        }
        struct Unlock(HANDLE);
        impl Drop for Unlock {
            fn drop(&mut self) {
                unsafe {
                    let _ = ReleaseMutex(self.0);
                }
            }
        }
        let _unlock = Unlock(self.mutex);
        unsafe {
            Some(f(
                &mut *self.view.Value.cast::<Header>(),
                self.view.Value.cast::<u8>().add(PIXEL_OFFSET),
            ))
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.mutex);
        }
    }
}
pub struct VrFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub pose: Option<FramePose>,
}
#[derive(Clone, Copy, Debug)]
pub struct FramePose {
    pub orientation: [f32;4],
    pub fov: [f32;8],
    pub sample_time_us: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverStatus {
    Unavailable,
    WaitingForTracking,
    WaitingForDriver,
    Ready,
}
pub struct DriverBridge {
    mapping: Option<Mapping>,
    clients: AtomicU32,
    requested: AtomicBool,
}
impl DriverBridge {
    pub fn new() -> Self {
        let mapping = match Mapping::new() {
            Ok(m) => {
                info!("Prism OpenVR IPC initialized (ABI 1)");
                Some(m)
            }
            Err(e) => {
                warn!("Prism OpenVR IPC unavailable: {e}");
                None
            }
        };
        Self {
            mapping,
            clients: AtomicU32::new(0),
            requested: AtomicBool::new(std::env::var("PRISM_PCVR_AUTO").as_deref() == Ok("1")),
        }
    }
    pub fn set_connected(&self, connected: bool) {
        if connected {
            self.clients.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = self
                .clients
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_sub(1))
                });
        }
        if let Some(m) = &self.mapping {
            m.with(10, |h, pixels| {
                h.client_connected = u32::from(self.clients.load(Ordering::SeqCst) > 0);
                h.server_heartbeat_ms = tick();
                if h.client_connected == 0 {
                    write_device_info(pixels, 0, 0, 0);
                    h.tracking.head.valid = 0;
                    for hand in &mut h.tracking.hands {
                        hand.valid = 0;
                    }
                }
            });
        }
    }
    pub fn update_tracking(&self, bytes: &[u8]) -> Result<()> {
        let t = decode_tracking(bytes)?;
        self.mapping
            .as_ref()
            .context("Prism IPC unavailable")?
            .with(3, |h, _| {
                h.tracking = t;
                h.tracking_received_ms = tick();
                h.server_heartbeat_ms = tick();
            })
            .context("Prism tracking IPC busy")?;
        Ok(())
    }
    pub fn update_device_info(&self, payload: &[u8]) -> Result<()> {
        let (hmd_type, eye_width, eye_height) = decode_mobile_identity(payload)?;
        self.mapping.as_ref().context("Prism IPC unavailable")?
            .with(10, |_, pixels| write_device_info(pixels, hmd_type, eye_width, eye_height))
            .context("Prism device metadata IPC busy")?;
        info!("Quest headset type {hmd_type} with OpenXR eye target {eye_width}x{eye_height} published to driver");
        Ok(())
    }
    pub fn status(&self) -> DriverStatus {
        let Some(m) = &self.mapping else {
            return DriverStatus::Unavailable;
        };
        m.with(3, |h, _| {
            let now = tick();
            h.server_heartbeat_ms = now;
            if h.client_connected == 0
                || h.tracking.head.valid == 0
                || now.saturating_sub(h.tracking_received_ms) > 500
            {
                DriverStatus::WaitingForTracking
            } else if h.driver_ready == 0 || now.saturating_sub(h.driver_heartbeat_ms) > 2000 {
                DriverStatus::WaitingForDriver
            } else {
                DriverStatus::Ready
            }
        })
        .unwrap_or(DriverStatus::WaitingForDriver)
    }
    pub fn request_vr(&self, value: bool) {
        self.requested.store(value, Ordering::SeqCst);
    }
    pub fn vr_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
    pub fn read_frame(&self, last: u64) -> Option<VrFrame> {
        self.mapping.as_ref()?.with(3, |h, pixels| {
            h.server_heartbeat_ms = tick();
            if h.driver_ready == 0
                || h.frame_sequence == 0
                || h.frame_sequence == last
                || tick().saturating_sub(h.frame_time_ms) > 1000
            {
                return None;
            }
            let (w, hh) = (h.frame_width, h.frame_height);
            if w == 0
                || hh == 0
                || w > MAX_WIDTH
                || hh > MAX_HEIGHT
                || h.frame_stride != w * 4
                || h.frame_format != 0
            {
                return None;
            }
            let data = unsafe {
                std::slice::from_raw_parts(pixels, w as usize * hh as usize * 4).to_vec()
            };
            let pose=unsafe { pixels.sub(PIXEL_OFFSET-FRAME_POSE_OFFSET).cast::<SharedFramePose>().read_unaligned() };
            let pose=if pose.magic==FRAME_POSE_MAGIC && pose.version==1 && pose.sequence==h.frame_sequence && pose.head.valid==1 {
                let norm:f32=pose.head.orientation.iter().map(|v|v*v).sum();
                let f=pose.fov;
                if norm.is_finite() && (0.25..4.0).contains(&norm) && f.iter().all(|v|v.is_finite()&&v.abs()<1.57)
                    && f[0]<0.0 && f[1]>0.0 && f[2]>0.0 && f[3]<0.0 && f[4]<0.0 && f[5]>0.0 && f[6]>0.0 && f[7]<0.0 {
                    Some(FramePose { orientation:pose.head.orientation.map(|v|v/norm.sqrt()), fov:f, sample_time_us:pose.sample_time_us })
                } else { None }
            } else { None };
            Some(VrFrame {
                data,
                width: w,
                height: hh,
                sequence: h.frame_sequence,
                pose,
            })
        })?
    }
    pub fn update_frame(&self, _frame: u32) {}
}
pub fn steamvr_root() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("PRISM_STEAMVR_PATH") {
        let p = PathBuf::from(p);
        if p.join("steamxr_win64.json").is_file() {
            return Ok(p);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        if let Ok(bytes) = std::fs::read(PathBuf::from(local).join("openvr/openvrpaths.vrpath")) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(paths) = v["runtime"].as_array() {
                    for p in paths.iter().filter_map(|p| p.as_str()) {
                        let p = PathBuf::from(p);
                        if p.join("steamxr_win64.json").is_file() {
                            return Ok(p);
                        }
                    }
                }
            }
        }
    }
    bail!("SteamVR runtime not found; set PRISM_STEAMVR_PATH")
}
pub fn driver_root() -> Result<PathBuf> {
    let adjacent = std::env::current_exe()?
        .parent()
        .context("No executable directory")?
        .join("prism_openvr_driver");
    for p in [
        adjacent,
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/prism_openvr_driver/driver"),
    ] {
        if p.join("bin/win64/driver_prism_aperture.dll").is_file()
            && p.join("driver.vrdrivermanifest").is_file()
        {
            return Ok(p);
        }
    }
    bail!("Prism driver not built; run tools/prism_openvr_driver/build_driver.ps1")
}
pub fn configure_runtime() -> Result<PathBuf> {
    use std::os::windows::process::CommandExt;
    let runtime = steamvr_root()?;
    let driver = driver_root()?;
    let settings = runtime
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .context("Invalid SteamVR directory")?
        .join("config/steamvr.vrsettings");
    if settings.is_file() {
        let bytes = std::fs::read(&settings)?;
        let mut v: serde_json::Value = serde_json::from_slice(&bytes)?;
        if v["steamvr"]["forcedDriver"].as_str() == Some("null") {
            let backup = settings.with_extension(format!(
                "vrsettings.prism-backup-{}",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            ));
            std::fs::write(&backup, &bytes)?;
            v["steamvr"]["forcedDriver"] = serde_json::Value::String("prism_aperture".into());
            v["driver_null"]["enable"] = serde_json::Value::Bool(false);
            std::fs::write(&settings, serde_json::to_vec_pretty(&v)?)?;
            info!("Replaced legacy null driver; backup {}", backup.display());
        }
    }
    let out = std::process::Command::new(runtime.join("bin/win64/vrpathreg.exe"))
        .arg("adddriver")
        .arg(&driver)
        .creation_flags(0x08000000)
        .output()?;
    ensure!(
        out.status.success(),
        "OpenVR registration failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    info!("Registered Prism driver {}", driver.display());
    Ok(runtime.join("steamxr_win64.json"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_sizes_and_offsets() {
        assert_eq!(std::mem::offset_of!(Header, tracking), 72);
        assert_eq!(std::mem::size_of::<Tracking>(), 192);
        assert_eq!(std::mem::size_of::<DeviceInfo>(), 16);
        assert!(DEVICE_INFO_OFFSET + std::mem::size_of::<DeviceInfo>() <= PIXEL_OFFSET);
    }
    #[test]
    fn recognizes_all_quest_models_without_changing_tracking_abi() {
        for hmd_type in 1..=5 {
            assert_eq!(decode_mobile_identity(&[8, hmd_type]).unwrap().0, u32::from(hmd_type));
        }
        assert!(decode_mobile_identity(&[8, 6]).is_err());
        assert!(decode_mobile_identity(&[8, 0x80]).is_err());
        // Unknown mobile settings fields are skipped by the protobuf decoder.
        assert_eq!(decode_mobile_identity(&[8, 5, 16, 0x80, 0x0f]).unwrap(), (5, 0, 0));
    }
    #[test]
    fn rejects_invalid_tracking() {
        assert!(decode_tracking(&[0; 191]).is_err());
        assert!(decode_tracking(&[0; 192]).is_err());
        let mut t = Tracking {
            version: 1,
            ipd: 0.063,
            refresh_rate: 90,
            ..Default::default()
        };
        let raw = |t: &Tracking| unsafe {
            std::slice::from_raw_parts((t as *const Tracking).cast::<u8>(), 192).to_vec()
        };
        assert!(decode_tracking(&raw(&t)).is_ok());
        t.head.valid = 1;
        t.head.orientation = [0., 0., 0., 1.];
        assert!(decode_tracking(&raw(&t)).is_ok());
        t.head.position[0] = f32::NAN;
        assert!(decode_tracking(&raw(&t)).is_err());
    }
}
