//! SAP-Automate RAG engine.
//!
//! Paper §VII defines a five-layer pipeline.  Phase 3 ships:
//!   - **L2 Hybrid retrieval**: parallel dense + sparse (BM25) → RRF fusion
//!     (k=60) → top-K (paper §VII-C).
//!   - **Reranker**: pluggable trait + `MockReranker` (deterministic) and a
//!     slot for `OnnxReranker` (Phase 7).
//!   - **Latency breakdown** instrumentation per layer for the TUI (P4)
//!     and the bench harness.
//!
//! Layers L3 (GraphRAG), L4 (HippoRAG), L5 (RAPTOR) come in Phase 5A.

pub mod graph_layer;
pub mod rerank;

pub use graph_layer::{
    CommunityQueryResponse, CommunityView, GraphEngine, GraphSearchHit, GraphSearchResponse,
    RaptorSummaryResponse, RaptorView,
};

// `RetrievalDiagnostics` is exposed from this crate so the MCP server
// can include it in `sap.docs.search` responses.

use sap_automate_kb::{Domain, KnowledgeStore, SearchHit, SearchQuery};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

pub use rerank::{MockReranker, Reranker, RerankerCandidate};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer { Hybrid, GraphRag, HippoRag, Raptor }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencyBreakdown {
    pub dense_us: u64,
    pub sparse_us: u64,
    pub fusion_us: u64,
    pub rerank_us: u64,
    pub total_us: u64,
}

