//! Retry + circuit breaker primitives.
//!
//! Insight from the reference projects: `sap-rfc-mcp-server` has no
//! retry/circuit logic and surfaces every RFC blip to the agent — agents
//! then waste tokens reasoning about transient SAP unavailability instead
//! of business logic.  We bake retry classification into the trait surface:
//! `RfcError::is_transient()` decides whether `retry_with_backoff` retries.

use crate::error::{RfcError, RfcResult};
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    pub max_attempts: u32,
    pub initial: Duration,
    pub multiplier: f32,
    pub max_delay: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(2),
        }
    }
}

/// Retry an async fallible call, with exponential backoff on transient
/// errors only.  Permanent errors short-circuit.
pub async fn retry_with_backoff<F, Fut, T>(policy: &BackoffPolicy, mut f: F) -> RfcResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = RfcResult<T>>,
{
    let mut delay = policy.initial;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if !e.is_transient() => {
                debug!(?e, "permanent error; not retrying");
                return Err(e);
            }
            Err(e) if attempt >= policy.max_attempts => {
                warn!(?e, attempt, "retry budget exhausted");
                return Err(e);
            }
            Err(e) => {
                warn!(?e, attempt, ?delay, "transient error; retrying");
                tokio::time::sleep(delay).await;
                let next = (delay.as_millis() as f32 * policy.multiplier) as u64;
                delay = Duration::from_millis(next).min(policy.max_delay);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Circuit breaker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState { Closed, Open, HalfOpen }

#[derive(Debug)]
struct CircuitInner {
    state: CircuitState,
    failures: u32,
    opened_at: Option<Instant>,
}

pub struct CircuitBreaker {
    failure_threshold: u32,
    open_duration: Duration,
    inner: Mutex<CircuitInner>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, open_duration: Duration) -> Self {
        Self {
            failure_threshold,
            open_duration,
            inner: Mutex::new(CircuitInner {
                state: CircuitState::Closed,
                failures: 0,
                opened_at: None,
            }),
        }
    }

    pub fn state(&self) -> CircuitState {
        self.tick().state
    }

    /// Tick the breaker forward in time: if it's been open long enough,
    /// transition to half-open so the next call can probe.  Returns a
    /// snapshot of the current state.
    fn tick(&self) -> CircuitInner {
        let mut g = self.inner.lock().unwrap();
        if g.state == CircuitState::Open {
            if let Some(opened) = g.opened_at {
                if opened.elapsed() >= self.open_duration {
                    g.state = CircuitState::HalfOpen;
                }
            }
        }
        CircuitInner { state: g.state, failures: g.failures, opened_at: g.opened_at }
    }

    /// Execute a call through the breaker.
    pub async fn call<F, Fut, T>(&self, f: F) -> RfcResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = RfcResult<T>>,
    {
        let snapshot = self.tick();
        if snapshot.state == CircuitState::Open {
            let remaining = snapshot
                .opened_at
                .map(|t| self.open_duration.saturating_sub(t.elapsed()))
                .unwrap_or(self.open_duration);
            return Err(RfcError::CircuitOpen { retry_after_ms: remaining.as_millis() as u64 });
        }

        let result = f().await;
        let mut g = self.inner.lock().unwrap();
        match &result {
            Ok(_) => {
                g.failures = 0;
                g.state = CircuitState::Closed;
                g.opened_at = None;
            }
            Err(e) if e.is_transient() => {
                g.failures += 1;
                if g.failures >= self.failure_threshold {
                    g.state = CircuitState::Open;
                    g.opened_at = Some(Instant::now());
                    warn!(failures = g.failures, "circuit OPEN");
                }
            }
            Err(_) => {
                // Permanent errors don't trip the breaker.
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn retries_transient_errors() {
        let attempts = Arc::new(AtomicU32::new(0));
        let policy = BackoffPolicy {
            max_attempts: 3,
            initial: Duration::from_millis(1),
            multiplier: 1.0,
            max_delay: Duration::from_millis(1),
        };
        let a = Arc::clone(&attempts);
        let result: RfcResult<&'static str> = retry_with_backoff(&policy, move || {
            let a = Arc::clone(&a);
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n < 2 { Err(RfcError::Timeout { timeout_ms: 10 }) }
                else { Ok("ok") }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_permanent_errors() {
        let attempts = Arc::new(AtomicU32::new(0));
        let a = Arc::clone(&attempts);
        let result: RfcResult<()> = retry_with_backoff(&BackoffPolicy::default(), move || {
            let a = Arc::clone(&a);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(RfcError::NotFound("Z_NOPE".into()))
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(50));
        for _ in 0..2 {
            let _ = cb.call(|| async {
                Err::<(), _>(RfcError::DestinationDown {
                    destination: "T01".into(),
                    reason: "down".into(),
                })
            }).await;
        }
        assert_eq!(cb.state(), CircuitState::Open);
        // Third call must short-circuit.
        let r = cb.call(|| async { Ok::<_, RfcError>(()) }).await;
        assert!(matches!(r, Err(RfcError::CircuitOpen { .. })));
    }
}
