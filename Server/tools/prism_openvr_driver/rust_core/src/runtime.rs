use crate::{prism_fov_to_projection, prism_ipc_beat, prism_ipc_close, prism_ipc_device_resolution,
    prism_ipc_device_type, prism_ipc_open, prism_ipc_read, prism_tracking_fresh, prism_tracking_pose,
    DriverPoseData, PrismSharedHeader};
use std::sync::{Mutex, OnceLock};

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_identity(device_type: u32, hand: i32, model: *mut i8, model_capacity: usize, render: *mut i8, render_capacity: usize) -> bool {
    if !(-1..=1).contains(&hand) || model.is_null() || render.is_null() || model_capacity == 0 || render_capacity == 0 { return false; }
    let (headset, controller, render_model) = match device_type {
        1 => ("Meta Quest", "Meta Touch", "oculus_quest_controller"),
        2 => ("Meta Quest 2", "Meta Touch", "oculus_quest2_controller"),
        3 => ("Meta Quest Pro", "Meta Touch Pro", "oculus_quest_pro_controller"),
        4 => ("Meta Quest 3", "Meta Touch Plus", "oculus_quest_plus_controller"),
        5 => ("Meta Quest 3S", "Meta Touch Plus", "oculus_quest_plus_controller"),
        _ => ("Meta Quest (connecting)", "Meta Touch", "oculus_quest2_controller"),
    };
    let model_text = if hand < 0 { headset.to_string() } else { format!("{headset} {controller} ({})", if hand == 0 { "Left" } else { "Right" }) };
    let render_text = if hand < 0 { String::new() } else { format!("{render_model}_{}", if hand == 0 { "left" } else { "right" }) };
    let model_bytes = model_text.as_bytes();
    let render_bytes = render_text.as_bytes();
    let model_count = model_bytes.len().min(model_capacity - 1);
    let render_count = render_bytes.len().min(render_capacity - 1);
    core::ptr::copy_nonoverlapping(model_bytes.as_ptr(), model.cast::<u8>(), model_count);
    core::ptr::write(model.add(model_count), 0);
    core::ptr::copy_nonoverlapping(render_bytes.as_ptr(), render.cast::<u8>(), render_count);
    core::ptr::write(render.add(render_count), 0);
    true
}

