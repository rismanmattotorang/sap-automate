//! Structured ADT error taxonomy.
//!
//! ADT-specific failure modes that the two reference projects surface as
//! either text strings or generic HTTP errors are split into typed
//! variants here so the MCP layer can map each to its appropriate
//! JSON-RPC error code (paper §IV-I).

use thiserror::Error;

pub type AdtResult<T> = std::result::Result<T, AdtError>;

/// Structured error codes for SAP ADT operations.  Numeric values are
/// stable across releases; `#[non_exhaustive]` lets us add new variants
/// without breaking downstream matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdtErrorCode {
    // Transient (-32100..-32199)
    Timeout = -32160,
    DestinationDown = -32161,
    CsrfRefresh = -32162,
    RateLimited = -32163,

    // Permanent (-32200..-32299)
    AuthFailed = -32260,
    NotFound = -32261,
    Forbidden = -32262,
    InvalidObjectName = -32263,
    InactiveObject = -32264,
    /// ADT data preview blocked on BTP-hosted systems (fr0ster note).
    DataPreviewBlocked = -32265,
    PermissionDenied = -32266,
    /// Object exists but in a locked state (transport not released, locked
    /// by another user, etc.).
    Locked = -32267,
    /// Server bug / programming error.  Never retried.
    Internal = -32298,
}

impl AdtErrorCode {
    pub fn as_i32(self) -> i32 { self as i32 }
    pub fn is_transient(self) -> bool {
        let v = self as i32;
        (-32199..=-32100).contains(&v)
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdtError {
    #[error("ADT timeout after {timeout_ms} ms")]
    Timeout { timeout_ms: u64 },

    #[error("ADT destination '{destination}' unreachable: {reason}")]
    DestinationDown { destination: String, reason: String },

    #[error("CSRF token refresh required")]
    CsrfRefresh,

    #[error("rate limited; retry after {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("object not found: {kind} '{name}'")]
    NotFound { kind: String, name: String },

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("invalid object name '{0}'")]
    InvalidObjectName(String),

    #[error("object is inactive: {0}")]
    InactiveObject(String),

    #[error("data preview blocked by SAP backend policy: {0}")]
    DataPreviewBlocked(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("object locked: {0}")]
    Locked(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl AdtError {
    pub fn code(&self) -> AdtErrorCode {
        match self {
            AdtError::Timeout { .. } => AdtErrorCode::Timeout,
            AdtError::DestinationDown { .. } => AdtErrorCode::DestinationDown,
            AdtError::CsrfRefresh => AdtErrorCode::CsrfRefresh,
            AdtError::RateLimited { .. } => AdtErrorCode::RateLimited,
            AdtError::AuthFailed(_) => AdtErrorCode::AuthFailed,
            AdtError::NotFound { .. } => AdtErrorCode::NotFound,
            AdtError::Forbidden(_) => AdtErrorCode::Forbidden,
            AdtError::InvalidObjectName(_) => AdtErrorCode::InvalidObjectName,
            AdtError::InactiveObject(_) => AdtErrorCode::InactiveObject,
            AdtError::DataPreviewBlocked(_) => AdtErrorCode::DataPreviewBlocked,
            AdtError::PermissionDenied(_) => AdtErrorCode::PermissionDenied,
            AdtError::Locked(_) => AdtErrorCode::Locked,
            // Internal errors are programmer bugs, not transient — must
            // NOT be retried.  See Phase 7 code-review pass.
            AdtError::Internal(_) => AdtErrorCode::Internal,
        }
    }

    pub fn is_transient(&self) -> bool { self.code().is_transient() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adt_internal_is_permanent() {
        let e = AdtError::Internal("bug".into());
        assert!(!e.is_transient(),
            "AdtError::Internal must be permanent to prevent retry-loop on programmer bugs");
    }

    #[test]
    fn adt_transient_kinds_are_classified_correctly() {
        for code in [AdtErrorCode::Timeout, AdtErrorCode::DestinationDown,
                     AdtErrorCode::CsrfRefresh, AdtErrorCode::RateLimited] {
            assert!(code.is_transient(), "{code:?} should be transient");
        }
        for code in [AdtErrorCode::AuthFailed, AdtErrorCode::NotFound,
                     AdtErrorCode::Forbidden, AdtErrorCode::Internal,
                     AdtErrorCode::DataPreviewBlocked, AdtErrorCode::Locked] {
            assert!(!code.is_transient(), "{code:?} should NOT be transient");
        }
    }
}
