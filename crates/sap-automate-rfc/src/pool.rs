//! Connection pool primitive.
//!
//! The pool is a thin semaphore-based concurrency limiter — it does not
//! itself manage live RFC connections, because the trait-based `SapClient`
//! design lets backends decide whether to pool, multiplex, or open per-call.
//! What this primitive *does* enforce is the upper bound: at most `cap`
//! concurrent in-flight calls, with a `TOOL_BUSY`-style error when the
//! bound is hit.  This matches paper §IV-G's per-session concurrency cap
//! and the `SAP_POOL_SIZE` knob from the Python reference project.

use crate::error::{RfcError, RfcResult};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct ConnectionPool {
    cap: usize,
    sem: Arc<Semaphore>,
}

impl ConnectionPool {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self { cap, sem: Arc::new(Semaphore::new(cap)) }
    }

    pub fn cap(&self) -> usize { self.cap }

    pub fn available(&self) -> usize { self.sem.available_permits() }

    pub async fn acquire(&self) -> RfcResult<OwnedSemaphorePermit> {
        Arc::clone(&self.sem)
            .acquire_owned()
            .await
            .map_err(|_| RfcError::PoolExhausted { cap: self.cap })
    }

    /// Try to acquire without waiting.  Returns `PoolExhausted` if no slot
    /// is immediately free.
    pub fn try_acquire(&self) -> RfcResult<OwnedSemaphorePermit> {
        Arc::clone(&self.sem)
            .try_acquire_owned()
            .map_err(|_| RfcError::PoolExhausted { cap: self.cap })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cap_is_enforced() {
        let pool = ConnectionPool::new(2);
        let _a = pool.acquire().await.unwrap();
        let _b = pool.acquire().await.unwrap();
        // Third try should fail immediately.
        assert!(pool.try_acquire().is_err());
        assert_eq!(pool.available(), 0);
    }

    #[tokio::test]
    async fn slot_released_on_drop() {
        let pool = ConnectionPool::new(1);
        {
            let _g = pool.acquire().await.unwrap();
        }
        assert_eq!(pool.available(), 1);
    }
}
