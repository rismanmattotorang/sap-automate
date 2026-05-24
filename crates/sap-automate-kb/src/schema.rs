//! Document and chunk schema (paper §VI-D).
//!
//! Every Help Portal page, ABAP object, BPMN process, and LeanIX fact sheet
//! is a first-class `Document` with stable URI + rich metadata.  Documents
//! are split into one or more `Chunk`s for embedding and retrieval; each
//! chunk preserves its parent document linkage and breadcrumb so reranking
//! and citation rendering work without an extra DB round-trip.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    SapHelp,
    Abap,
    Bpmn,
    Leanix,
}

impl Domain {
    /// Canonical Qdrant collection name for the domain.
    pub fn collection(self) -> &'static str {
        match self {
            Domain::SapHelp => "sap_help",
            Domain::Abap => "abap",
            Domain::Bpmn => "bpmn",
            Domain::Leanix => "leanix",
        }
    }
}

/// Stable document identifier.  Format: `<domain>:<external-id>` — e.g.
/// `sap_help:FI/period-close` or `abap:ZFIN/ZFIN_POST_JE`.
pub type DocumentId = String;
pub type ChunkId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub domain: Domain,
    pub uri: String,
    pub title: String,
    pub body: String,
    /// Hierarchical path from the source ("Finance > General Ledger > Period
    /// Close").  The chunker prepends a normalised breadcrumb to every chunk
    /// (paper §VI-E, "contextual breadcrumb").
    #[serde(default)]
    pub breadcrumbs: Vec<String>,
    /// SHA-256 of `body` used for change detection on re-crawl.
    pub content_hash: String,
    /// Source-side ETag, if the upstream HTTP server provided one.
    #[serde(default)]
    pub etag: Option<String>,
    /// Last seen by the crawler (ISO-8601).
    #[serde(default)]
    pub last_crawled: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl Document {
    /// Convenience constructor that fills in the content hash.
    pub fn new(
        id: impl Into<String>,
        domain: Domain,
        uri: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        let body = body.into();
        let content_hash = content_hash(&body);
        Self {
            id: id.into(),
            domain,
            uri: uri.into(),
            title: title.into(),
            body,
            breadcrumbs: Vec::new(),
            content_hash,
            etag: None,
            last_crawled: None,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: ChunkId,
    pub document_id: DocumentId,
    pub domain: Domain,
    /// 0-based index within the parent document.
    pub ordinal: u32,
    /// The text actually sent to the embedding model (breadcrumb +
    /// contextual prefix + body slice).
    pub text: String,
    /// Optional pre-computed embedding.  `None` until the embedding pipeline
    /// runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// Mirrored from the parent document so reranking can use them without a
    /// second lookup.
    #[serde(default)]
    pub breadcrumbs: Vec<String>,
    pub title: String,
    pub uri: String,
}

pub fn content_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}
