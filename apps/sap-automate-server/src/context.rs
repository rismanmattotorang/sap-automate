//! Shared server context — held in an `Arc` and cloned into every tool.

use sap_automate_adt::AdtClient;
use sap_automate_ingest::EmbeddingClient;
use sap_automate_observability::AuditLog;
use sap_automate_rag::{GraphEngine, RagEngine};
use sap_automate_rfc::{BusinessHubClient, MetadataCache, MockSapClient, SapClient};
use std::sync::Arc;

pub struct ServerContext {
    pub rag: Arc<RagEngine>,
    pub graph: Arc<GraphEngine>,
    pub embedder: Arc<dyn EmbeddingClient>,
    /// The cache-decorated SapClient used by every tool.  Identical
    /// trait surface to the underlying `MockSapClient` / future
    /// `NetweaverSapClient`; metadata reads are TTL-cached.
    pub sap_client: Arc<dyn SapClient>,
    /// Direct handle to the metadata cache for the cache-stats /
    /// invalidate tools and the `sap-cache://stats` resource.  `None`
    /// when caching is disabled via `--metadata-cache-ttl-secs=0`.
    pub metadata_cache: Option<Arc<MetadataCache<MockSapClient>>>,
    pub adt_client: Arc<dyn AdtClient>,
    /// SAP Business Accelerator Hub sandbox client.  `None` when no
    /// `SAP_BUSINESS_HUB_KEY` is configured — the `sap.bp.*` tools then
    /// return a friendly "feature disabled" error instead of crashing.
    pub business_hub: Option<Arc<BusinessHubClient>>,
    pub read_only: bool,
    pub agents_md: Option<String>,
    /// Append-only audit log for state-mutating tool calls (SOX / GDPR
    /// evidence).  Arguments are redacted by `AuditLog::record`.
    pub audit: Arc<AuditLog>,
    /// SAP system identity (host/client) recorded on each audit entry.
    pub sap_system: Option<String>,
}
