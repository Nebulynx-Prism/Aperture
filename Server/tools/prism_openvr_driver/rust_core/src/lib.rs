mod d3d_capture;
mod runtime;

use core::ffi::c_float;
use core::ffi::c_void;
use std::sync::Mutex;

#[cfg(windows)]
extern "C" { fn tanf(value: f32) -> f32; fn sqrt(value: f64) -> f64; }
#[cfg(not(windows))]
#[link(name = "m")]
extern "C" { fn tanf(value: f32) -> f32; fn sqrt(value: f64) -> f64; }

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismPose {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub valid: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismTracking {
    pub version: u32, pub flags: u32, pub sample_time_us: u64,
    pub head: PrismPose, pub hands: [PrismPose; 2], pub trigger: [f32; 2], pub grip: [f32; 2],
    pub stick: [f32; 4], pub buttons: [u32; 2], pub fov: [f32; 8], pub ipd: f32, pub refresh_rate: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismSharedHeader {
    pub magic: u32, pub version: u32, pub server_heartbeat_ms: u64, pub driver_heartbeat_ms: u64,
    pub tracking_received_ms: u64, pub frame_sequence: u64, pub frame_time_ms: u64,
    pub client_connected: u32, pub frame_width: u32, pub frame_height: u32, pub frame_stride: u32,
    pub driver_ready: u32, pub frame_format: u32, pub tracking: PrismTracking,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismFramePose {
    magic: u32, version: u32, frame_sequence: u64, head: PrismPose, fov: [f32; 8], sample_time_us: u64,
}

#[repr(C)]
#[derive(Default)]
pub struct DriverPoseData { pub connected: u32, pub valid: u32, pub position: [f64; 3], pub orientation_xyzw: [f64; 4] }

const _: () = assert!(core::mem::size_of::<PrismPose>() == 32);
const _: () = assert!(core::mem::size_of::<PrismTracking>() == 192);
const _: () = assert!(core::mem::size_of::<PrismSharedHeader>() == 264);
const _: () = assert!(core::mem::size_of::<PrismFramePose>() == 88);

type Handle = *mut c_void;
const PRISM_MAGIC: u32 = 0x5256_4150;
const PRISM_ABI_VERSION: u32 = 1;
const PRISM_MAPPING_BYTES: usize = 4096 + 4096 * 2048 * 4;
const SYNCHRONIZE: u32 = 0x0010_0000;
const MUTEX_MODIFY_STATE: u32 = 0x0001;
const FILE_MAP_ALL_ACCESS: u32 = 0x000f_001f;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x80;

#[repr(C)]
struct DeviceInfo { magic: u32, version: u32, hmd_type: u32, reserved: u32 }

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn MapViewOfFile(handle: Handle, access: u32, high: u32, low: u32, bytes: usize) -> *mut c_void;
    fn UnmapViewOfFile(address: *const c_void) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: Handle) -> i32;
    fn GetTickCount64() -> u64;
}

#[cfg(windows)]
struct IpcState { mutex: Handle, mapping: Handle, view: *mut PrismSharedHeader }
#[cfg(windows)]
unsafe impl Send for IpcState {}
#[cfg(windows)]
impl IpcState { const fn new() -> Self { Self { mutex: core::ptr::null_mut(), mapping: core::ptr::null_mut(), view: core::ptr::null_mut() } } }
#[cfg(windows)]
static IPC_STATE: Mutex<IpcState> = Mutex::new(IpcState::new());

#[cfg(windows)]
unsafe fn close_ipc_locked(state: &mut IpcState) {
    if !state.view.is_null() { UnmapViewOfFile(state.view.cast()); state.view = core::ptr::null_mut(); }
    if !state.mapping.is_null() { CloseHandle(state.mapping); state.mapping = core::ptr::null_mut(); }
    if !state.mutex.is_null() { CloseHandle(state.mutex); state.mutex = core::ptr::null_mut(); }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_open() -> bool {
    #[cfg(windows)] {
        let Ok(mut state) = IPC_STATE.lock() else { return false };
        if !state.view.is_null() { return true; }
        const MUTEX_NAME: &[u16] = &[76,111,99,97,108,92,80,114,105,115,109,65,112,101,114,116,117,114,101,46,79,112,101,110,86,82,46,77,117,116,101,120,46,118,49,0];
        const MAP_NAME: &[u16] = &[76,111,99,97,108,92,80,114,105,115,109,65,112,101,114,116,117,114,101,46,79,112,101,110,86,82,46,118,49,0];
        state.mutex = OpenMutexW(SYNCHRONIZE | MUTEX_MODIFY_STATE, 0, MUTEX_NAME.as_ptr());
        state.mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, MAP_NAME.as_ptr());
        if state.mutex.is_null() || state.mapping.is_null() {
            close_ipc_locked(&mut state);
            return false;
        }
        state.view = MapViewOfFile(state.mapping, FILE_MAP_ALL_ACCESS, 0, 0, PRISM_MAPPING_BYTES) as *mut PrismSharedHeader;
        if state.view.is_null() { close_ipc_locked(&mut state); return false; }
        let wait = WaitForSingleObject(state.mutex, 0);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED { close_ipc_locked(&mut state); return false; }
        let valid = (*state.view).magic == PRISM_MAGIC && (*state.view).version == PRISM_ABI_VERSION;
        ReleaseMutex(state.mutex);
        if !valid { close_ipc_locked(&mut state); }
        valid
    }
    #[cfg(not(windows))] { false }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_read(out: *mut PrismSharedHeader, timeout_ms: u32) -> bool {
    #[cfg(windows)] {
        if out.is_null() { return false; }
        let Ok(state) = IPC_STATE.lock() else { return false };
        if state.view.is_null() || state.mutex.is_null() { return false; }
        let wait = WaitForSingleObject(state.mutex, timeout_ms);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED { return false; }
        core::ptr::copy_nonoverlapping(state.view, out, 1);
        let valid = (*out).magic == PRISM_MAGIC && (*out).version == PRISM_ABI_VERSION;
        ReleaseMutex(state.mutex);
        valid
    }
    #[cfg(not(windows))] { let _ = (out, timeout_ms); false }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_beat(ready: bool) {
    #[cfg(windows)] {
        let Ok(mut state) = IPC_STATE.lock() else { return };
        if state.view.is_null() || state.mutex.is_null() { return; }
        let wait = WaitForSingleObject(state.mutex, 100);
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            if (*state.view).magic == PRISM_MAGIC && (*state.view).version == PRISM_ABI_VERSION {
                (*state.view).driver_heartbeat_ms = GetTickCount64();
                (*state.view).driver_ready = ready as u32;
            }
            ReleaseMutex(state.mutex);
        }
    }
    #[cfg(not(windows))] { let _ = ready; }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_device_type() -> u32 {
    #[cfg(windows)] {
        let Ok(state) = IPC_STATE.lock() else { return 0 };
        if state.view.is_null() || state.mutex.is_null() { return 0; }
        let wait = WaitForSingleObject(state.mutex, 0);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED { return 0; }
        let info = (state.view as *const u8).add(264) as *const DeviceInfo;
        let result = if (*state.view).magic == PRISM_MAGIC && (*state.view).version == PRISM_ABI_VERSION && (*info).magic == 0x4944_5650 && (*info).version == 1 && (*info).hmd_type <= 5 { (*info).hmd_type } else { 0 };
        ReleaseMutex(state.mutex);
        result
    }
    #[cfg(not(windows))] { 0 }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_device_resolution(width: *mut u32, height: *mut u32) -> bool {
    #[cfg(windows)] {
        if width.is_null() || height.is_null() { return false; }
        let Ok(state) = IPC_STATE.lock() else { return false };
        if state.view.is_null() || state.mutex.is_null() { return false; }
        let wait = WaitForSingleObject(state.mutex, 0);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED { return false; }
        let info = (state.view as *const u8).add(264) as *const DeviceInfo;
        let packed = (*info).reserved;
        let w = packed & 0xffff; let h = packed >> 16;
        let ok = (*state.view).magic == PRISM_MAGIC && (*state.view).version == PRISM_ABI_VERSION && (*info).magic == 0x4944_5650 && (*info).version == 1 && (512..=4096).contains(&w) && (512..=4096).contains(&h);
        if ok { *width = w; *height = h; }
        ReleaseMutex(state.mutex);
        ok
    }
    #[cfg(not(windows))] { let _ = (width, height); false }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_publish_frame(
    src: *const u8, src_pitch: usize, width: u32, height: u32, rgba: bool, pose: *const PrismFramePose,
) -> u64 {
    #[cfg(windows)] {
        if src.is_null() || pose.is_null()
            || width < 512 || width > 4096 || width % 2 != 0 || height < 256 || height > 2048
        { return 0; }
        let Ok(mut state) = IPC_STATE.lock() else { return 0 };
        if state.view.is_null() || state.mutex.is_null() { return 0; }
        let wait = WaitForSingleObject(state.mutex, 1);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED { return 0; }
        let header = &mut *state.view;
        let valid = header.magic == PRISM_MAGIC && header.version == PRISM_ABI_VERSION;
        let mut sequence = 0;
        if valid {
            let dst = (state.view as *mut u8).add(4096);
            if prism_copy_frame_rows(src, src_pitch, dst, width as usize, height as usize, rgba) {
                header.frame_width = width;
                header.frame_height = height;
                header.frame_stride = width * 4;
                header.frame_format = 0;
                header.frame_time_ms = GetTickCount64();
                header.frame_sequence = header.frame_sequence.wrapping_add(1);
                sequence = header.frame_sequence;
                let pose_dst = (state.view as *mut u8).add(280) as *mut PrismFramePose;
                let mut frame_pose = *pose;
                frame_pose.frame_sequence = sequence;
                core::ptr::write_unaligned(pose_dst, frame_pose);
            }
        }
        ReleaseMutex(state.mutex);
        sequence
    }
    #[cfg(not(windows))] { let _ = (src, src_pitch, width, height, rgba, pose); 0 }
}