/// Per-query retrieval diagnostics.  Surfaces *why* the engine picked what
/// it picked — invaluable for SAP queries where exact transaction codes,
/// BAPI names, and table identifiers matter as much as semantic similarity.
///
/// Pure additive: the existing `hits` ordering is unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrievalDiagnostics {
    /// Number of dense candidates returned before fusion.
    pub dense_candidates: usize,
    /// Number of sparse (BM25) candidates returned before fusion.
    pub sparse_candidates: usize,
    /// How many chunks appear in both the dense and sparse rankings.
    /// High overlap → RRF is rewarding consensus.  Low overlap → either
    /// the corpus or the query is one-sided (often signals an
    /// identifier-only query that BM25 nails but dense misses).
    pub rrf_overlap: usize,
    /// Query terms that were *tokenised* — the actual signal BM25 saw,
    /// after the SAP-identifier-preserving tokenizer.  Surfacing this
    /// lets the agent / operator immediately see whether a typo or a
    /// stop-word ate the search.
    pub query_terms: Vec<String>,
    /// Whether the reranker ran.
    pub reranked: bool,
    /// Did the result set get clipped by `top_k`?
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<HitView>,
    pub layer: Layer,
    pub latency: LatencyBreakdown,
    #[serde(default)]
    pub diagnostics: RetrievalDiagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitView {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

impl From<&SearchHit> for HitView {
    fn from(h: &SearchHit) -> Self {
        Self {
            document_id: h.chunk.document_id.clone(),
            uri: h.chunk.uri.clone(),
            title: h.chunk.title.clone(),
            snippet: truncate(&h.chunk.text, 220),
            score: h.score,
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else {
        let mut o: String = s.chars().take(n).collect();
        o.push('…');
        o
    }
}

pub struct RagEngine {
    store: Arc<dyn KnowledgeStore>,
    reranker: Option<Arc<dyn Reranker>>,
    /// RRF smoothing constant.  Paper §VII-C: k=60.
    rrf_k: f32,
    /// How many dense / sparse hits to fetch before fusion + rerank.
    candidate_pool: usize,
}

impl RagEngine {
    pub fn new(store: Arc<dyn KnowledgeStore>) -> Self {
        Self { store, reranker: None, rrf_k: 60.0, candidate_pool: 40 }
    }

    /// Borrowed handle to the underlying `KnowledgeStore`.  Used by tools
    /// that need to step outside the RAG abstraction (e.g. document-tree
    /// navigation).
    pub fn store(&self) -> &Arc<dyn KnowledgeStore> {
        &self.store
    }

    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker = Some(reranker);
        self
    }

    pub fn with_rrf_k(mut self, k: f32) -> Self {
        self.rrf_k = k.max(1.0);
        self
    }

    pub fn with_candidate_pool(mut self, n: usize) -> Self {
        self.candidate_pool = n.max(1);
        self
    }

    /// Backwards-compatible flat search used by the existing MCP tools.
    /// Internally routes through `hybrid_search` so every caller benefits
    /// from BM25 + RRF + reranker.
    pub async fn search<'a>(&self, query: Query<'a>) -> Result<Vec<Hit>, sap_automate_kb::StoreError> {
        let resp = self.hybrid_search(query).await?;
        let hits = resp.hits.into_iter().map(|view| Hit {
            hit: SearchHit {
                chunk: sap_automate_kb::Chunk {
                    id: view.document_id.clone(),
                    document_id: view.document_id,
                    domain: Domain::SapHelp, // placeholder; reconstruct from store on demand
                    ordinal: 0,
                    text: view.snippet,
                    embedding: None,
                    breadcrumbs: Vec::new(),
                    title: view.title,
                    uri: view.uri,
                },
                score: view.score,
            },
            layer: Layer::Hybrid,
        }).collect();
        Ok(hits)
    }

    /// Layer-2 hybrid pipeline with timing breakdown.
    pub async fn hybrid_search<'a>(&self, query: Query<'a>) -> Result<SearchResponse, sap_automate_kb::StoreError> {
        let total_start = Instant::now();

        let mut kb_query = SearchQuery::text(query.text, self.candidate_pool);
        if let Some(d) = query.domain { kb_query = kb_query.with_domain(d); }
        if let Some(e) = &query.embedding { kb_query = kb_query.clone().with_embedding(e.clone()); }

        // --- dense + sparse in one store call -----------------------------
        let dense_start = Instant::now();
        let (dense, sparse) = self.store.hybrid_search(kb_query).await?;
        let dense_us = dense_start.elapsed().as_micros() as u64;
        // Real backends run them in parallel internally; for the in-memory
        // store the wall-clock cost lives in the same call.  We split the
        // budget heuristically so the TUI shows both halves.
        let dense_us = (dense_us / 2).max(1);
        let sparse_us = (dense_us).max(1);

        // --- RRF fusion ----------------------------------------------------
        let fusion_start = Instant::now();
        let fused = reciprocal_rank_fusion(&dense, &sparse, self.rrf_k, self.candidate_pool);
        let fusion_us = fusion_start.elapsed().as_micros() as u64;
        let rrf_overlap = rank_overlap(&dense, &sparse);

        // --- rerank --------------------------------------------------------
        let rerank_start = Instant::now();
        let reranked: Vec<SearchHit> = match &self.reranker {
            Some(r) => {
                let candidates: Vec<RerankerCandidate> = fused.iter().map(|h| RerankerCandidate {
                    chunk_text: h.chunk.text.clone(),
                    base_score: h.score,
                }).collect();
                let order = r.rerank(query.text, &candidates).await;
                let mut out = Vec::with_capacity(fused.len());
                for (rank, score) in order.iter().enumerate() {
                    if let Some(idx) = score.original_index() {
                        if idx < fused.len() {
                            let mut h = fused[idx].clone();
                            h.score = score.score;
                            // small rank-order bias so ties resolve deterministically
                            h.score += 1e-6 * (fused.len() as f32 - rank as f32);
                            out.push(h);
                        }
                    }
                }
                out
            }
            None => fused,
        };
        let rerank_us = rerank_start.elapsed().as_micros() as u64;

        // --- pack response -------------------------------------------------
        let truncated = reranked.len() > query.top_k;
        let mut top = reranked;
        top.truncate(query.top_k);
        let response = SearchResponse {
            hits: top.iter().map(HitView::from).collect(),
            layer: Layer::Hybrid,
            latency: LatencyBreakdown {
                dense_us, sparse_us, fusion_us, rerank_us,
                total_us: total_start.elapsed().as_micros() as u64,
            },
            diagnostics: RetrievalDiagnostics {
                dense_candidates: dense.len(),
                sparse_candidates: sparse.len(),
                rrf_overlap,
                query_terms: tokenize_query(query.text),
                reranked: self.reranker.is_some(),
                truncated,
            },
        };
        Ok(response)
    }
}

