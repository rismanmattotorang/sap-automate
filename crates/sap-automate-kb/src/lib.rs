//! SAP-Automate knowledge base.
//!
//! Phase 1A: introduces the `KnowledgeStore` trait so backends are pluggable.
//! Two implementations ship: `InMemoryKb` (dev/test) and `QdrantStore`
//! (production, behind the `qdrant` feature).  Both implement the same
//! `KnowledgeStore` async contract so the RAG engine and ingestion pipeline
//! see one surface.

pub mod schema;
pub mod store;
pub mod doc_tree;

#[cfg(feature = "qdrant")]
pub mod qdrant;

pub use schema::{
    Chunk, ChunkId, Document, DocumentId, Domain, content_hash,
};
pub use store::{InMemoryKb, KnowledgeStore, Layer, SearchHit, SearchQuery, StoreError, UpsertBatch, UpsertStats};
pub use doc_tree::{DocTreeNode, DocumentTree, build_document_tree};

#[cfg(feature = "qdrant")]
pub use qdrant::QdrantStore;
