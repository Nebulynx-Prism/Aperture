//! Aperture-owned, endian-explicit protocol definitions, shared by driver/server.
pub mod clock;
pub const ALIGNED_OFFSET: usize = 480;
pub const ALIGNED_BYTES: usize = 40;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct AlignedMotion {
    pub magic: u32,
    pub version: u32,
    pub sample_us: u64,
    pub target_host_ns: u64,
    pub written_host_ns: u64,
    pub rtt_ns: u64,
}
impl AlignedMotion {
    pub fn encode(&self) -> [u8; ALIGNED_BYTES] {
        let mut out = [0u8; ALIGNED_BYTES];
        out[..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.version.to_le_bytes());
        for (i, value) in [
            self.sample_us,
            self.target_host_ns,
            self.written_host_ns,
            self.rtt_ns,
        ]
        .into_iter()
        .enumerate()
        {
            out[8 + i * 8..16 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        out
    }
    pub fn valid_for(&self, sample: u64, now: u64) -> bool {
        self.magic == 0x544d5641
            && self.version == 1
            && self.sample_us == sample
            && self.target_host_ns > 0
            && self.written_host_ns > 0
            && now >= self.written_host_ns
            && now - self.written_host_ns < 500_000_000
            && self.rtt_ns <= 250_000_000
    }
}
pub const MOTION_BYTES: usize = 112;
pub const MOTION_OFFSET: usize = 368;
pub const MOTION_MESSAGE: u32 = 109;
pub const MOTION_MAGIC: u32 = 0x4d505641;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Motion {
    pub magic: u32,
    pub version: u32,
    pub sample_us: u64,
    pub target_lead_ns: i64,
    pub flags: [u32; 3],
    pub timing_valid: u32,
    pub velocities: [[f32; 6]; 3],
}
impl Motion {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != MOTION_BYTES {
            return None;
        }
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let mut m = Self {
            magic: u32_at(0),
            version: u32_at(4),
            sample_us: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            target_lead_ns: i64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            flags: [u32_at(24), u32_at(28), u32_at(32)],
            timing_valid: u32_at(36),
            velocities: [[0.; 6]; 3],
        };
        for device in 0..3 {
            for axis in 0..6 {
                m.velocities[device][axis] = f32::from_bits(u32_at(40 + (device * 6 + axis) * 4));
            }
        }
        if m.valid() {
            Some(m)
        } else {
            None
        }
    }
    pub fn valid(&self) -> bool {
        self.magic == MOTION_MAGIC
            && self.version == 1
            && self.sample_us != 0
            && self.timing_valid == 1
            && (0..=200_000_000).contains(&self.target_lead_ns)
            && self.flags.iter().all(|v| v & !3 == 0)
            && self.velocities.iter().flatten().all(|v| v.is_finite())
    }
    pub fn encode(&self) -> [u8; MOTION_BYTES] {
        let mut b = [0u8; MOTION_BYTES];
        b[0..4].copy_from_slice(&self.magic.to_le_bytes());
        b[4..8].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.sample_us.to_le_bytes());
        b[16..24].copy_from_slice(&self.target_lead_ns.to_le_bytes());
        for i in 0..3 {
            b[24 + i * 4..28 + i * 4].copy_from_slice(&self.flags[i].to_le_bytes());
        }
        b[36..40].copy_from_slice(&self.timing_valid.to_le_bytes());
        for device in 0..3 {
            for axis in 0..6 {
                let i = 40 + (device * 6 + axis) * 4;
                b[i..i + 4].copy_from_slice(&self.velocities[device][axis].to_le_bytes());
            }
        }
        b
    }
}
const _: () = assert!(std::mem::size_of::<Motion>() == MOTION_BYTES);
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Motion {
        Motion {
            magic: MOTION_MAGIC,
            version: 1,
            sample_us: 123,
            target_lead_ns: 11_000_000,
            flags: [3; 3],
            timing_valid: 1,
            velocities: [[1., 2., 3., 4., 5., 6.]; 3],
        }
    }
    #[test]
    fn wire_roundtrip_preserves_all_three_devices() {
        let m = sample();
        assert_eq!(Motion::decode(&m.encode()), Some(m));
    }
    #[test]
    fn truncated_or_foreign_motion_is_rejected() {
        let m = sample();
        assert!(Motion::decode(&m.encode()[..111]).is_none());
        let mut b = m.encode();
        b[0] ^= 1;
        assert!(Motion::decode(&b).is_none());
    }
    #[test]
    fn invalid_motion_is_never_used_for_prediction() {
        for m in [
            Motion {
                target_lead_ns: -1,
                ..sample()
            },
            Motion {
                target_lead_ns: 200_000_001,
                ..sample()
            },
            Motion {
                flags: [4; 3],
                ..sample()
            },
            Motion {
                timing_valid: 0,
                ..sample()
            },
            Motion {
                velocities: [[f32::NAN; 6]; 3],
                ..sample()
            },
        ] {
            assert!(Motion::decode(&m.encode()).is_none());
        }
    }
}