#[no_mangle]
pub unsafe extern "C" fn prism_ipc_close() {
    #[cfg(windows)] {
        if let Ok(mut state) = IPC_STATE.lock() { close_ipc_locked(&mut state); }
    }
}

#[no_mangle]
pub unsafe extern "C" fn prism_pose_validate(pose: *const PrismPose) -> bool {
    let Some(pose) = pose.as_ref() else { return false };
    if pose.valid == 0 || !pose.position.iter().all(|v| v.is_finite()) || !pose.orientation.iter().all(|v| v.is_finite()) {
        return false;
    }
    let norm = pose.orientation.iter().map(|v| v * v).sum::<f32>();
    (0.25..4.0).contains(&norm)
}

#[no_mangle]
pub unsafe extern "C" fn prism_tracking_fresh(header_ptr: *const c_void) -> bool {
    #[cfg(windows)] {
        if header_ptr.is_null() { return false; }
        let header = &*(header_ptr as *const PrismSharedHeader);
        let now = GetTickCount64();
        header.magic == PRISM_MAGIC && header.version == PRISM_ABI_VERSION && header.client_connected != 0
            && header.server_heartbeat_ms != 0 && now >= header.server_heartbeat_ms && now - header.server_heartbeat_ms < 500
            && header.tracking_received_ms != 0 && now >= header.tracking_received_ms && now - header.tracking_received_ms < 500
            && header.tracking.version == 1
    }
    #[cfg(not(windows))] { let _ = header_ptr; false }
}

