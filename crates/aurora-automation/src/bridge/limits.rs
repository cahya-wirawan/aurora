//! The request rate limit. It takes the time as an argument so tests need no
//! sleeping.
//!
//! There is deliberately **no failed-authentication lockout** (task 165): a
//! lockout that refuses connections after repeated failures also refuses
//! the real controller, so any local process could hold it out with a
//! stream of wrong tokens. A failed handshake costs only its own connection
//! (see `server`); the 256-bit token is what makes guessing pointless.

use std::time::Instant;

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
    use std::time::Duration;

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
        let later = t0 + Duration::from_mins(1);
        assert!(bucket.allow(later) && bucket.allow(later) && bucket.allow(later));
        assert!(!bucket.allow(later));
    }
}
