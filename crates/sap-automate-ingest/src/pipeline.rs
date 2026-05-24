//! Ingestion pipeline orchestrator.
//!
//! Walks a stream of documents through chunking → embedding → KB upsert.
//! Embeddings are batched to amortise the (often-dominant) embedding-API
//! latency cost; the default batch size of 32 matches the paper §VI-F
//! recommendation.

use crate::chunker::{chunk_document, ChunkerConfig};
use crate::embed::{EmbeddingClient, EmbeddingError};
use sap_automate_kb::{Document, KnowledgeStore, UpsertBatch};
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("embedding: {0}")]
    Embedding(#[from] EmbeddingError),
    #[error("store: {0}")]
    Store(#[from] sap_automate_kb::StoreError),
}

#[derive(Debug, Default, Clone)]
pub struct IngestionReport {
    pub documents: usize,
    pub chunks: usize,
}

pub struct IngestionPipeline {
    pub embedder: Arc<dyn EmbeddingClient>,
    pub store: Arc<dyn KnowledgeStore>,
    pub chunker: ChunkerConfig,
    /// Maximum chunks per embedding API call.
    pub batch_size: usize,
}

impl IngestionPipeline {
    pub fn new(embedder: Arc<dyn EmbeddingClient>, store: Arc<dyn KnowledgeStore>) -> Self {
        Self { embedder, store, chunker: ChunkerConfig::default(), batch_size: 32 }
    }

    pub fn with_chunker(mut self, cfg: ChunkerConfig) -> Self {
        self.chunker = cfg;
        self
    }

    pub fn with_batch_size(mut self, n: usize) -> Self {
        self.batch_size = n.max(1);
        self
    }

    /// Ingest a collection of documents.  Documents are processed
    /// sequentially; chunks are embedded in batches and upserted per
    /// document.
    pub async fn ingest(&self, documents: Vec<Document>) -> Result<IngestionReport, PipelineError> {
        let mut report = IngestionReport::default();
        for doc in documents {
            let chunks = chunk_document(&doc, &self.chunker);
            if chunks.is_empty() {
                warn!(id = %doc.id, "document produced no chunks; skipping");
                continue;
            }
            report.documents += 1;
            report.chunks += chunks.len();

            // Embed in batches.
            let mut embedded = Vec::with_capacity(chunks.len());
            for batch in chunks.chunks(self.batch_size) {
                let texts: Vec<String> = batch.iter().map(|c| c.text.clone()).collect();
                let vectors = self.embedder.embed(&texts).await?;
                for (mut chunk, vec) in batch.iter().cloned().zip(vectors.into_iter()) {
                    chunk.embedding = Some(vec);
                    embedded.push(chunk);
                }
            }

            self.store.upsert(UpsertBatch {
                documents: vec![doc],
                chunks: embedded,
            }).await?;
        }
        info!(?report, "ingestion complete");
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use sap_automate_kb::{Domain, InMemoryKb, SearchQuery};

    #[tokio::test]
    async fn end_to_end_ingest_then_search() {
        let store = Arc::new(InMemoryKb::new());
        let embedder = Arc::new(MockEmbedder::new(128));
        let pipeline = IngestionPipeline::new(embedder.clone(), store.clone());

        let docs = vec![
            {
                let mut d = Document::new(
                    "sap_help:FI/period-close", Domain::SapHelp,
                    "sap-help://FI/period-close",
                    "Period-End Close in SAP FI",
                    "Open and close posting periods via transaction T001B. Execute foreign-currency revaluation. Post accruals and deferrals; reconcile BSEG to FAGLFLEXA.",
                );
                d.breadcrumbs = vec!["Finance".into(), "General Ledger".into()];
                d
            },
            Document::new(
                "sap_help:MM/goods-movement", Domain::SapHelp,
                "sap-help://MM/goods-movement",
                "Goods Movement Posting",
                "Post goods movements with transaction MIGO. Movement types 101 receipt, 102 reversal, 122 return.",
            ),
        ];

        let report = pipeline.ingest(docs).await.unwrap();
        assert_eq!(report.documents, 2);
        assert!(report.chunks >= 2);

        // Search by intent: query embedded to vector, store returns the right page.
        let q_vec = embedder.embed(&vec!["period close FAGLFLEXA reconciliation".to_string()]).await.unwrap();
        let hits = store.search(
            SearchQuery::text("period close FAGLFLEXA reconciliation", 5)
                .with_embedding(q_vec[0].clone())
                .with_domain(Domain::SapHelp),
        ).await.unwrap();

        assert!(!hits.is_empty());
        // Top hit should be the period-close doc.
        assert!(hits[0].chunk.document_id.contains("period-close"));
    }
}
