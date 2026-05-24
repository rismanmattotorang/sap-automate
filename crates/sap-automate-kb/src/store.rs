//! Knowledge store trait + in-memory implementation.

use crate::schema::{Chunk, ChunkId, Document, DocumentId, Domain};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Batch upsert payload.  Documents and chunks travel together so the store
/// can keep parent and children consistent.
#[derive(Debug, Default, Clone)]
pub struct UpsertBatch {
    pub documents: Vec<Document>,
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchQuery {
    /// Optional raw text — used by lexical/in-memory backends.
    pub text: String,
    /// Optional pre-computed query embedding — used by vector backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    #[serde(default)]
    pub domains: Vec<Domain>,
    pub top_k: usize,
}

impl SearchQuery {
    pub fn text(text: impl Into<String>, top_k: usize) -> Self {
        Self {
            text: text.into(),
            embedding: None,
            domains: Vec::new(),
            top_k,
        }
    }

    pub fn with_domain(mut self, domain: Domain) -> Self {
        self.domains.push(domain);
        self
    }

    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub chunk: Chunk,
    pub score: f32,
}

/// Pluggable knowledge store contract.
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    /// Insert or update a batch of documents + chunks.
    async fn upsert(&self, batch: UpsertBatch) -> Result<(), StoreError>;

    /// Fetch a document by id.
    async fn get_document(&self, id: &DocumentId) -> Result<Option<Document>, StoreError>;

    /// Run a search against the store.  Backends pick the strategy that fits
    /// the query (text vs embedding); both `text` and `embedding` may be
    /// present to allow hybrid stores to combine them.
    async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>, StoreError>;

    /// Total stored chunk count, for index-freshness dashboards.
    async fn chunk_count(&self) -> Result<usize, StoreError>;
}

// ---------------------------------------------------------------------------
// In-memory implementation
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct InMemoryKb {
    documents: RwLock<HashMap<DocumentId, Document>>,
    chunks: RwLock<HashMap<ChunkId, Chunk>>,
}

impl InMemoryKb {
    pub fn new() -> Self { Self::default() }

    /// Synchronous, infallible upsert for sync seed code (no .await needed).
    pub fn insert_document(&self, doc: Document) {
        self.documents.write().unwrap().insert(doc.id.clone(), doc);
    }

    pub fn insert_chunk(&self, chunk: Chunk) {
        self.chunks.write().unwrap().insert(chunk.id.clone(), chunk);
    }
}

#[async_trait]
impl KnowledgeStore for InMemoryKb {
    async fn upsert(&self, batch: UpsertBatch) -> Result<(), StoreError> {
        for d in batch.documents { self.insert_document(d); }
        for c in batch.chunks { self.insert_chunk(c); }
        Ok(())
    }

    async fn get_document(&self, id: &DocumentId) -> Result<Option<Document>, StoreError> {
        Ok(self.documents.read().unwrap().get(id).cloned())
    }

    async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        // Prefer cosine over embeddings if both sides provided one; otherwise
        // fall back to a term-frequency score.
        let chunks = self.chunks.read().unwrap();
        let want_domain = |d: Domain| query.domains.is_empty() || query.domains.contains(&d);

        let mut hits: Vec<SearchHit> = if let Some(qe) = &query.embedding {
            chunks
                .values()
                .filter(|c| want_domain(c.domain))
                .filter_map(|c| {
                    let ce = c.embedding.as_ref()?;
                    if ce.len() != qe.len() { return None; }
                    Some(SearchHit { chunk: c.clone(), score: cosine(ce, qe) })
                })
                .collect()
        } else {
            let q_lc = query.text.to_lowercase();
            let terms: Vec<&str> = q_lc.split_whitespace().collect();
            chunks
                .values()
                .filter(|c| want_domain(c.domain))
                .filter_map(|c| {
                    let hay = c.text.to_lowercase();
                    let count: usize = terms.iter().map(|t| hay.matches(t).count()).sum();
                    if count == 0 { None }
                    else { Some(SearchHit { chunk: c.clone(), score: count as f32 / (1.0 + c.text.len() as f32 / 200.0) }) }
                })
                .collect()
        };

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(query.top_k);
        Ok(hits)
    }

    async fn chunk_count(&self) -> Result<usize, StoreError> {
        Ok(self.chunks.read().unwrap().len())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-12);
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::content_hash;

    fn sample_chunk(id: &str, domain: Domain, text: &str, embedding: Option<Vec<f32>>) -> Chunk {
        Chunk {
            id: id.into(),
            document_id: format!("{}:doc1", domain.collection()),
            domain,
            ordinal: 0,
            text: text.into(),
            embedding,
            breadcrumbs: Vec::new(),
            title: "T".into(),
            uri: "u".into(),
        }
    }

    #[tokio::test]
    async fn text_search_filters_by_domain() {
        let kb = InMemoryKb::new();
        kb.insert_chunk(sample_chunk("a", Domain::SapHelp, "period close in FI", None));
        kb.insert_chunk(sample_chunk("b", Domain::Abap, "period close routine", None));

        let hits = kb.search(SearchQuery::text("period close", 10).with_domain(Domain::SapHelp))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.id, "a");
    }

    #[tokio::test]
    async fn vector_search_orders_by_cosine() {
        let kb = InMemoryKb::new();
        kb.insert_chunk(sample_chunk("near", Domain::SapHelp, "x", Some(vec![1.0, 0.0, 0.0])));
        kb.insert_chunk(sample_chunk("far",  Domain::SapHelp, "y", Some(vec![0.0, 1.0, 0.0])));

        let q = SearchQuery::text("", 10).with_embedding(vec![1.0, 0.0, 0.0]);
        let hits = kb.search(q).await.unwrap();
        assert_eq!(hits[0].chunk.id, "near");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn content_hash_stable() {
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("Hello"));
    }
}
