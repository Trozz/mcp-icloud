use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct LoginLimiter {
    max: usize,
    window: Duration,
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl LoginLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        let mut map = self.failures.lock().expect("limiter lock");
        let now = Instant::now();
        map.retain(|_, times| {
            times.retain(|t| now.duration_since(*t) < self.window);
            !times.is_empty()
        });
        map.get(&ip).is_some_and(|t| t.len() >= self.max)
    }

    pub fn record_failure(&self, ip: IpAddr) {
        self.failures
            .lock()
            .expect("limiter lock")
            .entry(ip)
            .or_default()
            .push(Instant::now());
    }
}

pub struct ReplayGuard {
    used: Mutex<HashMap<String, u64>>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            used: Mutex::new(HashMap::new()),
        }
    }

    pub fn first_use(&self, jti: &str, exp_unix: u64) -> bool {
        let now = jsonwebtoken::get_current_timestamp();
        let mut used = self.used.lock().expect("replay lock");
        used.retain(|_, exp| *exp >= now);
        used.insert(jti.to_string(), exp_unix).is_none()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.used.lock().unwrap().len()
    }
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_blocks_after_max_failures() {
        let l = LoginLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        for _ in 0..3 {
            assert!(!l.is_blocked(ip));
            l.record_failure(ip);
        }
        assert!(l.is_blocked(ip));
        assert!(!l.is_blocked(other));
    }

    #[test]
    fn limiter_forgets_after_window() {
        let l = LoginLimiter::new(1, Duration::from_millis(20));
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        l.record_failure(ip);
        assert!(l.is_blocked(ip));
        std::thread::sleep(Duration::from_millis(40));
        assert!(!l.is_blocked(ip));
    }

    #[test]
    fn replay_guard_allows_first_use_only() {
        let g = ReplayGuard::new();
        let exp = jsonwebtoken::get_current_timestamp() + 60;
        assert!(g.first_use("jti-1", exp));
        assert!(!g.first_use("jti-1", exp));
        assert!(g.first_use("jti-2", exp));
    }

    #[test]
    fn replay_guard_prunes_expired() {
        let g = ReplayGuard::new();
        let past = jsonwebtoken::get_current_timestamp() - 10;
        assert!(g.first_use("old", past));
        g.first_use("trigger-prune", past + 1000);
        assert_eq!(g.len(), 1);
    }
}
