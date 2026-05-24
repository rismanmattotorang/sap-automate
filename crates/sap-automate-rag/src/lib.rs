//! SAP-Automate RAG engine.
//!
//! Paper §VII defines a five-layer pipeline: L0 query analysis, L1 routing,
//! L2 hybrid retrieval, L3 GraphRAG, L4 HippoRAG, L5 RAPTOR.  This Phase 1
//! skeleton implements only L2 against `InMemoryKb`; the algorithm
//! signatures are stable so later phases plug in concrete implementations
//! without changing call sites.

use sap_automate_kb::{Document, Domain, InMemoryKb};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub text: &'a str,
    pub domain: Option<Domain>,
    pub top_k: usize,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub document: Document,
    /// Layer that produced the hit (paper Algorithm 1).
    pub layer: Layer,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Hybrid,
    GraphRag,
    HippoRag,
    Raptor,
}

pub struct RagEngine {
    kb: Arc<InMemoryKb>,
}

impl RagEngine {
    pub fn new(kb: Arc<InMemoryKb>) -> Self { Self { kb } }

    /// Phase 1 path: Layer 2 (hybrid) → simple term-frequency search until the
    /// BM25 + dense + RRF implementation lands in Phase 3.
    pub async fn search<'a>(&self, query: Query<'a>) -> Vec<Hit> {
        let docs = self.kb.search(query.text, query.domain, query.top_k);
        docs.into_iter()
            .enumerate()
            .map(|(rank, document)| Hit {
                document,
                layer: Layer::Hybrid,
                score: 1.0 / (1.0 + rank as f32),
            })
            .collect()
    }
}
