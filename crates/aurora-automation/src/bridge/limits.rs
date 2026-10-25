//! The two throttles: a failed-authentication lockout and a request rate
//! limit. Both take the time as an argument so tests need no sleeping.

use std::time::{Duration, Instant};

/// After this many consecutive failed handshakes the listener starts to
/// refuse connections for a growing time.
pub const FREE_FAILURES: u32 = 5;
/// The first lockout, doubling with each further failure.
pub const LOCKOUT_BASE: Duration = Duration::from_secs(1);
/// The longest lockout (small on purpose: a lockout is also a denial of
/// service against the real controller).
pub const LOCKOUT_MAX: Duration = Duration::from_secs(60);

/// Failed-handshake bookkeeping.
#[derive(Debug, Clone)]
pub struct Lockout {
    free_failures: u32,
    base: Duration,
    max: Duration,
    failures: u32,
    until: Option<Instant>,
}

impl Lockout {
    #[must_use]
    pub fn new(free_failures: u32, base: Duration, max: Duration) -> Self {
        Self {
            free_failures,
            base,
            max,
            failures: 0,
            until: None,
        }
    }

    /// How long a new connection must be refused for, if at all.
    #[must_use]
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.until
            .map(|until| until.saturating_duration_since(now))
            .filter(|left| !left.is_zero())
    }

    /// A handshake with a wrong token or a malformed hello.
    pub fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        if self.failures > self.free_failures {
            let doublings = (self.failures - self.free_failures - 1).min(16);
            let delay = self.base.saturating_mul(1_u32 << doublings).min(self.max);
            self.until = Some(now + delay);
        }
    }

    /// A correct token: the counter starts again.
    pub fn succeeded(&mut self) {
        self.failures = 0;
        self.until = None;
    }

    /// Consecutive failures so far.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// A token bucket: `burst` calls at once, refilled at `per_second`.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    burst: f64,
    per_second: f64,
    tokens: f64,
    last: Option<Instant>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(burst: u32, per_second: f64) -> Self {
        Self {
            burst: f64::from(burst),
            per_second,
            tokens: f64::from(burst),
            last: None,
        }
    }

    /// Takes one call from the bucket; `false` when it is empty.
    pub fn allow(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last).as_secs_f64();
            self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        }
        self.last = Some(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_starts_after_the_free_failures_and_grows_to_the_cap() {
        let t0 = Instant::now();
        let mut lock = Lockout::new(2, Duration::from_secs(1), Duration::from_secs(8));
        lock.failed(t0);
        lock.failed(t0);
        assert_eq!(lock.remaining(t0), None, "two free failures");
        lock.failed(t0);
        assert_eq!(lock.remaining(t0), Some(Duration::from_secs(1)));
        lock.failed(t0);
        assert_eq!(lock.remaining(t0), Some(Duration::from_secs(2)));
        for _ in 0..20 {
            lock.failed(t0);
        }
        assert_eq!(lock.remaining(t0), Some(Duration::from_secs(8)), "capped");
        assert_eq!(lock.remaining(t0 + Duration::from_secs(9)), None, "expires");
        lock.succeeded();
        assert_eq!(lock.failures(), 0);
        assert_eq!(lock.remaining(t0), None);
    }

    #[test]
    fn the_bucket_allows_a_burst_then_refills() {
        let t0 = Instant::now();
        let mut bucket = RateLimiter::new(3, 2.0);
        assert!(bucket.allow(t0));
        assert!(bucket.allow(t0));
        assert!(bucket.allow(t0));
        assert!(!bucket.allow(t0), "the burst is spent");
        assert!(!bucket.allow(t0 + Duration::from_millis(100)));
        assert!(
            bucket.allow(t0 + Duration::from_millis(600)),
            "half a second refills one"
        );
        assert!(!bucket.allow(t0 + Duration::from_millis(600)));
        // never above the burst after a long wait
        let later = t0 + Duration::from_secs(60);
        assert!(bucket.allow(later) && bucket.allow(later) && bucket.allow(later));
        assert!(!bucket.allow(later));
    }
}
