//! Four-timestamp clock alignment. Client and host monotonic epochs are never
//! assumed equal. The lowest RTT observation in a short window bounds jitter.
use std::collections::VecDeque;
pub const PROBE: u32 = 110;
pub const REPLY: u32 = 111;
#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    pub host_minus_client_ns: i128,
    pub rtt_ns: u64,
    pub observed_ns: u64,
}
impl Estimate {
    pub fn align(&self, client_ns: u64, host_now: u64) -> Option<u64> {
        if host_now < self.observed_ns
            || host_now - self.observed_ns > 3_000_000_000
            || self.rtt_ns > 250_000_000
        {
            return None;
        }
        u64::try_from(client_ns as i128 + self.host_minus_client_ns).ok()
    }
}
#[derive(Default)]
pub struct Sync {
    pending: VecDeque<u64>,
    samples: VecDeque<Estimate>,
}
impl Sync {
    pub fn sent(&mut self, t1: u64) {
        if t1 > 0 {
            self.pending.push_back(t1);
            while self.pending.len() > 16 {
                self.pending.pop_front();
            }
        }
    }
    pub fn reply(&mut self, payload: &[u8], t4: u64) -> bool {
        if payload.len() != 24 {
            return false;
        }
        let t1 = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let t2 = u64::from_le_bytes(payload[8..16].try_into().unwrap());
        let t3 = u64::from_le_bytes(payload[16..24].try_into().unwrap());
        let Some(index) = self.pending.iter().position(|v| *v == t1) else {
            return false;
        };
        self.pending.remove(index);
        if t2 == 0 || t3 < t2 || t4 < t1 {
            return false;
        }
        let Some(rtt) = (t4 - t1).checked_sub(t3 - t2) else {
            return false;
        };
        if rtt > 250_000_000 {
            return false;
        }
        let offset = ((t1 as i128 - t2 as i128) + (t4 as i128 - t3 as i128)) / 2;
        self.samples.push_back(Estimate {
            host_minus_client_ns: offset,
            rtt_ns: rtt,
            observed_ns: t4,
        });
        while self.samples.len() > 16 {
            self.samples.pop_front();
        }
        true
    }
    pub fn estimate(&self, now: u64) -> Option<Estimate> {
        self.samples
            .iter()
            .copied()
            .filter(|s| now >= s.observed_ns && now - s.observed_ns <= 3_000_000_000)
            .min_by_key(|s| s.rtt_ns)
    }
}
#[cfg(windows)]
pub fn host_now_ns() -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(out: *mut i64) -> i32;
        fn QueryPerformanceFrequency(out: *mut i64) -> i32;
    }
    static FREQUENCY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let hz = *FREQUENCY.get_or_init(|| {
        let mut value = 0i64;
        if unsafe { QueryPerformanceFrequency(&mut value) } != 0 && value > 0 {
            value as u64
        } else {
            0
        }
    });
    let mut ticks = 0i64;
    if hz == 0 || unsafe { QueryPerformanceCounter(&mut ticks) } == 0 || ticks < 0 {
        return 0;
    }
    ((ticks as u128 * 1_000_000_000) / hz as u128) as u64
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn different_epochs_and_client_processing_are_accounted_for() {
        let mut s = Sync::default();
        s.sent(1_000_000_000);
        let payload = [1_000_000_000u64, 302_000_000, 303_000_000]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(s.reply(&payload, 1_005_000_000));
        let e = s.estimate(1_006_000_000).unwrap();
        assert_eq!(e.rtt_ns, 4_000_000);
        assert_eq!(e.align(333_000_000, 1_006_000_000), Some(1_033_000_000));
        assert!(e.align(333_000_000, 5_000_000_000).is_none());
        assert!(!s.reply(&payload, 1_005_000_000));
    }
    #[test]
    fn unsolicited_or_impossible_timestamps_are_rejected() {
        let mut s = Sync::default();
        let payload = [100u64, 200, 300]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(!s.reply(&payload, 150));
        s.sent(100);
        assert!(!s.reply(&payload, 150));
        assert!(s.estimate(150).is_none());
    }
}
