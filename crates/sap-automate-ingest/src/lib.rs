//! SAP-Automate ingestion pipeline.
//!
//! Phase 1A scope (paper §X-B):
//!   - HTML extraction for the SAP Help Portal page model
//!   - chunker that prepends a normalised breadcrumb (paper §VI-E)
//!   - pluggable `EmbeddingClient` with a deterministic mock + OpenAI adapter
//!   - `IngestionPipeline` orchestrator: crawl -> parse -> chunk -> embed ->
//!     upsert

pub mod chunker;
pub mod crawler;
pub mod embed;
pub mod pipeline;

pub use chunker::{chunk_document, ChunkerConfig};
pub use crawler::{HelpPortalCrawler, ParsedPage, parse_help_portal_html};
pub use embed::{EmbeddingClient, EmbeddingError, MockEmbedder, OpenAiEmbedder};
pub use pipeline::{IngestionPipeline, IngestionReport};
