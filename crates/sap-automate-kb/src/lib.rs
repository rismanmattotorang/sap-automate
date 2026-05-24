//! SAP-Automate knowledge base.
//!
//! Phase 1A (paper §X-B) populates this crate with:
//! - Document schema for Help Portal, ABAP, BPMN, LeanIX (`schema::Document`).
//! - Qdrant client wrapper for the four primary collections.
//! - Postgres-backed metadata store.
//! - ArangoDB graph (Phase 5A).
//!
//! For Phase 1 we expose just the document schema and an in-memory KB suitable
//! for sample apps and unit tests.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    /// SAP Help Portal pages.
    SapHelp,
    /// ABAP objects (programs, classes, function modules).
    Abap,
    /// Signavio BPMN processes.
    Bpmn,
    /// LeanIX EAM fact sheets.
    Leanix,
}

/// First-class retrieval unit (PageIndex-style; see paper §VI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub domain: Domain,
    pub uri: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub breadcrumbs: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// In-memory KB.  Real deployments back this with Qdrant + Postgres; this
/// implementation exists so the sample apps and tests can drive end-to-end
/// flows without external services.
#[derive(Default)]
pub struct InMemoryKb {
    docs: RwLock<HashMap<String, Document>>,
}

impl InMemoryKb {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&self, doc: Document) {
        self.docs.write().unwrap().insert(doc.id.clone(), doc);
    }

    pub fn get(&self, id: &str) -> Option<Document> {
        self.docs.read().unwrap().get(id).cloned()
    }

    pub fn search(&self, query: &str, domain: Option<Domain>, top_k: usize) -> Vec<Document> {
        let q = query.to_lowercase();
        let docs = self.docs.read().unwrap();
        let mut hits: Vec<(usize, Document)> = docs
            .values()
            .filter(|d| domain.map_or(true, |dom| d.domain == dom))
            .filter_map(|d| {
                let score = score_naive(&q, d);
                if score > 0 { Some((score, d.clone())) } else { None }
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        hits.into_iter().take(top_k).map(|(_, d)| d).collect()
    }

    pub fn len(&self) -> usize { self.docs.read().unwrap().len() }
    pub fn is_empty(&self) -> bool { self.docs.read().unwrap().is_empty() }
}

/// Term-frequency scorer.  Placeholder for the BM25 + dense hybrid in §VII;
/// kept intentionally simple so it has zero external dependencies.
fn score_naive(query_lc: &str, doc: &Document) -> usize {
    let terms: Vec<&str> = query_lc.split_whitespace().collect();
    let haystack = format!(
        "{} {} {}",
        doc.title.to_lowercase(),
        doc.body.to_lowercase(),
        doc.breadcrumbs.join(" ").to_lowercase(),
    );
    terms.iter().map(|t| haystack.matches(t).count()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_expected_doc() {
        let kb = InMemoryKb::new();
        kb.insert(Document {
            id: "abap.zfoo".into(),
            domain: Domain::Abap,
            uri: "abap-obj://Z_FOO/ZFOO".into(),
            title: "ZFOO".into(),
            body: "Report ZFOO updates the material master.".into(),
            breadcrumbs: vec!["Z_FOO".into()],
            metadata: HashMap::new(),
        });
        let hits = kb.search("material master", None, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "abap.zfoo");
    }
}
