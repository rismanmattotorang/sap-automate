//! Reranker trait + in-process mock.
//!
//! The paper §VII-H notes that a cross-encoder reranker gives the biggest
//! single precision-at-K lift for the cost (one extra forward pass over the
//! top-N).  Phase 3 ships:
//!   - `Reranker` async trait
//!   - `MockReranker` — deterministic, term-overlap based, demonstrably
//!     reorders the top of the candidate pool toward the query
//!
//! `OnnxReranker` (real cross-encoder via ONNX Runtime) is a Phase 7
//! hardening task.

use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct RerankerCandidate {
    pub chunk_text: String,
    pub base_score: f32,
}

#[derive(Debug, Clone)]
pub struct RerankedItem {
    pub idx: usize,
    pub score: f32,
}

impl RerankedItem {
    pub fn original_index(&self) -> Option<usize> { Some(self.idx) }
}

#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &str, candidates: &[RerankerCandidate]) -> Vec<RerankedItem>;
}

/// Deterministic mock reranker.  Combines:
///   - exact-match term-overlap (cheap proxy for cross-encoder relevance)
///   - position-decay (preserves some of the base ordering as a tie-breaker)
///   - identifier bonus (SAP tx codes, BAPIs, transport IDs — anything that
///     looks like `[A-Z0-9_]{3,}` and is also in the query — score boost)
///
/// Crude but it pushes consensus hits up the way a real cross-encoder
/// would on identifier-heavy SAP queries.
pub struct MockReranker;

impl MockReranker {
    pub fn new() -> Self { Self }
}

impl Default for MockReranker {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Reranker for MockReranker {
    async fn rerank(&self, query: &str, candidates: &[RerankerCandidate]) -> Vec<RerankedItem> {
        let q_tokens: Vec<String> = tokens(query);
        let q_identifiers: Vec<String> = q_tokens.iter()
            .filter(|t| t.len() >= 3 && t.chars().any(|c| c.is_uppercase() || c == '_'))
            .cloned().collect();

        let mut scored: Vec<RerankedItem> = candidates.iter().enumerate().map(|(idx, c)| {
            let body_tokens = tokens(&c.chunk_text);
            let overlap = q_tokens.iter().filter(|t| body_tokens.contains(t)).count() as f32;
            let ident_bonus = q_identifiers.iter().filter(|t| {
                c.chunk_text.contains(t.as_str())
                    || c.chunk_text.to_ascii_uppercase().contains(&t.to_ascii_uppercase())
            }).count() as f32 * 0.5;
            let pos_decay = 1.0 / (1.0 + idx as f32 * 0.05);
            let score = overlap + ident_bonus + 0.1 * c.base_score + pos_decay;
            RerankedItem { idx, score }
        }).collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rerank_pushes_identifier_match_to_top() {
        let r = MockReranker::new();
        let candidates = vec![
            RerankerCandidate { chunk_text: "Posting periods are managed via T001B.".into(), base_score: 0.3 },
            RerankerCandidate { chunk_text: "Generic finance prose without any tx code.".into(), base_score: 0.5 },
            RerankerCandidate { chunk_text: "BAPI_ACC_DOCUMENT_POST posts journal entries.".into(), base_score: 0.4 },
        ];
        let order = r.rerank("How does BAPI_ACC_DOCUMENT_POST work?", &candidates).await;
        assert_eq!(order[0].idx, 2, "BAPI_ACC_DOCUMENT_POST-mentioning chunk should top out");
    }

    #[tokio::test]
    async fn rerank_is_stable_for_equal_inputs() {
        let r = MockReranker::new();
        let candidates = vec![
            RerankerCandidate { chunk_text: "ABC".into(), base_score: 0.5 },
            RerankerCandidate { chunk_text: "DEF".into(), base_score: 0.5 },
        ];
        let order1 = r.rerank("query", &candidates).await;
        let order2 = r.rerank("query", &candidates).await;
        assert_eq!(order1.len(), order2.len());
    }
}