#[repr(C)]
#[derive(Default)]
pub struct TickResult {
    pub connected: u32,
    pub connection_changed: u32,
    pub device_type: u32,
    pub recommendation_changed: u32,
    pub recommended_width: u32,
    pub recommended_height: u32,
    pub fov_changed: u32,
    pub fov: [f32; 8],
    pub ipd_changed: u32,
    pub ipd: f32,
    pub refresh_changed: u32,
    pub refresh_rate: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct InputResult { pub buttons: u32, pub axes: [f32; 4] }

#[repr(C)]
pub struct ControllerConfig {
    pub role_hint: u32,
    pub controller_type: [i8; 32],
    pub profile: [i8; 64],
    pub buttons: [[i8; 48]; 4],
    pub axes: [[i8; 48]; 4],
    pub scalar_units: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
pub struct DisplayProperties {
    pub has_virtual_display: u32,
    pub wireless: u32,
    pub is_on_desktop: u32,
    pub direct_mode: u32,
    pub universe_id: u64,
    pub refresh_rate: f32,
    pub ipd: f32,
    pub seconds_to_photons: f32,
}

fn copy_fixed<const N: usize>(source: &str, target: &mut [i8; N]) {
    let bytes = source.as_bytes();
    let count = bytes.len().min(N - 1);
    for i in 0..count { target[i] = bytes[i] as i8; }
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_controller_config(hand: i32, out: *mut ControllerConfig) -> bool {
    if !(0..=1).contains(&hand) || out.is_null() { return false; }
    let mut config = ControllerConfig { role_hint: if hand == 0 { 1 } else { 2 }, controller_type: [0;32], profile: [0;64], buttons: [[0;48];4], axes: [[0;48];4], scalar_units: [0,0,1,1] };
    copy_fixed("oculus_touch", &mut config.controller_type);
    copy_fixed("{oculus}/input/touch_profile.json", &mut config.profile);
    let buttons = if hand == 0 { ["/input/x/click", "/input/y/click", "/input/system/click", "/input/joystick/click"] }
        else { ["/input/a/click", "/input/b/click", "/input/system/click", "/input/joystick/click"] };
    let axes = ["/input/trigger/value", "/input/grip/value", "/input/joystick/x", "/input/joystick/y"];
    for i in 0..4 { copy_fixed(buttons[i], &mut config.buttons[i]); copy_fixed(axes[i], &mut config.axes[i]); }
    *out = config;
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_display_properties(redirect: bool, out: *mut DisplayProperties) -> bool {
    if out.is_null() { return false; }
    *out = DisplayProperties { has_virtual_display: 1, wireless: 1, is_on_desktop: 0, direct_mode: 0,
        universe_id: if redirect { 0 } else { 42 }, refresh_rate: if redirect { 0.0 } else { 72.0 },
        ipd: if redirect { 0.0 } else { 0.064 }, seconds_to_photons: 0.0 };
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_distortion(u: f32, v: f32, out: *mut f32) -> bool {
    if out.is_null() || !u.is_finite() || !v.is_finite() { return false; }
    *out=u;*out.add(1)=v;*out.add(2)=u;*out.add(3)=v;*out.add(4)=u;*out.add(5)=v;true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_inverse_distortion(u: f32, v: f32, out: *mut f32) -> bool {
    if out.is_null() || !u.is_finite() || !v.is_finite() { return false; }
    *out=u;*out.add(1)=v;true
}

struct State {
    snapshot: PrismSharedHeader,
    has_snapshot: bool,
    connected: bool,
    device_type: u32,
    eye_width: u32,
    eye_height: u32,
    projection: [f32; 8],
    ipd: f32,
    refresh_rate: u32,
    vsync_epoch_ms: u64,
}

impl State {
    fn new() -> Self {
        Self { snapshot: unsafe { core::mem::zeroed() }, has_snapshot: false, connected: false,
            device_type: 0, eye_width: 1440, eye_height: 1600,
            projection: [-1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0], ipd: 0.0, refresh_rate: 72, vsync_epoch_ms: 0 }
    }
}

fn state() -> &'static Mutex<State> { static STATE: OnceLock<Mutex<State>> = OnceLock::new(); STATE.get_or_init(|| Mutex::new(State::new())) }

fn fallback_eye_size(device_type: u32) -> (u32, u32) {
    match device_type { 1 => (1440,1600), 2 => (1832,1920), 3 => (1800,1920), 4 => (2064,2208), 5 => (1832,1920), _ => (1440,1600) }
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_start() -> bool {
    if !prism_ipc_open() { return false; }
    let mut snapshot = core::mem::MaybeUninit::<PrismSharedHeader>::uninit();
    if !prism_ipc_read(snapshot.as_mut_ptr().cast(), 0) { prism_ipc_close(); return false; }
    let snapshot = snapshot.assume_init();
    let device_type = prism_ipc_device_type();
    let (mut width, mut height) = fallback_eye_size(device_type);
    prism_ipc_device_resolution(&mut width, &mut height);
    if let Ok(mut s) = state().lock() {
        s.snapshot = snapshot;
        s.has_snapshot = true;
        s.device_type = device_type;
        s.eye_width = width;
        s.eye_height = height;
        s.connected = prism_tracking_fresh((&s.snapshot as *const PrismSharedHeader).cast());
        #[cfg(windows)] { s.vsync_epoch_ms = crate::GetTickCount64(); }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_cleanup() {
    prism_ipc_beat(false);
    prism_ipc_close();
    crate::d3d_capture::reset_capture();
    if let Ok(mut s) = state().lock() { *s = State::new(); }
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_tick(out: *mut TickResult) -> bool {
    if out.is_null() { return false; }
    prism_ipc_beat(true);
    let mut incoming = core::mem::MaybeUninit::<PrismSharedHeader>::uninit();
    let got_snapshot = prism_ipc_read(incoming.as_mut_ptr().cast(), 0);
    let mut result = TickResult::default();
    let Ok(mut s) = state().lock() else { return false };
    if got_snapshot { s.snapshot = incoming.assume_init(); s.has_snapshot = true; }
    let was_connected = s.connected;
    s.connected = s.has_snapshot && prism_tracking_fresh((&s.snapshot as *const PrismSharedHeader).cast());
    result.connected = s.connected as u32;
    result.connection_changed = (was_connected != s.connected) as u32;
    let old_type = s.device_type;
    let device_type = prism_ipc_device_type();
    if device_type != 0 { s.device_type = device_type; }
    let type_changed = old_type != s.device_type;
    result.device_type = s.device_type;
    let (mut width, mut height) = fallback_eye_size(s.device_type);
    let has_published_resolution = prism_ipc_device_resolution(&mut width, &mut height);
    if (has_published_resolution || type_changed) && (width != s.eye_width || height != s.eye_height) {
        s.eye_width = width; s.eye_height = height; result.recommendation_changed = 1;
    }
    result.recommended_width = s.eye_width;
    result.recommended_height = s.eye_height;
    if s.connected {
        let mut projection = [0.0f32; 8];
        if prism_fov_to_projection(s.snapshot.tracking.fov.as_ptr(), projection.as_mut_ptr())
            && prism_fov_to_projection(s.snapshot.tracking.fov.as_ptr().add(4), projection.as_mut_ptr().add(4))
        {
            let changed = (0..8).any(|i| (projection[i] - s.projection[i]).abs() > 0.001);
            if changed { s.projection = projection; result.fov_changed = 1; result.fov = s.snapshot.tracking.fov; }
        }
        let ipd = s.snapshot.tracking.ipd;
        if ipd.is_finite() && ipd > 0.04 && ipd < 0.09 && ipd != s.ipd {
            s.ipd = ipd; result.ipd_changed = 1; result.ipd = ipd;
        }
        let refresh = s.snapshot.tracking.refresh_rate;
        if (60..=144).contains(&refresh) && refresh != s.refresh_rate {
            s.refresh_rate = refresh; result.refresh_changed = 1; result.refresh_rate = refresh;
        }
    }
    *out = result;
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_pose(hand: i32, out: *mut DriverPoseData) -> bool {
    if out.is_null() { return false; }
    if (0..=1).contains(&hand) {
        // Controller queries should consume the latest received grip sample,
        // rather than waiting for the next provider RunFrame snapshot. Never
        // wait for the compositor's pixel-copy mutex on the pose query path.
        let mut latest = core::mem::MaybeUninit::<PrismSharedHeader>::uninit();
        if prism_ipc_read(latest.as_mut_ptr().cast(), 0) {
            return prism_tracking_pose(latest.as_ptr().cast(), hand, out);
        }
    }
    let Ok(s) = state().lock() else { return false };
    if !s.has_snapshot { return false; }
    prism_tracking_pose((&s.snapshot as *const PrismSharedHeader).cast(), hand, out)
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_input(hand: i32, out: *mut InputResult) -> bool {
    if out.is_null() || !(0..=1).contains(&hand) { return false; }
    let Ok(s) = state().lock() else { return false };
    if !s.has_snapshot { return false; }
    let mut pose = DriverPoseData::default();
    prism_tracking_pose((&s.snapshot as *const PrismSharedHeader).cast(), hand, &mut pose);
    let mut result = InputResult::default();
    if pose.valid != 0 {
        result.buttons = s.snapshot.tracking.buttons[hand as usize];
        let i = hand as usize;
        let values = [s.snapshot.tracking.trigger[i], s.snapshot.tracking.grip[i], s.snapshot.tracking.stick[i*2], s.snapshot.tracking.stick[i*2+1]];
        for j in 0..4 { result.axes[j] = if values[j].is_finite() { values[j].clamp(if j < 2 {0.0} else {-1.0}, 1.0) } else {0.0}; }
    }
    *out = result;
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_projection(eye: u32, out: *mut f32) -> bool {
    if eye > 1 || out.is_null() { return false; }
    let Ok(s) = state().lock() else { return false };
    core::ptr::copy_nonoverlapping(s.projection.as_ptr().add(eye as usize * 4), out, 4);
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_recommended(out_width: *mut u32, out_height: *mut u32) -> bool {
    if out_width.is_null() || out_height.is_null() { return false; }
    let Ok(s) = state().lock() else { return false };
    *out_width = s.eye_width; *out_height = s.eye_height;
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_frame_pose(out: *mut crate::PrismFramePose) -> bool {
    if out.is_null() { return false; }
    let Ok(s) = state().lock() else { return false };
    let mut pose = crate::PrismFramePose { magic: 0x4650_5650, version: 1, frame_sequence: 0,
        head: crate::PrismPose { position: [0.0; 3], orientation: [0.0; 4], valid: 0 },
        fov: [0.0; 8], sample_time_us: 0 };
    if s.has_snapshot && s.connected && s.snapshot.tracking.head.valid != 0 {
        pose.head = s.snapshot.tracking.head;
        pose.fov = s.snapshot.tracking.fov;
        pose.sample_time_us = s.snapshot.tracking.sample_time_us;
    }
    *out = pose;
    true
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_device_type() -> u32 {
    state().lock().map(|s| s.device_type).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn prism_runtime_vsync(seconds_since_vsync: *mut f32, frame_counter: *mut u64) -> bool {
    if seconds_since_vsync.is_null() || frame_counter.is_null() { return false; }
    let Ok(s) = state().lock() else { return false };
    if s.vsync_epoch_ms == 0 || !(60..=144).contains(&s.refresh_rate) { return false; }
    #[cfg(windows)] {
        let now = crate::GetTickCount64();
        let interval_ms = 1000.0 / s.refresh_rate as f64;
        let elapsed_ms = now.saturating_sub(s.vsync_epoch_ms) as f64;
        *seconds_since_vsync = (elapsed_ms.rem_euclid(interval_ms) / 1000.0) as f32;
        *frame_counter = (elapsed_ms / interval_ms) as u64;
        true
    }
    #[cfg(not(windows))] { false }
}
