//! SAP-Automate ABAP Development Tools (ADT) client.
//!
//! Brings the design ideas from `mario-andreschak/mcp-abap-adt` (clean
//! object-type-specific read-only tools over the ADT REST API) and
//! `fr0ster/mcp-abap-adt` (CRUD breadth, RAP-first, multi-transport,
//! destination model, "AI pairing, not vibing" safety stance) into a
//! Rust trait-based architecture that matches the rest of SAP-Automate.
//!
//! The crate is split into:
//!   - `types`    — request/response shapes shared by every backend
//!   - `client`   — the `AdtClient` async trait
//!   - `mock`     — offline `MockAdtClient` with realistic ABAP fixtures
//!   - `error`    — structured `AdtError` taxonomy mapped to MCP codes
//!   - `destination` — destination model (name, base URL, auth method)
//!   - `http` (feature `http`) — `HttpAdtClient` against a live SAP system
//!
//! Read-only-by-default safety is enforced by the `AdtCallContext::read_only`
//! flag, mirroring the `sap-automate-rfc` pattern.

pub mod client;
pub mod destination;
pub mod error;
pub mod mock;
pub mod types;

#[cfg(feature = "http")]
pub mod http;

pub use client::{AdtCallContext, AdtClient};
pub use destination::{AdtAuth, AdtDestination};
pub use error::{AdtError, AdtErrorCode, AdtResult};
pub use mock::MockAdtClient;
pub use types::{
    AbapObjectKind, ActivationOutcome, ActivationRequest, AdtSearchHit, AdtSearchRequest,
    CdsView, PackageMember, PackageContents, ProgramSource, TableRow, WhereUsedHit,
    WhereUsedRequest, MAX_TABLE_ROWS,
};

#[cfg(feature = "http")]
pub use http::HttpAdtClient;
