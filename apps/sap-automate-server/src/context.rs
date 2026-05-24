//! Shared server context — held in an `Arc` and cloned into every tool.

use sap_automate_adt::AdtClient;
use sap_automate_ingest::EmbeddingClient;
use sap_automate_rag::RagEngine;
use sap_automate_rfc::SapClient;
use std::sync::Arc;

pub struct ServerContext {
    pub rag: Arc<RagEngine>,
    pub embedder: Arc<dyn EmbeddingClient>,
    pub sap_client: Arc<dyn SapClient>,
    pub adt_client: Arc<dyn AdtClient>,
    pub read_only: bool,
    pub agents_md: Option<String>,
}
