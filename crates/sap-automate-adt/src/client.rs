//! AdtClient async trait.
//!
//! Phase 2 finalisation: every method that modifies state takes an
//! `AdtCallContext` carrying the server's read-only-mode flag.  Mock and
//! HTTP backends both honour the flag, refusing writes when set.

use crate::destination::AdtDestination;
use crate::error::AdtResult;
use crate::types::{
    ActivationOutcome, ActivationRequest, AdtSearchHit, AdtSearchRequest, CdsView,
    PackageContents, ProgramSource, TableRow, WhereUsedHit, WhereUsedRequest,
};
use async_trait::async_trait;

/// Per-call security / observability context.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdtCallContext {
    pub read_only: bool,
}

#[async_trait]
pub trait AdtClient: Send + Sync {
    /// Destination metadata (redacted form is safe for logs).
    fn destination(&self) -> &AdtDestination;

    // --- Read-only ---------------------------------------------------------

    async fn get_program(&self, name: &str) -> AdtResult<ProgramSource>;
    async fn get_class(&self, name: &str) -> AdtResult<ProgramSource>;
    async fn get_interface(&self, name: &str) -> AdtResult<ProgramSource>;
    async fn get_include(&self, name: &str) -> AdtResult<ProgramSource>;
    async fn get_function_module(&self, group: &str, name: &str) -> AdtResult<ProgramSource>;
    async fn get_package_contents(&self, package: &str) -> AdtResult<PackageContents>;
    async fn get_cds_view(&self, name: &str) -> AdtResult<CdsView>;

    async fn search(&self, request: AdtSearchRequest) -> AdtResult<Vec<AdtSearchHit>>;
    async fn where_used(&self, request: WhereUsedRequest) -> AdtResult<Vec<WhereUsedHit>>;

    /// Read table contents through the ADT Data Preview API.  On SAP BTP
    /// this is blocked at the backend; the call returns
    /// `AdtError::DataPreviewBlocked` so the agent can fall back to RFC
    /// (`sap.table.read`).
    async fn get_table_contents(&self, table: &str, max_rows: usize) -> AdtResult<Vec<TableRow>>;

    // --- Write (gated by `ctx.read_only`) ---------------------------------

    async fn activate(&self, request: ActivationRequest, ctx: AdtCallContext) -> AdtResult<ActivationOutcome>;
}
