//! A single replaceable *raw* presentation slot. Compressed inter-frame packets
//! are never dropped here: this queue is upstream of capture/encoding.
use std::sync::{Condvar, Mutex};
pub enum Poll<T> {
    Value(T),
    Timeout,
    Closed,
}
pub struct Latest<T> {
    state: Mutex<(Option<T>, bool)>,
    wake: Condvar,
}
impl<T> Latest<T> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new((None, false)),
            wake: Condvar::new(),
        }
    }
    pub fn publish(&self, value: T) -> bool {
        let Ok(mut s) = self.state.lock() else {
            return false;
        };
        if s.1 {
            return false;
        }
        s.0 = Some(value);
        self.wake.notify_one();
        true
    }
    #[cfg(test)]
    pub fn receive(&self) -> Option<T> {
        let mut s = self.state.lock().ok()?;
        while s.0.is_none() && !s.1 {
            s = self.wake.wait(s).ok()?;
        }
        if s.1 {
            None
        } else {
            s.0.take()
        }
    }
    pub fn receive_timeout(&self, wait: std::time::Duration) -> Poll<T> {
        let Ok(mut s) = self.state.lock() else {
            return Poll::Closed;
        };
        if s.0.is_none() && !s.1 {
            let Ok((state, _)) = self.wake.wait_timeout(s, wait) else {
                return Poll::Closed;
            };
            s = state;
        }
        if s.1 {
            Poll::Closed
        } else if let Some(value) = s.0.take() {
            Poll::Value(value)
        } else {
            Poll::Timeout
        }
    }
    pub fn close(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.1 = true;
            s.0 = None;
            self.wake.notify_all();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn congestion_retains_only_latest_raw_frame() {
        let q = Latest::new();
        for id in 1..100 {
            assert!(q.publish(id));
        }
        assert_eq!(q.receive(), Some(99));
        q.close();
        assert_eq!(q.receive(), None);
        assert!(!q.publish(100));
    }
    #[test]
    fn close_wakes_waiting_capture_worker() {
        let q = std::sync::Arc::new(Latest::<u64>::new());
        let other = q.clone();
        let join = std::thread::spawn(move || other.receive());
        q.close();
        assert_eq!(join.join().unwrap(), None);
    }
}
