//! SAP-Automate RFC and table client abstraction.
//!
//! Brings together the design insights from the reference projects we
//! studied (paper §III + new comparative analysis in `docs/COMPARISON.md`):
//!
//! - **From `thupalo/sap-rfc-mcp-server`**: connection pooling, metadata
//!   caching, bulk metadata loads, version-aware behaviour.
//! - **From `CDataSoftware/sap-erp-mcp-server`**: schema-discovery-first
//!   tool design (`get_tables` → `get_columns` → `run_query`) and the
//!   read-only-by-default safety posture.
//! - **From `SAP/mdk-mcp-server`**: constrained-enum tool parameters,
//!   project-aware tool calls, AGENTS.md guardrails.
//!
//! The crate is split into:
//! - `client`: the `SapClient` trait + `MockSapClient` (offline)
//! - `credentials`: layered credential provider (env / keyring / file)
//! - `error`: structured RFC error taxonomy mapped to MCP error codes
//! - `pool`: tokio-semaphore-based connection limiter
//! - `retry`: exponential-backoff helper + circuit-breaker primitive

pub mod bapiret2;
pub mod client;
pub mod credentials;
pub mod error;
pub mod metadata_cache;
#[cfg(feature = "odata")]
pub mod odata;
pub mod pool;
pub mod retry;

pub use bapiret2::{BapiRet2Message, BapiRet2Severity, parse_bapiret2};
pub use metadata_cache::{CacheStats, MetadataCache};
#[cfg(feature = "odata")]
pub use odata::{
    BusinessHubClient, BusinessHubConfig, BusinessPartner, OdataError, OdataResult,
};

pub use client::{
    BulkMetadata, MockSapClient, ReadTableRequest, RfcCallRequest, RfcFunctionMeta,
    RfcFunctionSummary, RfcParameter, RfcParamDirection, RfcSearchResult, SapClient,
    SystemInfo, TableRow, TableStructure, TableField, MAX_ROWS_HARD_CAP,
};
pub use credentials::{
    Credentials, CredentialProvider, CredentialSource, EnvCredentialProvider,
    LayeredCredentialProvider, StaticCredentialProvider,
};
pub use error::{RfcError, RfcErrorCode, RfcResult};
pub use pool::ConnectionPool;
pub use retry::{retry_with_backoff, BackoffPolicy, CircuitBreaker, CircuitState};
