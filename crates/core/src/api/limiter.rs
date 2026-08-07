//! Client-side rate limiting.
//!
//! The server enforces two independent limits: a per-key token bucket (capacity
//! from the key's `rate_limit`, refilling over a 60 s window) and a per-IP block
//! that trips at 300 requests in 10 s and then locks you out for an hour. The
//! second one is the dangerous one — it is not a backoff, it is an hour of
//! downtime for the user.
//!
//! So we shape traffic on our side rather than discovering the limit by hitting
//! it. `Instant` is injected so the behaviour is testable without sleeping.

use std::time::{Duration, Instant};

/// A classic token bucket: `capacity` tokens, refilling at `refill_per_sec`.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_per_sec: f64, now: Instant) -> Self {
        assert!(capacity > 0.0, "capacity must be positive");
        assert!(refill_per_sec > 0.0, "refill rate must be positive");
        Self {
            capacity,
            tokens: capacity,
            refill_per_sec,
            last: now,
        }
    }

    /// Conservative default: 2 requests/second sustained, bursting to 8.
    ///
    /// A scanner makes two requests per room entered (lookup, then detail) and
    /// rooms arrive seconds apart, so this is far above what play generates
    /// while staying an order of magnitude below the per-IP block threshold.
    pub fn default_at(now: Instant) -> Self {
        Self::new(8.0, 2.0, now)
    }

    /// Take a token if one is available. Returns `None` when the caller may
    /// proceed, or `Some(wait)` for how long until a token exists.
    pub fn try_take(&mut self, now: Instant) -> Option<Duration> {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            let deficit = 1.0 - self.tokens;
            Some(Duration::from_secs_f64(deficit / self.refill_per_sec))
        }
    }

    /// Drain the bucket, e.g. after the server says we are rate limited. Stops
    /// us from immediately spending a burst allowance the server has rejected.
    pub fn drain(&mut self, now: Instant) {
        self.refill(now);
        self.tokens = 0.0;
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            self.last = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_fresh_bucket_allows_a_full_burst() {
        let start = t0();
        let mut b = TokenBucket::new(3.0, 1.0, start);
        assert!(b.try_take(start).is_none());
        assert!(b.try_take(start).is_none());
        assert!(b.try_take(start).is_none());
        assert!(b.try_take(start).is_some(), "fourth exceeds capacity");
    }

    #[test]
    fn reports_how_long_to_wait() {
        let start = t0();
        let mut b = TokenBucket::new(1.0, 2.0, start);
        assert!(b.try_take(start).is_none());
        let wait = b.try_take(start).expect("bucket empty");
        // One token at 2/sec is half a second.
        assert!(
            (wait.as_secs_f64() - 0.5).abs() < 1e-6,
            "expected ~0.5s, got {wait:?}"
        );
    }

    #[test]
    fn refills_over_time() {
        let start = t0();
        let mut b = TokenBucket::new(2.0, 1.0, start);
        b.try_take(start);
        b.try_take(start);
        assert!(b.try_take(start).is_some(), "empty");

        let later = start + Duration::from_secs(1);
        assert!(b.try_take(later).is_none(), "one token refilled");
    }

    #[test]
    fn refill_is_capped_at_capacity() {
        let start = t0();
        let mut b = TokenBucket::new(2.0, 1.0, start);
        b.try_take(start);
        b.try_take(start);

        // An hour later we still only get `capacity` back, not 3600 tokens.
        let much_later = start + Duration::from_secs(3600);
        assert!(b.try_take(much_later).is_none());
        assert!(b.try_take(much_later).is_none());
        assert!(b.try_take(much_later).is_some(), "capped at capacity");
    }

    #[test]
    fn drain_empties_the_burst_allowance() {
        let start = t0();
        let mut b = TokenBucket::new(5.0, 1.0, start);
        b.drain(start);
        assert!(b.try_take(start).is_some(), "drained");
    }

    #[test]
    fn the_default_stays_well_under_the_ip_block_threshold() {
        // The server blocks an IP for an hour at 300 requests / 10s. Sustained
        // throughput here must be nowhere near that.
        let start = t0();
        let mut b = TokenBucket::default_at(start);
        let mut taken = 0;
        for tick in 0..100 {
            let now = start + Duration::from_millis(tick * 100); // 10s total
            if b.try_take(now).is_none() {
                taken += 1;
            }
        }
        assert!(taken < 40, "10s of saturation issued {taken} requests");
    }
}
