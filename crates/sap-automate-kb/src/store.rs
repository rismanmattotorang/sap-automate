//! Knowledge store trait + in-memory implementation.

use crate::doc_tree::{build_document_tree, DocumentTree};
use crate::schema::{Chunk, ChunkId, Document, DocumentId, Domain};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Dense,
    Sparse,
    Hybrid,
}

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

    /// Run a single-pass search.  Backends pick the strategy that fits
    /// the query (text vs embedding).
    async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>, StoreError>;

    /// Paper §VII-C: dense + sparse search in parallel, returned as two
    /// rankings so the caller can fuse with RRF.  Default impl falls back
    /// to a single search() call; backends that have both indexes should
    /// override.
    async fn hybrid_search(&self, query: SearchQuery)
        -> Result<(Vec<SearchHit>, Vec<SearchHit>), StoreError>
    {
        let dense = self.search(query.clone()).await?;
        Ok((dense, Vec::new()))
    }

    /// Total stored chunk count, for index-freshness dashboards.
    async fn chunk_count(&self) -> Result<usize, StoreError>;

    /// Build (or retrieve) the hierarchical document tree for a document.
    /// OpenKB + PageIndex convergent pattern: lets an agent navigate a long
    /// document by section path instead of doing similarity-blind retrieval.
    ///
    /// Default impl is deterministic and on-demand: fetch the document, run
    /// `build_document_tree`.  Production backends with persistent storage
    /// should override to cache the tree alongside the document.
    async fn get_document_tree(&self, id: &DocumentId) -> Result<Option<DocumentTree>, StoreError> {
        match self.get_document(id).await? {
            None => Ok(None),
            Some(doc) => Ok(Some(build_document_tree(&doc))),
        }
    }

    /// Upsert counters surfaced for telemetry.  Default `0/0` keeps backends
    /// that don't track dedup statistics from having to lie.
    fn upsert_stats(&self) -> UpsertStats {
        UpsertStats::default()
    }
}

/// Lifetime upsert counters.  `dedup_skipped` increments when the store
/// recognises a chunk by `content_hash(text)` and elects to skip the write.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct UpsertStats {
    pub chunks_written: u64,
    pub chunks_dedup_skipped: u64,
    pub documents_written: u64,
}

// ---------------------------------------------------------------------------
// In-memory implementation
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct InMemoryKb {
    documents: RwLock<HashMap<DocumentId, Document>>,
    chunks: RwLock<HashMap<ChunkId, Chunk>>,
    /// Content hash → set of chunk ids that already store that body.
    /// Used by the dedup path in `upsert`: a chunk whose `content_hash(text)`
    /// is already present *under the same id* is a no-op write.
    content_hashes: RwLock<HashMap<ChunkId, String>>,
    stats: RwLock<UpsertStats>,
}

impl InMemoryKb {
    pub fn new() -> Self { Self::default() }

    /// Synchronous, infallible upsert for sync seed code (no .await needed).
    pub fn insert_document(&self, doc: Document) {
        self.documents.write().unwrap().insert(doc.id.clone(), doc);
        self.stats.write().unwrap().documents_written += 1;
    }

    pub fn insert_chunk(&self, chunk: Chunk) {
        let hash = crate::schema::content_hash(&chunk.text);
        let mut hashes = self.content_hashes.write().unwrap();
        if hashes.get(&chunk.id).is_some_and(|h| h == &hash) {
            self.stats.write().unwrap().chunks_dedup_skipped += 1;
            return;
        }
        hashes.insert(chunk.id.clone(), hash);
        drop(hashes);
        self.chunks.write().unwrap().insert(chunk.id.clone(), chunk);
        self.stats.write().unwrap().chunks_written += 1;
    }
}

#[async_trait]
impl KnowledgeStore for InMemoryKb {
    async fn upsert(&self, batch: UpsertBatch) -> Result<(), StoreError> {
        for d in batch.documents {
            self.insert_document(d);
        }
        for c in batch.chunks {
            self.insert_chunk(c);
        }
        Ok(())
    }

    async fn get_document(&self, id: &DocumentId) -> Result<Option<Document>, StoreError> {
        Ok(self.documents.read().unwrap().get(id).cloned())
    }

    async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        // Single-pass: if embedding present, dense search; otherwise BM25.
        if let Some(qe) = &query.embedding {
            self.dense_search(qe, &query).await
        } else {
            self.bm25_search(&query).await
        }
    }

    async fn hybrid_search(&self, query: SearchQuery)
        -> Result<(Vec<SearchHit>, Vec<SearchHit>), StoreError>
    {
        // Dense and sparse rankings run in parallel and are returned separately
        // for RRF fusion in the caller (paper §VII-C).
        let dense = if let Some(qe) = &query.embedding {
            self.dense_search(qe, &query).await?
        } else {
            Vec::new()
        };
        let sparse = self.bm25_search(&query).await?;
        Ok((dense, sparse))
    }

    async fn chunk_count(&self) -> Result<usize, StoreError> {
        Ok(self.chunks.read().unwrap().len())
    }

    fn upsert_stats(&self) -> UpsertStats {
        *self.stats.read().unwrap()
    }
}

