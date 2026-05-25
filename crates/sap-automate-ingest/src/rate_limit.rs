//! Simple per-host token-bucket rate limiter for the crawler.
//!
//! Convergent with Crawl4AI's stealth-mode rate-limiting: production crawls
//! must respect the source's published cadence (`Crawl-delay:` from
//! robots.txt) and avoid burst behaviour that gets the crawler IP-banned.
//!
//! This implementation is deliberately tiny — one mutex, one map of
//! `(host -> last_tick)` — because we run a single-tenant crawler.  It's
//! enough to keep the well-known SAP Help Portal happy and to model
//! correctness under tests.  Swap in `governor` or a real distributed
//! rate limiter when the crawl matrix grows beyond a single node.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-host token-bucket limiter.  Each host has its own bucket: a fetch
/// consumes one token, refill is `1 / interval` tokens per second.
pub struct RateLimiter {
    default_interval: Duration,
    state: Mutex<HashMap<String, BucketState>>,
}

#[derive(Debug, Clone, Copy)]
struct BucketState {
    /// Earliest instant at which the next fetch is allowed.  We don't bother
    /// tracking fractional tokens — the only operation we need is `wait
    /// until ready`, which is a single `Instant` comparison.
    next_ready: Instant,
    interval: Duration,
}

impl RateLimiter {
    /// Build a limiter with a default per-host interval.  A `Crawl-delay:`
    /// from robots.txt can override this per host via [`Self::set_interval`].
    pub fn new(default_interval: Duration) -> Self {
        Self {
            default_interval,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Override the interval for a specific host.  Idempotent.
    pub fn set_interval(&self, host: &str, interval: Duration) {
        let mut s = self.state.lock().unwrap();
        let entry = s.entry(host.to_string()).or_insert_with(|| BucketState {
            next_ready: Instant::now(),
            interval,
        });
        entry.interval = interval;
    }

    /// Compute the `Duration` we should sleep *before* fetching `host`.
    /// Mutates state — calling `acquire_wait` and then immediately fetching
    /// is the canonical use; calling it without fetching wastes a token
    /// (correct behaviour for cancellable callers).
    pub fn acquire_wait(&self, host: &str) -> Duration {
        let now = Instant::now();
        let mut s = self.state.lock().unwrap();
        let entry = s.entry(host.to_string()).or_insert_with(|| BucketState {
            next_ready: now,
            interval: self.default_interval,
        });
        let wait = if now >= entry.next_ready {
            Duration::ZERO
        } else {
            entry.next_ready - now
        };
        // Schedule next slot relative to *now or planned* — whichever is later.
        let base = entry.next_ready.max(now);
        entry.next_ready = base + entry.interval;
        wait
    }

    /// Inspect (without mutating) the per-host interval.
    pub fn interval_of(&self, host: &str) -> Duration {
        let s = self.state.lock().unwrap();
        s.get(host).map(|b| b.interval).unwrap_or(self.default_interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_does_not_wait() {
        let rl = RateLimiter::new(Duration::from_millis(200));
        assert_eq!(rl.acquire_wait("a"), Duration::ZERO);
    }

    #[test]
    fn second_immediate_call_waits_approximately_the_interval() {
        let rl = RateLimiter::new(Duration::from_millis(200));
        let _ = rl.acquire_wait("a");
        let w = rl.acquire_wait("a");
        // Allow a tiny scheduling slack window.
        assert!(w >= Duration::from_millis(190));
        assert!(w <= Duration::from_millis(220));
    }

    #[test]
    fn distinct_hosts_have_independent_buckets() {
        let rl = RateLimiter::new(Duration::from_millis(200));
        let _ = rl.acquire_wait("a");
        let w = rl.acquire_wait("b");
        assert_eq!(w, Duration::ZERO, "host b's first call should be free");
    }

    #[test]
    fn set_interval_takes_effect_for_next_call() {
        let rl = RateLimiter::new(Duration::from_millis(200));
        // First call: free, schedules next_ready = now + 200ms (old interval).
        let _ = rl.acquire_wait("a");
        // Shrink the interval before any subsequent call.
        rl.set_interval("a", Duration::from_millis(50));
        // The already-scheduled slot still uses the old spacing, so we
        // wait through it before checking the new cadence.
        std::thread::sleep(Duration::from_millis(220));
        // Bucket is now drained.  Two back-to-back calls should be spaced
        // by the new (50ms) interval, not the old 200ms.
        let w1 = rl.acquire_wait("a");
        let w2 = rl.acquire_wait("a");
        assert_eq!(w1, Duration::ZERO, "drained bucket, first call should be free");
        assert!(w2 >= Duration::from_millis(40) && w2 <= Duration::from_millis(70),
            "expected ~50ms spacing under new interval; got {w2:?}");
    }

    #[test]
    fn interval_of_returns_default_for_unknown_host() {
        let rl = RateLimiter::new(Duration::from_millis(200));
        assert_eq!(rl.interval_of("unseen"), Duration::from_millis(200));
    }
}