#[no_mangle]
pub unsafe extern "C" fn prism_tracking_pose(header_ptr: *const c_void, hand: i32, out: *mut DriverPoseData) -> bool {
    if header_ptr.is_null() || out.is_null() || !(-1..=1).contains(&hand) { return false; }
    let header = &*(header_ptr as *const PrismSharedHeader);
    let tracking_fresh = prism_tracking_fresh(header_ptr);
    let pose = if hand < 0 { &header.tracking.head } else { &header.tracking.hands[hand as usize] };
    let mut result = DriverPoseData { connected: (tracking_fresh && pose.valid != 0) as u32, valid: 0,
        position: [0.0; 3], orientation_xyzw: [0.0, 0.0, 0.0, 1.0] };
    let norm = pose.orientation.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>();
    let values_finite = pose.position.iter().all(|v| v.is_finite()) && pose.orientation.iter().all(|v| v.is_finite());
    if tracking_fresh && pose.valid != 0 && values_finite && norm > 0.25 && norm < 4.0 {
        let scale = sqrt(norm);
        result.valid = 1;
        result.orientation_xyzw = [pose.orientation[0] as f64 / scale, pose.orientation[1] as f64 / scale,
            pose.orientation[2] as f64 / scale, pose.orientation[3] as f64 / scale];
        for i in 0..3 { result.position[i] = pose.position[i] as f64; }
        if header.tracking.flags & 1 == 0 { result.position[1] += 1.6; }
    }
    *out = result;
    true
}

