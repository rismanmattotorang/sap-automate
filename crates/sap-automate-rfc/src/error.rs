//! Structured RFC error taxonomy.
//!
//! Improves on `thupalo/sap-rfc-mcp-server` (which surfaces RFC failures as
//! plain text) by typing every failure mode and mapping to the MCP error
//! taxonomy from paper §IV-I.  Callers can therefore distinguish transient
//! (retry) from permanent (do not retry) errors at the JSON-RPC layer.

use thiserror::Error;

pub type RfcResult<T> = std::result::Result<T, RfcError>;

/// RFC error codes.  Values overlap the MCP code ranges in
/// `mcp_core::error::ErrorCode` so they translate cleanly when serialised
/// into a JSON-RPC error object.
/// Structured error codes for SAP RFC operations.  Numeric values are
/// stable across releases; `#[non_exhaustive]` lets us add new variants
/// in a minor release without breaking exhaustive matches in user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RfcErrorCode {
    // Transient (-32100..-32199): retryable
    Timeout = -32110,
    DestinationDown = -32120,
    PoolExhausted = -32130,
    CircuitOpen = -32140,
    UpstreamRateLimit = -32150,

    // Permanent (-32200..-32299): do not retry
    AuthFailed = -32210,
    NotFound = -32220,
    TableBufferOverflow = -32230,
    InvalidParameter = -32240,
    PermissionDenied = -32250,
    SchemaViolation = -32260,
    /// Server bug / programming error.  Never retried.
    Internal = -32299,

    // Degraded (-32300..-32399): partial result
    PartialBulk = -32310,
    StaleMetadata = -32320,
}

impl RfcErrorCode {
    pub fn as_i32(self) -> i32 { self as i32 }

    /// Whether the caller should retry after backoff.
    pub fn is_transient(self) -> bool {
        let v = self as i32;
        (-32199..=-32100).contains(&v)
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RfcError {
    #[error("RFC timeout after {timeout_ms} ms")]
    Timeout { timeout_ms: u64 },

    #[error("SAP destination '{destination}' unreachable: {reason}")]
    DestinationDown { destination: String, reason: String },

    #[error("connection pool exhausted (cap={cap})")]
    PoolExhausted { cap: usize },

    #[error("circuit open until ~{retry_after_ms} ms from now")]
    CircuitOpen { retry_after_ms: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("table buffer overflow for '{table}' (max_rows={max_rows})")]
    TableBufferOverflow { table: String, max_rows: usize },

    #[error("invalid parameter '{name}': {reason}")]
    InvalidParameter { name: String, reason: String },

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("schema violation: {0}")]
    SchemaViolation(String),

    #[error("partial result: {0}")]
    PartialBulk(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl RfcError {
    pub fn code(&self) -> RfcErrorCode {
        match self {
            RfcError::Timeout { .. } => RfcErrorCode::Timeout,
            RfcError::DestinationDown { .. } => RfcErrorCode::DestinationDown,
            RfcError::PoolExhausted { .. } => RfcErrorCode::PoolExhausted,
            RfcError::CircuitOpen { .. } => RfcErrorCode::CircuitOpen,
            RfcError::AuthFailed(_) => RfcErrorCode::AuthFailed,
            RfcError::NotFound(_) => RfcErrorCode::NotFound,
            RfcError::TableBufferOverflow { .. } => RfcErrorCode::TableBufferOverflow,
            RfcError::InvalidParameter { .. } => RfcErrorCode::InvalidParameter,
            RfcError::PermissionDenied(_) => RfcErrorCode::PermissionDenied,
            RfcError::SchemaViolation(_) => RfcErrorCode::SchemaViolation,
            RfcError::PartialBulk(_) => RfcErrorCode::PartialBulk,
            // Internal errors are programming bugs, not transient SAP
            // outages — they must NOT be retried (Phase 7 code review).
            RfcError::Internal(_) => RfcErrorCode::Internal,
        }
    }

    pub fn is_transient(&self) -> bool { self.code().is_transient() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_classification_only_matches_transient_range() {
        for c in [
            RfcErrorCode::Timeout, RfcErrorCode::DestinationDown,
            RfcErrorCode::PoolExhausted, RfcErrorCode::CircuitOpen,
            RfcErrorCode::UpstreamRateLimit,
        ] {
            assert!(c.is_transient(), "{c:?} should be transient");
        }
        for c in [
            RfcErrorCode::AuthFailed, RfcErrorCode::NotFound,
            RfcErrorCode::TableBufferOverflow, RfcErrorCode::InvalidParameter,
            RfcErrorCode::PermissionDenied, RfcErrorCode::SchemaViolation,
            RfcErrorCode::Internal,
            RfcErrorCode::PartialBulk, RfcErrorCode::StaleMetadata,
        ] {
            assert!(!c.is_transient(), "{c:?} should NOT be transient");
        }
    }

    #[test]
    fn rfc_error_internal_is_permanent() {
        // Regression for the Phase 7 review finding: previously
        // Internal mapped to Timeout, which caused retry_with_backoff
        // to spin on programming bugs.
        let e = RfcError::Internal("bug".into());
        assert!(!e.is_transient());
        assert_eq!(e.code() as i32, RfcErrorCode::Internal as i32);
    }
}
