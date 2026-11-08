//! Published Aperture IPC v1 contract; optional v2 motion stays in the reserved area.
//! No ALVR/VDXR wire formats, source modules, or runtime imports are used.
pub const MAGIC: u32 = 0x52564150;
pub const PIXELS: usize = 4096;
pub const MAX_WIDTH: u32 = 4096;
pub const MAX_HEIGHT: u32 = 2048;
pub const MAPPING_BYTES: usize = PIXELS + MAX_WIDTH as usize * MAX_HEIGHT as usize * 4;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPose {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub valid: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Tracking {
    pub version: u32,
    pub flags: u32,
    pub sample_us: u64,
    pub head: RawPose,
    pub hands: [RawPose; 2],
    pub trigger: [f32; 2],
    pub grip: [f32; 2],
    pub stick: [f32; 4],
    pub buttons: [u32; 2],
    pub fov: [f32; 8],
    pub ipd: f32,
    pub refresh: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Header {
    pub magic: u32,
    pub version: u32,
    pub server_ms: u64,
    pub driver_ms: u64,
    pub tracking_ms: u64,
    pub frame_sequence: u64,
    pub frame_ms: u64,
    pub connected: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub ready: u32,
    pub format: u32,
    pub tracking: Tracking,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FramePose {
    pub magic: u32,
    pub version: u32,
    pub sequence: u64,
    pub head: RawPose,
    pub fov: [f32; 8],
    pub sample_us: u64,
}
pub use aperture_wire::{AlignedMotion, Motion, ALIGNED_OFFSET, MOTION_OFFSET};
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub header: Header,
    pub eye_width: u32,
    pub eye_height: u32,
    pub headset: u32,
    pub motion: Option<Motion>,
    pub aligned: Option<AlignedMotion>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            header: Header::default(),
            eye_width: 0,
            eye_height: 0,
            headset: 0,
            motion: None,
            aligned: None,
        }
    }
}
pub fn fresh(s: &Snapshot, now: u64) -> bool {
    let h = &s.header;
    h.magic == MAGIC
        && h.version == 1
        && h.connected != 0
        && h.tracking.version == 1
        && h.server_ms > 0
        && now >= h.server_ms
        && now - h.server_ms < 500
        && h.tracking_ms > 0
        && now >= h.tracking_ms
        && now - h.tracking_ms < 150
}
/// OpenXR up/down angles map to OpenVR's historically reversed top/bottom names.
pub fn projection(fov: [f32; 4]) -> Option<[f32; 4]> {
    if fov.iter().any(|v| !v.is_finite() || v.abs() >= 1.57)
        || fov[0] >= 0.
        || fov[1] <= 0.
        || fov[2] <= 0.
        || fov[3] >= 0.
    {
        return None;
    }
    Some([fov[0].tan(), fov[1].tan(), fov[3].tan(), fov[2].tan()])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_abi_is_preserved() {
        assert_eq!(std::mem::size_of::<RawPose>(), 32);
        assert_eq!(std::mem::size_of::<Tracking>(), 192);
        assert_eq!(std::mem::offset_of!(Header, tracking), 72);
        assert_eq!(std::mem::size_of::<Header>(), 264);
        assert_eq!(std::mem::size_of::<FramePose>(), 88);
        assert_eq!(std::mem::size_of::<Motion>(), 112);
        assert!(MOTION_OFFSET + 112 < PIXELS);
    }
    #[test]
    fn tracking_freshness_uses_host_receipt_not_foreign_clock() {
        let mut s = Snapshot::default();
        s.header.magic = MAGIC;
        s.header.version = 1;
        s.header.connected = 1;
        s.header.tracking.version = 1;
        s.header.server_ms = 1000;
        s.header.tracking_ms = 1000;
        s.header.tracking.sample_us = u64::MAX;
        assert!(fresh(&s, 1010));
        assert!(!fresh(&s, 1150));
        assert!(!fresh(&s, 999));
    }
    #[test]
    fn asymmetric_projection_keeps_device_angles() {
        let p = projection([-1.2f32.atan(), 0.8f32.atan(), 1.5f32.atan(), -0.6f32.atan()]).unwrap();
        for (a, b) in p.into_iter().zip([-1.2, 0.8, -0.6, 1.5]) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
