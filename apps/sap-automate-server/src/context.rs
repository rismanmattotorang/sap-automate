//! Shared server context — held in an `Arc` and cloned into every tool.

use sap_automate_adt::AdtClient;
use sap_automate_ingest::EmbeddingClient;
use sap_automate_rag::{GraphEngine, RagEngine};
use sap_automate_rfc::{MetadataCache, MockSapClient, SapClient};
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
    pub read_only: bool,
    pub agents_md: Option<String>,
}
