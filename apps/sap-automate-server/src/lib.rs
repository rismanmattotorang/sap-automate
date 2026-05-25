//! Library surface for the SAP-Automate server binary.
//!
//! Exposes the same builder functions the binary uses internally so
//! integration tests can construct a server in-process — no subprocess,
//! no cold-start seeding cost.

pub mod context;
pub mod prompts;
pub mod resources;
pub mod seed;
pub mod tools;

use std::sync::Arc;
use std::time::Duration;

use mcp_server::Server;
use sap_automate_adt::{AdtClient, AdtDestination, MockAdtClient};
use sap_automate_graph::InMemoryGraph;
use sap_automate_ingest::{EmbeddingClient, MockEmbedder};
use sap_automate_kb::{InMemoryKb, KnowledgeStore};
use sap_automate_rag::{GraphEngine, MockReranker, RagEngine};
use sap_automate_rfc::{MetadataCache, MockSapClient, SapClient};
use sap_automate_skills::SkillRegistry;

pub use context::ServerContext;

/// How the test harness wants its context built.
#[derive(Clone)]
pub struct TestServerOptions {
    pub read_only: bool,
    pub metadata_cache_ttl: Duration,
    pub seed_kb: bool,
    pub embedding_dim: usize,
    pub agents_md: Option<String>,
}

impl Default for TestServerOptions {
    fn default() -> Self {
        Self {
            read_only: true,
            metadata_cache_ttl: Duration::from_secs(300),
            seed_kb: false,
            embedding_dim: 64,
            agents_md: None,
        }
    }
}

/// Build a ready-to-run `Server` for integration tests.  Identical wiring
/// to `main.rs`, minus the network transport setup and (optionally) the
/// KB seed step.
pub async fn build_test_server(
    opts: TestServerOptions,
) -> (Server, Arc<ServerContext>) {
    let store: Arc<dyn KnowledgeStore> = Arc::new(InMemoryKb::new());
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder::new(opts.embedding_dim));
    if opts.seed_kb {
        seed::populate_with_embeddings(&store, embedder.as_ref())
            .await
            .expect("seed");
    }
    let rag = Arc::new(RagEngine::new(store.clone()).with_reranker(Arc::new(MockReranker::new())));

    let kg = Arc::new(InMemoryGraph::with_demo_corpus());
    let graph_engine = Arc::new(GraphEngine::new(kg));

    let inner = MockSapClient::new(4, serde_json::json!({}));
    let metadata_cache = MetadataCache::new(inner.clone(), opts.metadata_cache_ttl);
    let sap_client: Arc<dyn SapClient> = metadata_cache.clone();

    let adt_destination = AdtDestination::mock("test".to_string());
    let adt_client: Arc<dyn AdtClient> = MockAdtClient::new(adt_destination);

    let ctx = Arc::new(ServerContext {
        rag,
        graph: graph_engine,
        embedder,
        sap_client,
        metadata_cache: Some(metadata_cache),
        adt_client,
        read_only: opts.read_only,
        agents_md: opts.agents_md.clone(),
    });

    let policy = if opts.read_only {
        mcp_server::ExposurePolicy::ReadOnlyOnly
    } else {
        mcp_server::ExposurePolicy::All
    };
    let mut builder = Server::builder("sap-automate-test-server", env!("CARGO_PKG_VERSION"))
        .exposure(policy)
        .instructions("integration test".to_string());

    for desc in tools::rag_tools(&ctx) { builder = builder.tool(desc); }
    for desc in tools::sap_tools(&ctx) { builder = builder.tool(desc); }
    for desc in tools::adt_tools(&ctx) { builder = builder.tool(desc); }
    for desc in tools::graph_tools(&ctx) { builder = builder.tool(desc); }
    for desc in tools::workflow_tools(&ctx) { builder = builder.tool(desc); }
    for desc in resources::all(&ctx) { builder = builder.resource(desc); }
    let skills = SkillRegistry::new();
    for desc in prompts::all(&skills) { builder = builder.prompt(desc); }

    (builder.build(), ctx)
}
