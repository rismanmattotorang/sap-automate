//! SAP-Automate RAG engine.
//!
//! Paper §VII defines a five-layer pipeline: L0 query analysis, L1 routing,
//! L2 hybrid retrieval, L3 GraphRAG, L4 HippoRAG, L5 RAPTOR.  Phase 1A
//! implements L2 (hybrid) against any `KnowledgeStore` backend, taking an
//! optional pre-computed query embedding so vector search works against
//! Qdrant when available and falls back to lexical search for InMemoryKb.

use sap_automate_kb::{Domain, KnowledgeStore, SearchHit, SearchQuery};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub text: &'a str,
    pub domain: Option<Domain>,
    pub top_k: usize,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub hit: SearchHit,
    pub layer: Layer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Hybrid,
    GraphRag,
    HippoRag,
    Raptor,
}

pub struct RagEngine {
    store: Arc<dyn KnowledgeStore>,
}

impl RagEngine {
    pub fn new(store: Arc<dyn KnowledgeStore>) -> Self {
        Self { store }
    }

    /// Phase 1A path: Layer 2 hybrid retrieval against the configured KB.
    /// Layer 3/4/5 routing arrives in Phase 5A (paper §X-H).
    pub async fn search<'a>(&self, query: Query<'a>) -> Result<Vec<Hit>, sap_automate_kb::StoreError> {
        let mut q = SearchQuery::text(query.text, query.top_k);
        if let Some(d) = query.domain { q = q.with_domain(d); }
        if let Some(e) = query.embedding { q = q.with_embedding(e); }
        let hits = self.store.search(q).await?;
        Ok(hits.into_iter().map(|h| Hit { hit: h, layer: Layer::Hybrid }).collect())
    }
}
