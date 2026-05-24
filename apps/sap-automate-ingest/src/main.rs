//! SAP-Automate ingestion CLI.
//!
//! Crawls a local directory of HTML files (or, in HTTP mode, fetches a list
//! of URLs) and ingests them into the configured KnowledgeStore.  This
//! binary is the Phase 1A acceptance harness: running it against a small
//! Help Portal snapshot should leave the store able to answer intent
//! queries like "period close" by returning the right page.

use clap::{Parser, ValueEnum};
use sap_automate_ingest::{
    HelpPortalCrawler, IngestionPipeline, MockEmbedder, OpenAiEmbedder,
    EmbeddingClient,
};
use sap_automate_kb::{InMemoryKb, KnowledgeStore, SearchQuery};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Backend {
    /// In-memory store (default; offline-friendly).
    Memory,
    /// Qdrant REST.  Requires --qdrant-url.
    Qdrant,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Embedder {
    /// Deterministic hash-based embedder (default).  No network.
    Mock,
    /// OpenAI / OpenAI-compatible.  Requires --openai-api-key and --openai-model.
    Openai,
}

#[derive(Parser)]
#[command(
    name = "sap-automate-ingest",
    about = "Crawl, chunk, embed, and upsert SAP knowledge into the configured store."
)]
struct Cli {
    /// Directory of *.html files to ingest.
    #[arg(long)]
    input_dir: String,

    /// Knowledge store backend.
    #[arg(long, value_enum, default_value_t = Backend::Memory)]
    backend: Backend,

    /// Qdrant REST base URL (e.g. http://localhost:6333).
    #[arg(long, default_value = "http://localhost:6333")]
    qdrant_url: String,

    /// Embedder selection.
    #[arg(long, value_enum, default_value_t = Embedder::Mock)]
    embedder: Embedder,

    /// Embedding vector dimension.
    #[arg(long, default_value_t = 256)]
    embedding_dim: usize,

    /// OpenAI API base URL (for `--embedder openai`).
    #[arg(long, default_value = "https://api.openai.com/v1")]
    openai_base_url: String,

    /// OpenAI API key.  Defaults to env $OPENAI_API_KEY.
    #[arg(long)]
    openai_api_key: Option<String>,

    /// OpenAI embedding model name.
    #[arg(long, default_value = "text-embedding-3-large")]
    openai_model: String,

    /// Optional smoke-test query to run after ingestion.
    #[arg(long)]
    verify_query: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();

    let embedder: Arc<dyn EmbeddingClient> = match cli.embedder {
        Embedder::Mock => Arc::new(MockEmbedder::new(cli.embedding_dim)),
        Embedder::Openai => {
            let key = cli.openai_api_key.clone().or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .ok_or_else(|| anyhow::anyhow!("openai embedder requires --openai-api-key or $OPENAI_API_KEY"))?;
            Arc::new(OpenAiEmbedder::new(cli.openai_base_url.clone(), key, cli.openai_model.clone(), cli.embedding_dim))
        }
    };

    let store: Arc<dyn KnowledgeStore> = match cli.backend {
        Backend::Memory => Arc::new(InMemoryKb::new()),
        Backend::Qdrant => {
            let q = sap_automate_kb::QdrantStore::new(cli.qdrant_url.clone(), cli.embedding_dim);
            q.ensure_collections().await?;
            Arc::new(q)
        }
    };

    println!(
        "→ Backend: {:?}  Embedder: {:?} (dim={})",
        cli.backend, cli.embedder, cli.embedding_dim
    );

    let crawler = HelpPortalCrawler::new();
    let documents = crawler.crawl_directory(&cli.input_dir).await?;
    println!("→ Crawled {} document(s) from {}", documents.len(), cli.input_dir);

    let pipeline = IngestionPipeline::new(Arc::clone(&embedder), Arc::clone(&store));
    let report = pipeline.ingest(documents).await?;
    println!("→ Ingested {} document(s), {} chunk(s)", report.documents, report.chunks);
    println!("→ KB now holds {} chunk(s)", store.chunk_count().await?);

    if let Some(query) = cli.verify_query.as_deref() {
        let q_vec = embedder.embed(&[query.to_string()]).await?.into_iter().next().unwrap();
        let hits = store.search(SearchQuery::text(query, 5).with_embedding(q_vec)).await?;
        println!("\n== verify_query = {query:?} ({} hit(s))", hits.len());
        for h in hits {
            println!(
                "  - [{:.3}] {} :: {}",
                h.score,
                h.chunk.title,
                truncate(&h.chunk.text, 120),
            );
        }
    }

    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
