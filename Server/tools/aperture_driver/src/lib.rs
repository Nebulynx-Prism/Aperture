//! Aperture-owned OpenVR driver core. Public SDK/Windows APIs only.
mod capture;
mod contract;
mod ipc;
mod latest;
pub mod math;
mod protocol;
use protocol::*;
use std::{
    ffi::{c_char, CString},
    sync::{Arc, Mutex},
    time::Instant,
};
type Logger = Option<extern "C" fn(*const c_char)>;
static LOG: Mutex<Logger> = Mutex::new(None);
pub(crate) fn log(message: &str) {
    if let Ok(log) = LOG.lock() {
        if let Some(callback) = *log {
            if let Ok(text) = CString::new(message) {
                callback(text.as_ptr());
            }
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DriverPose {
    pub connected: u32,
    pub valid: u32,
    pub position: [f64; 3],
    pub orientation: [f64; 4],
    pub linear_velocity: [f64; 3],
    pub angular_velocity: [f64; 3],
    pub time_offset: f64,
}
impl Default for DriverPose {
    fn default() -> Self {
        Self {
            connected: 0,
            valid: 0,
            position: [0.; 3],
            orientation: [0., 0., 0., 1.],
            linear_velocity: [0.; 3],
            angular_velocity: [0.; 3],
            time_offset: 0.,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Display {
    pub panel_width: u32,
    pub panel_height: u32,
    pub eye_width: u32,
    pub eye_height: u32,
    pub refresh: u32,
    pub headset: u32,
    pub ipd: f32,
    pub projection: [f32; 8],
    pub adapter_luid: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Input {
    pub buttons: u32,
    pub trigger: f32,
    pub grip: f32,
    pub stick: [f32; 2],
}
struct Engine {
    mapping: Arc<ipc::Mapping>,
    capture: capture::Capture,
    snapshot: Snapshot,
    epoch: Instant,
    adapter_luid: u64,
}
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
fn observed_display(s: &Snapshot) -> Option<Display> {
    let h = &s.header;
    let t = &h.tracking;
    if !(512..=4096).contains(&s.eye_width)
        || !(256..=4096).contains(&s.eye_height)
        || !(512..=4096).contains(&h.width)
        || !(256..=2048).contains(&h.height)
        || h.width % 2 != 0
        || !(60..=144).contains(&t.refresh)
        || !t.ipd.is_finite()
        || !(0.04..=0.09).contains(&t.ipd)
    {
        return None;
    }
    let left = projection(t.fov[0..4].try_into().ok()?)?;
    let right = projection(t.fov[4..8].try_into().ok()?)?;
    let mut bounds = [0.; 8];
    bounds[..4].copy_from_slice(&left);
    bounds[4..].copy_from_slice(&right);
    Some(Display {
        panel_width: h.width,
        panel_height: h.height,
        eye_width: s.eye_width,
        eye_height: s.eye_height,
        refresh: t.refresh,
        headset: s.headset,
        ipd: t.ipd,
        projection: bounds,
        adapter_luid: 0,
    })
}
fn observed_pose(s: &Snapshot, hand: i32, now: u64) -> DriverPose {
    observed_pose_at(s, hand, now, aperture_wire::clock::host_now_ns())
}
fn observed_pose_at(s: &Snapshot, hand: i32, now: u64, host_ns: u64) -> DriverPose {
    let mut output = DriverPose::default();
    if !(-1..=1).contains(&hand) {
        return output;
    }
    output.connected = (s.header.connected != 0) as u32;
    if !fresh(s, now) {
        return output;
    }
    let raw = if hand < 0 {
        s.header.tracking.head
    } else {
        s.header.tracking.hands[hand as usize]
    };
    if raw.valid != 1 {
        return output;
    }
    let Some(pose) = math::finite_pose(math::Pose {
        position: raw.position.map(f64::from),
        orientation: raw.orientation.map(f64::from),
    }) else {
        return output;
    };
    let origin = math::Pose {
        position: if s.header.tracking.flags & 1 == 0 {
            [0., 1.6, 0.]
        } else {
            [0.; 3]
        },
        ..math::Pose::default()
    };
    let located = math::compose(origin, pose);
    output.position = located.position;
    output.orientation = located.orientation;
    // Legacy LOCAL-space compatibility. The new protocol will explicitly carry
    // floor origin and motion timing; never compare client and PC clocks here.
    output.valid = 1;
    if let Some(motion) = s.motion {
        let device = if hand < 0 { 0 } else { hand as usize + 1 };
        if motion.flags[device] & 1 != 0 {
            output.linear_velocity = std::array::from_fn(|i| motion.velocities[device][i] as f64);
        }
        if motion.flags[device] & 2 != 0 {
            output.angular_velocity =
                std::array::from_fn(|i| motion.velocities[device][i + 3] as f64);
        }
        output.time_offset =
            motion.target_lead_ns as f64 / 1e9 - (now - s.header.tracking_ms) as f64 / 1000.;
        if let Some(aligned) = s.aligned {
            if aligned.valid_for(motion.sample_us, host_ns) {
                let offset = (aligned.target_host_ns as i128 - host_ns as i128) as f64 / 1e9;
                if offset.abs() <= 0.5 {
                    output.time_offset = offset;
                }
            }
        }
    }
    // Legacy samples do not invent velocity or prediction. The optional motion
    // extension carries actual OpenXR velocity and a converted pose target time.
    output
}
#[no_mangle]
pub extern "C" fn aperture_start(logger: Logger) -> bool {
    if let Ok(mut slot) = LOG.lock() {
        *slot = logger;
    }
    let Ok(mut state) = ENGINE.lock() else {
        return false;
    };
    if state.is_some() {
        return true;
    }
    let Some(mapping) = ipc::Mapping::open() else {
        log("Aperture: server mapping unavailable");
        return false;
    };
    let Some((width, height)) = contract::output_dimensions() else {
        log("Aperture: explicit display contract missing or invalid");
        return false;
    };
    mapping.configure_output(width, height);
    let Some(snapshot) = mapping.snapshot() else {
        log("Aperture: server mapping busy or incompatible");
        return false;
    };
    let mapping = Arc::new(mapping);
    let Some(capture) = capture::Capture::new(mapping.clone()) else {
        log("Aperture: cannot create capture worker");
        return false;
    };
    let adapter_luid = unsafe {
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
        CreateDXGIFactory1::<IDXGIFactory1>()
            .and_then(|f| f.EnumAdapters1(0))
            .and_then(|a| a.GetDesc1())
            .map(|d| {
                u64::from(d.AdapterLuid.LowPart) | ((d.AdapterLuid.HighPart as u32 as u64) << 32)
            })
            .unwrap_or(0)
    };
    mapping.beat(true);
    *state = Some(Engine {
        mapping,
        capture,
        snapshot,
        epoch: Instant::now(),
        adapter_luid,
    });
    log("Aperture: independent Rust core initialized; no foreign VR runtime loaded");
    true
}
#[no_mangle]
pub extern "C" fn aperture_stop() {
    let old = ENGINE.lock().ok().and_then(|mut s| s.take());
    if let Some(engine) = old {
        engine.mapping.beat(false);
        drop(engine);
    }
    if let Ok(mut slot) = LOG.lock() {
        *slot = None;
    }
}
#[no_mangle]
pub extern "C" fn aperture_tick() -> bool {
    let Ok(mut state) = ENGINE.try_lock() else {
        return false;
    };
    let Some(engine) = state.as_mut() else {
        return false;
    };
    if let Some(snapshot) = engine.mapping.snapshot() {
        engine.snapshot = snapshot;
    }
    engine.mapping.beat(true);
    true
}
#[no_mangle]
pub unsafe extern "C" fn aperture_pose(hand: i32, out: *mut DriverPose) -> bool {
    if out.is_null() || !(-1..=1).contains(&hand) {
        return false;
    }
    let Ok(mut state) = ENGINE.try_lock() else {
        *out = DriverPose::default();
        return false;
    };
    let Some(engine) = state.as_mut() else {
        *out = DriverPose::default();
        return false;
    };
    if let Some(snapshot) = engine.mapping.snapshot() {
        engine.snapshot = snapshot;
    }
    *out = observed_pose(&engine.snapshot, hand, ipc::now_ms());
    true
}
#[no_mangle]
pub unsafe extern "C" fn aperture_display(out: *mut Display) -> bool {
    if out.is_null() {
        return false;
    }
    let Ok(state) = ENGINE.try_lock() else {
        return false;
    };
    let Some(engine) = state.as_ref() else {
        return false;
    };
    let Some(mut display) = observed_display(&engine.snapshot) else {
        return false;
    };
    display.adapter_luid = engine.adapter_luid;
    *out = display;
    true
}
#[no_mangle]
pub unsafe extern "C" fn aperture_input(hand: u32, out: *mut Input) -> bool {
    if hand > 1 || out.is_null() {
        return false;
    }
    *out = Input::default();
    let Ok(state) = ENGINE.try_lock() else {
        return false;
    };
    let Some(engine) = state.as_ref() else {
        return false;
    };
    if observed_pose(&engine.snapshot, hand as i32, ipc::now_ms()).valid == 0 {
        return true;
    }
    let t = engine.snapshot.header.tracking;
    let i = hand as usize;
    let axis = |v: f32, low: f32| if v.is_finite() { v.clamp(low, 1.) } else { 0. };
    *out = Input {
        buttons: t.buttons[i] & 15,
        trigger: axis(t.trigger[i], 0.),
        grip: axis(t.grip[i], 0.),
        stick: [axis(t.stick[i * 2], -1.), axis(t.stick[i * 2 + 1], -1.)],
    };
    true
}
#[no_mangle]
pub extern "C" fn aperture_present(handle: u64, frame: u64) {
    if handle == 0 {
        return;
    }
    let Ok(state) = ENGINE.try_lock() else {
        return;
    };
    let Some(engine) = state.as_ref() else {
        return;
    };
    engine.capture.present(capture::Presentation {
        handle,
        frame,
        tracking: engine.snapshot.header.tracking,
    });
}
#[no_mangle]
pub unsafe extern "C" fn aperture_vsync(seconds: *mut f32, frame: *mut u64) -> bool {
    if seconds.is_null() || frame.is_null() {
        return false;
    }
    let Ok(state) = ENGINE.try_lock() else {
        return false;
    };
    let Some(engine) = state.as_ref() else {
        return false;
    };
    if !fresh(&engine.snapshot, ipc::now_ms()) {
        return false;
    }
    let hz = engine.snapshot.header.tracking.refresh;
    if !(60..=144).contains(&hz) {
        return false;
    }
    let periods = engine.epoch.elapsed().as_secs_f64() * hz as f64;
    *frame = periods.floor() as u64;
    *seconds = (periods.fract() / hz as f64) as f32;
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_or_stale_controller_is_never_reported_tracked() {
        let mut s = Snapshot::default();
        s.header.magic = MAGIC;
        s.header.version = 1;
        s.header.connected = 1;
        s.header.server_ms = 1000;
        s.header.tracking_ms = 1000;
        s.header.tracking.version = 1;
        s.header.tracking.hands[0] = RawPose {
            position: [1., 2., 3.],
            orientation: [0., 0., 0., 1.],
            valid: 1,
        };
        assert_eq!(observed_pose(&s, 0, 1010).valid, 1);
        assert_eq!(observed_pose(&s, 0, 1150).valid, 0);
        s.header.tracking.hands[0].orientation = [0.; 4];
        assert_eq!(observed_pose(&s, 0, 1010).valid, 0);
    }
    #[test]
    fn unknown_display_does_not_create_fake_default_resolution() {
        assert!(observed_display(&Snapshot::default()).is_none());
    }
    #[test]
    fn actual_motion_is_forwarded_without_extra_extrapolation() {
        let mut s = Snapshot::default();
        s.header.magic = MAGIC;
        s.header.version = 1;
        s.header.connected = 1;
        s.header.server_ms = 1000;
        s.header.tracking_ms = 1000;
        s.header.tracking.version = 1;
        s.header.tracking.flags = 1;
        s.header.tracking.sample_us = 99;
        s.header.tracking.hands[0] = RawPose {
            position: [1., 2., 3.],
            orientation: [0., 0., 0., 1.],
            valid: 1,
        };
        let mut m = aperture_wire::Motion {
            magic: aperture_wire::MOTION_MAGIC,
            version: 1,
            sample_us: 99,
            target_lead_ns: 20_000_000,
            flags: [3; 3],
            timing_valid: 1,
            ..Default::default()
        };
        m.velocities[1] = [1., 2., 3., 4., 5., 6.];
        s.motion = Some(m);
        let p = observed_pose(&s, 0, 1010);
        assert_eq!(p.position, [1., 2., 3.]);
        assert_eq!(p.linear_velocity, [1., 2., 3.]);
        assert_eq!(p.angular_velocity, [4., 5., 6.]);
        assert!((p.time_offset - 0.01).abs() < 1e-9);
        s.aligned = Some(aperture_wire::AlignedMotion {
            magic: 0x544d5641,
            version: 1,
            sample_us: 99,
            target_host_ns: 1_002_000_000,
            written_host_ns: 1_000_000_000,
            rtt_ns: 4_000_000,
        });
        let aligned = observed_pose_at(&s, 0, 1010, 1_012_000_000);
        assert!((aligned.time_offset + 0.01).abs() < 1e-9);
        assert_eq!(aligned.position, [1., 2., 3.]);
    }
}