impl InMemoryKb {
    async fn dense_search(&self, qe: &[f32], query: &SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        let chunks = self.chunks.read().unwrap();
        let want_domain = |d: Domain| query.domains.is_empty() || query.domains.contains(&d);
        let mut hits: Vec<SearchHit> = chunks.values()
            .filter(|c| want_domain(c.domain))
            .filter_map(|c| {
                let ce = c.embedding.as_ref()?;
                if ce.len() != qe.len() { return None; }
                Some(SearchHit { chunk: c.clone(), score: cosine(ce, qe) })
            })
            .collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(query.top_k);
        Ok(hits)
    }

    /// BM25 sparse retrieval with classical (k1=1.5, b=0.75) tuning.
    /// Paper §VII-C / VII-D: tuned for the SAP corpus where exact-match
    /// retrieval of transaction codes and ABAP identifiers matters as much
    /// as semantic similarity.
    async fn bm25_search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        const K1: f32 = 1.5;
        const B: f32 = 0.75;

        let chunks = self.chunks.read().unwrap();
        let want_domain = |d: Domain| query.domains.is_empty() || query.domains.contains(&d);

        // Filter corpus to the targeted domains so IDF stays relevant.
        let candidates: Vec<&Chunk> = chunks.values().filter(|c| want_domain(c.domain)).collect();
        let n = candidates.len();
        if n == 0 { return Ok(Vec::new()); }

        let query_terms: Vec<String> = tokenize(&query.text);
        if query_terms.is_empty() { return Ok(Vec::new()); }

        // Average document length across the candidate set.
        let mut doc_term_counts: Vec<HashMap<String, u32>> = Vec::with_capacity(n);
        let mut doc_lens: Vec<u32> = Vec::with_capacity(n);
        for c in &candidates {
            let mut counts = HashMap::new();
            let mut len = 0u32;
            for t in tokenize(&c.text) {
                *counts.entry(t).or_insert(0) += 1;
                len += 1;
            }
            doc_term_counts.push(counts);
            doc_lens.push(len);
        }
        let avg_dl: f32 = doc_lens.iter().sum::<u32>() as f32 / n as f32;
        if avg_dl == 0.0 { return Ok(Vec::new()); }

        // Document frequency per query term.
        let df: HashMap<&String, usize> = query_terms.iter().map(|term| {
            let count = doc_term_counts.iter().filter(|c| c.contains_key(term)).count();
            (term, count)
        }).collect();

        let mut hits: Vec<SearchHit> = Vec::with_capacity(n);
        for (idx, chunk) in candidates.iter().enumerate() {
            let dl = doc_lens[idx] as f32;
            let counts = &doc_term_counts[idx];
            let mut score = 0.0f32;
            for term in &query_terms {
                let f = *counts.get(term).unwrap_or(&0) as f32;
                if f == 0.0 { continue; }
                let df_t = *df.get(term).unwrap_or(&0) as f32;
                let idf = ((n as f32 - df_t + 0.5) / (df_t + 0.5) + 1.0).ln();
                let denom = f + K1 * (1.0 - B + B * dl / avg_dl);
                score += idf * (f * (K1 + 1.0)) / denom;
            }
            if score > 0.0 {
                hits.push(SearchHit { chunk: (*chunk).clone(), score });
            }
        }
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(query.top_k);
        Ok(hits)
    }
}

/// Tokeniser used by both BM25 indexing and ad-hoc helpers.  Lowercase,
/// alphanumeric-or-underscore runs; preserves SAP identifier shape
/// (`BAPI_ACC_DOCUMENT_POST`, `MARA-MATNR`).
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            if current.len() >= 2 { out.push(std::mem::take(&mut current)); }
            else { current.clear(); }
        }
    }
    if current.len() >= 2 { out.push(current); }
    out
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

    #[tokio::test]
    async fn upsert_dedup_skips_identical_chunk() {
        let kb = InMemoryKb::new();
        let chunk = sample_chunk("c-1", Domain::SapHelp, "identical body", None);
        kb.insert_chunk(chunk.clone());
        kb.insert_chunk(chunk.clone());
        kb.insert_chunk(chunk.clone());
        assert_eq!(kb.chunk_count().await.unwrap(), 1);
        let s = kb.upsert_stats();
        assert_eq!(s.chunks_written, 1);
        assert_eq!(s.chunks_dedup_skipped, 2);
    }

    #[tokio::test]
    async fn upsert_dedup_treats_changed_text_as_new() {
        let kb = InMemoryKb::new();
        let c1 = sample_chunk("c-1", Domain::SapHelp, "v1", None);
        let c2 = sample_chunk("c-1", Domain::SapHelp, "v2", None);
        kb.insert_chunk(c1);
        kb.insert_chunk(c2);
        // Same id, different text: the second write wins (overwrite), but
        // the dedup counter does not advance.
        let s = kb.upsert_stats();
        assert_eq!(s.chunks_written, 2);
        assert_eq!(s.chunks_dedup_skipped, 0);
    }

    #[tokio::test]
    async fn get_document_tree_uses_default_builder() {
        let kb = InMemoryKb::new();
        let doc = Document::new(
            "sap_help:demo", Domain::SapHelp, "u", "Demo",
            "# Top\nbody\n## Sub\nmore\n",
        );
        kb.insert_document(doc.clone());
        let tree = kb.get_document_tree(&doc.id).await.unwrap().unwrap();
        assert_eq!(tree.document_id, "sap_help:demo");
        assert!(tree.node_count() >= 3);
        assert_eq!(tree.max_depth, 2);
    }
}
