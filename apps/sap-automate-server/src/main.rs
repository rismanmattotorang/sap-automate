//! SAP-Automate MCP server binary.
//!
//! Phase 2: this binary is the upgraded SAP-Automate server, incorporating
//! the design insights from the reference projects studied in
//! `docs/COMPARISON.md`:
//!
//! - **8 new SAP tools** (`sap.system.info`, `sap.rfc.search`,
//!   `sap.rfc.metadata`, `sap.rfc.bulk_metadata`, `sap.rfc.call`,
//!   `sap.table.read`, `sap.table.structure`, `sap.docs.search`)
//!   alongside the four existing RAG tools.
//! - **Resources**: `sap-system://info`, `sap-table://{name}/structure`,
//!   `sap-rfc://{name}`, plus an AGENTS-md guardrails resource.
//! - **Prompts**: `sap.review-rfc-call`, `sap.transport-impact-analysis`.
//! - **Read-only mode by default** (CData pattern); explicit
//!   `--enable-writes` flips the safety gate.
//! - **AGENTS.md loader** (MDK pattern): per-project guardrails surfaced
//!   in `initialize.instructions`.
//! - **Structured SAP error taxonomy** mapped to MCP JSON-RPC error codes.

mod context;
mod seed;
mod tools;
mod resources;
mod prompts;

use clap::Parser;
use mcp_server::Server;
use mcp_transport::StdioTransport;
use sap_automate_ingest::{EmbeddingClient, MockEmbedder};
use sap_automate_kb::{InMemoryKb, KnowledgeStore};
use sap_automate_rag::RagEngine;
use sap_automate_rfc::{
    Credentials, CredentialProvider, CredentialSource, EnvCredentialProvider,
    LayeredCredentialProvider, MockSapClient, SapClient, StaticCredentialProvider,
};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use context::ServerContext;

#[derive(Parser, Clone)]
#[command(
    name = "sap-automate-server",
    about = "SAP-Automate MCP server with RAG + RFC tools, resources, and prompts.",
    version,
)]
struct Cli {
    /// Disable read-only safety; allow MCP tools to call write-side RFCs.
    /// Equivalent to setting `SAP_AUTOMATE_ENABLE_WRITES=1`.
    #[arg(long)]
    enable_writes: bool,

    /// Maximum concurrent SAP calls (paper §IV-G).
    #[arg(long, default_value_t = 8)]
    pool_size: usize,

    /// Path to an AGENTS.md guardrails file (MDK pattern).  Surfaced in
    /// `initialize.instructions` so MCP clients can apply project-local
    /// policy.  Defaults to `./AGENTS.md` if present.
    #[arg(long)]
    agents_md: Option<String>,

    /// Embedding vector dimension for the in-memory KB.
    #[arg(long, default_value_t = 256)]
    embedding_dim: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let read_only = !cli.enable_writes && std::env::var("SAP_AUTOMATE_ENABLE_WRITES").ok().as_deref() != Some("1");

    // Build the KB + embedder.
    let store: Arc<dyn KnowledgeStore> = Arc::new(InMemoryKb::new());
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder::new(cli.embedding_dim));
    seed::populate_with_embeddings(&store, embedder.as_ref()).await?;
    let rag = Arc::new(RagEngine::new(store.clone()));

    // Build the SAP client.  Credentials are layered (env first, static
    // fallback for the offline demo).
    let creds_provider = LayeredCredentialProvider::new()
        .add(Arc::new(EnvCredentialProvider::new()))
        .add(Arc::new(StaticCredentialProvider::new(Credentials {
            ashost: "mock.sap.example".into(),
            sysnr: "00".into(),
            client: "100".into(),
            user: "DEMO".into(),
            password: "redacted".into(),
            language: "EN".into(),
            saprouter: None,
            source: CredentialSource::Static,
        })));
    let creds = creds_provider.fetch().await
        .map_err(|e| anyhow::anyhow!("credential resolution failed: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("no credentials available"))?;
    tracing::info!(identity = %creds.redacted(), "SAP identity resolved");

    let sap_client: Arc<dyn SapClient> = MockSapClient::new(cli.pool_size, creds.redacted());

    // AGENTS.md guardrails.
    let agents_md = load_agents_md(cli.agents_md.as_deref()).await;

    let ctx = Arc::new(ServerContext {
        rag,
        embedder,
        sap_client,
        read_only,
        agents_md: agents_md.clone(),
    });

    let server = build_server(ctx.clone(), &agents_md, read_only);
    tracing::info!(
        read_only = read_only,
        pool_size = cli.pool_size,
        embedding_dim = cli.embedding_dim,
        tools = 12,
        "SAP-Automate server configured"
    );
    server.run(StdioTransport::from_stdio()).await?;
    Ok(())
}

fn build_server(ctx: Arc<ServerContext>, agents_md: &Option<String>, read_only: bool) -> Server {
    let mut builder = Server::builder("sap-automate-server", env!("CARGO_PKG_VERSION"))
        .instructions(build_instructions(agents_md, read_only));

    // Existing RAG tools (Phase 1 + 1A).
    for desc in tools::rag_tools(&ctx) { builder = builder.tool(desc); }

    // New SAP tools (Phase 2).
    for desc in tools::sap_tools(&ctx) { builder = builder.tool(desc); }

    // Resources.
    for desc in resources::all(&ctx) { builder = builder.resource(desc); }

    // Prompts.
    for desc in prompts::all() { builder = builder.prompt(desc); }

    builder.build()
}

fn build_instructions(agents_md: &Option<String>, read_only: bool) -> String {
    let mut s = String::new();
    s.push_str(
        "SAP-Automate MCP server (Phase 2).\n\
         Tools: abap.search, bpmn.find_process, eam.search_apps, sap.help.search, \
         sap.system.info, sap.rfc.search, sap.rfc.metadata, sap.rfc.bulk_metadata, \
         sap.rfc.call, sap.table.read, sap.table.structure, sap.docs.search.\n\
         Resources: sap-system://info, sap-table://{name}/structure, sap-rfc://{name}, \
         agents://guardrails.\n\
         Prompts: sap.review-rfc-call, sap.transport-impact-analysis.\n",
    );
    if read_only {
        s.push_str("Mode: READ-ONLY. Write-side RFCs (e.g. BAPI_SALESORDER_CREATEFROMDAT2, BAPI_ACC_DOCUMENT_POST) are blocked. Pass --enable-writes to allow.\n");
    } else {
        s.push_str("Mode: WRITE-ENABLED. Treat every sap.rfc.call invocation as authorised.\n");
    }
    if let Some(md) = agents_md {
        s.push_str("\n--- AGENTS.md guardrails ---\n");
        s.push_str(md);
    }
    s
}

async fn load_agents_md(explicit_path: Option<&str>) -> Option<String> {
    let candidates: Vec<String> = match explicit_path {
        Some(p) => vec![p.to_string()],
        None => vec!["AGENTS.md".to_string(), ".sap-automate/AGENTS.md".to_string()],
    };
    for path in candidates {
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            tracing::info!(path, bytes = content.len(), "loaded AGENTS.md guardrails");
            return Some(content);
        }
    }
    None
}