/// Convert one eye's four OpenXR tangent angles to OpenVR projection bounds.
/// Input order is left, right, up, down. Output order matches OpenVR: left, right, top, bottom.
/// OpenVR's historically named top is the negative-Y (down) bound; bottom is up.
#[no_mangle]
pub unsafe extern "C" fn prism_fov_to_projection(fov: *const c_float, out: *mut c_float) -> bool {
    let (Some(fov), Some(out)) = (fov.as_ref(), out.as_mut()) else { return false };
    let angles = core::slice::from_raw_parts(fov, 4);
    if !angles.iter().all(|v| v.is_finite() && v.abs() < 1.57)
        || angles[0] >= 0.0 || angles[1] <= 0.0 || angles[2] <= 0.0 || angles[3] >= 0.0
    { return false; }
    let values = unsafe { [tanf(angles[0]), tanf(angles[1]), tanf(angles[3]), tanf(angles[2])] };
    if !values.iter().all(|v| v.is_finite()) { return false; }
    core::ptr::copy_nonoverlapping(values.as_ptr(), out, 4);
    true
}

#[cfg(test)]
mod projection_tests {
    use super::prism_fov_to_projection;

    #[test]
    fn asymmetric_openxr_fov_preserves_openvr_vertical_bounds() {
        // atan([-1.2, 0.8, 1.5, -0.6]): different upper/lower extents
        // catch both an inverted projection and sign-only conversion.
        let angles = [-1.2f32.atan(), 0.8f32.atan(), 1.5f32.atan(), -0.6f32.atan()];
        let mut bounds = [0.0; 4];
        assert!(unsafe { prism_fov_to_projection(angles.as_ptr(), bounds.as_mut_ptr()) });
        for (actual, expected) in bounds.iter().zip([-1.2, 0.8, -0.6, 1.5]) {
            assert!((actual - expected).abs() < 0.00001);
        }
    }
}

/// Rank a hardware adapter. Callers try the selected adapter first and retry the
/// others when the compositor's shared texture cannot be opened on that device.
#[no_mangle]
pub extern "C" fn prism_adapter_rank(vendor: u32, is_arc: bool, dedicated_memory: u64) -> i32 {
    let class = match vendor {
        0x10de => 4, // NVIDIA GeForce
        0x1002 => 3, // AMD Radeon
        0x8086 if is_arc => 2, // Intel Arc
        0x8086 => 1, // Intel integrated graphics fallback
        _ => 0,
    };
    class * 1_000_000 + (dedicated_memory / (1024 * 1024)).min(999_999) as i32
}

/// Copy a mapped row into the Prism BGRA shared-frame format, converting RGBA
/// source pixels when needed. Row traversal is explicit and never reverses Y.
#[no_mangle]
pub unsafe extern "C" fn prism_copy_frame_rows(
    src: *const u8, src_pitch: usize, dst: *mut u8, width: usize, height: usize, rgba: bool,
) -> bool {
    if src.is_null() || dst.is_null() || width == 0 || height == 0 || src_pitch < width.saturating_mul(4) {
        return false;
    }
    for y in 0..height {
        let source_row = src.add(y * src_pitch);
        let dest_row = dst.add(y * width * 4);
        if rgba {
            for x in 0..width {
                let s = source_row.add(x * 4);
                let d = dest_row.add(x * 4);
                *d = *s.add(2);
                *d.add(1) = *s.add(1);
                *d.add(2) = *s;
                *d.add(3) = *s.add(3);
            }
        } else {
            core::ptr::copy_nonoverlapping(source_row, dest_row, width * 4);
        }
    }
    true
}