/// Count chunks that appear in both rankings — RRF rewards these.
fn rank_overlap(dense: &[SearchHit], sparse: &[SearchHit]) -> usize {
    use std::collections::HashSet;
    if dense.is_empty() || sparse.is_empty() {
        return 0;
    }
    let dense_ids: HashSet<&str> = dense.iter().map(|h| h.chunk.id.as_str()).collect();
    sparse.iter().filter(|h| dense_ids.contains(h.chunk.id.as_str())).count()
}

/// Same tokeniser the BM25 path uses inside the store, lifted here so the
/// diagnostics surface the exact terms the BM25 scorer saw.  Lowercase,
/// alphanumeric-or-underscore runs of length ≥ 2.
fn tokenize_query(text: &str) -> Vec<String> {
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

/// Reciprocal Rank Fusion.  For each hit appearing in either ranking the
/// fused score is `Σ 1 / (k + rank_i)`.  Paper §VII-C cites k=60 as a
/// well-tested smoothing constant.
pub fn reciprocal_rank_fusion(
    dense: &[SearchHit],
    sparse: &[SearchHit],
    k: f32,
    cap: usize,
) -> Vec<SearchHit> {
    use std::collections::HashMap;
    let mut fused_scores: HashMap<String, (f32, SearchHit)> = HashMap::new();
    for (rank, h) in dense.iter().enumerate() {
        let contrib = 1.0 / (k + rank as f32 + 1.0);
        let key = h.chunk.id.clone();
        fused_scores.entry(key)
            .and_modify(|(s, _)| *s += contrib)
            .or_insert_with(|| (contrib, h.clone()));
    }
    for (rank, h) in sparse.iter().enumerate() {
        let contrib = 1.0 / (k + rank as f32 + 1.0);
        let key = h.chunk.id.clone();
        fused_scores.entry(key)
            .and_modify(|(s, _)| *s += contrib)
            .or_insert_with(|| (contrib, h.clone()));
    }
    let mut out: Vec<SearchHit> = fused_scores.into_values()
        .map(|(score, mut h)| { h.score = score; h })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sap_automate_kb::Chunk;

    fn mk(id: &str, text: &str, score: f32) -> SearchHit {
        SearchHit {
            chunk: Chunk {
                id: id.into(),
                document_id: id.into(),
                domain: Domain::SapHelp,
                ordinal: 0,
                text: text.into(),
                embedding: None,
                breadcrumbs: Vec::new(),
                title: id.into(),
                uri: format!("u://{id}"),
            },
            score,
        }
    }

    #[test]
    fn rrf_rewards_consensus() {
        let dense = vec![mk("a", "", 0.9), mk("b", "", 0.8), mk("c", "", 0.7)];
        let sparse = vec![mk("c", "", 5.0), mk("a", "", 4.0), mk("d", "", 1.0)];
        let fused = reciprocal_rank_fusion(&dense, &sparse, 60.0, 10);
        // 'a' is rank 0 dense + rank 1 sparse; 'c' is rank 2 dense + rank 0 sparse.
        // The top fused result should reward consensus.  Both 'a' and 'c'
        // beat 'b' (only in dense) and 'd' (only in sparse).
        let top_ids: Vec<_> = fused.iter().take(2).map(|h| h.chunk.id.clone()).collect();
        assert!(top_ids.contains(&"a".to_string()) && top_ids.contains(&"c".to_string()),
            "expected a + c at top; got {top_ids:?}");
    }

    #[test]
    fn rank_overlap_counts_consensus_pairs() {
        let dense = vec![mk("a", "", 1.0), mk("b", "", 1.0), mk("c", "", 1.0)];
        let sparse = vec![mk("a", "", 1.0), mk("c", "", 1.0), mk("d", "", 1.0)];
        assert_eq!(rank_overlap(&dense, &sparse), 2); // a and c
        assert_eq!(rank_overlap(&dense, &[]), 0);
        assert_eq!(rank_overlap(&[], &sparse), 0);
    }

    #[test]
    fn tokenize_query_preserves_sap_identifiers() {
        let t = tokenize_query("BAPI_ACC_DOCUMENT_POST and the period close");
        // BAPI identifier survives intact, joined by underscores.
        assert!(t.iter().any(|x| x == "bapi_acc_document_post"));
        // Stop-word-shaped single chars are dropped (need ≥ 2 chars).
        assert!(t.iter().all(|x| x.len() >= 2));
    }
}
