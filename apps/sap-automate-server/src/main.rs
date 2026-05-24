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
use sap_automate_adt::{AdtClient, AdtDestination, MockAdtClient};
use sap_automate_skills::SkillRegistry;
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

    /// ADT destination name (fr0ster/mcp-abap-adt pattern). Defaults to
    /// "default" → MockAdtClient. Real wiring (HttpAdtClient) is Phase 7.
    #[arg(long)]
    adt_destination: Option<String>,
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
    let rag = Arc::new(
        RagEngine::new(store.clone())
            .with_reranker(Arc::new(sap_automate_rag::MockReranker::new()))
    );

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

    // ADT client — defaults to MockAdtClient so the binary runs offline.
    let adt_destination = AdtDestination::mock(cli.adt_destination.clone().unwrap_or_else(|| "default".into()));
    let adt_client: Arc<dyn AdtClient> = MockAdtClient::new(adt_destination);

    // AGENTS.md guardrails.
    let agents_md = load_agents_md(cli.agents_md.as_deref()).await;

    // Skills auto-discovery — marianfoo pattern.
    let mut skills = SkillRegistry::new();
    let skill_paths: Vec<std::path::PathBuf> = vec![
        std::path::PathBuf::from("./skills"),
        std::path::PathBuf::from("./.sap-automate/skills"),
        dirs_config_path("sap-automate/skills"),
    ];
    let loaded = skills.scan_paths(&skill_paths).await.unwrap_or(0);
    if loaded > 0 {
        tracing::info!(skills = loaded, "loaded agentic skills");
    }

    let ctx = Arc::new(ServerContext {
        rag,
        embedder,
        sap_client,
        adt_client,
        read_only,
        agents_md: agents_md.clone(),
    });

    let server = build_server(ctx.clone(), &agents_md, read_only, &skills);
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

fn build_server(ctx: Arc<ServerContext>, agents_md: &Option<String>, read_only: bool, skills: &SkillRegistry) -> Server {
    let policy = if read_only {
        mcp_server::ExposurePolicy::ReadOnlyOnly
    } else {
        mcp_server::ExposurePolicy::All
    };
    let mut builder = Server::builder("sap-automate-server", env!("CARGO_PKG_VERSION"))
        .exposure(policy)
        .instructions(build_instructions(agents_md, read_only));

    // Existing RAG tools (Phase 1 + 1A) — all read-only.
    for desc in tools::rag_tools(&ctx) { builder = builder.tool(desc); }

    // RFC + table tools (Phase 2).
    for desc in tools::sap_tools(&ctx) { builder = builder.tool(desc); }

    // ADT tools (Phase 2 finalisation — informed by mario-andreschak +
    // fr0ster/mcp-abap-adt).
    for desc in tools::adt_tools(&ctx) { builder = builder.tool(desc); }

    // Resources.
    for desc in resources::all(&ctx) { builder = builder.resource(desc); }

    // Prompts (built-in + skills loaded from disk).
    for desc in prompts::all(skills) { builder = builder.prompt(desc); }

    builder.build()
}

fn build_instructions(agents_md: &Option<String>, read_only: bool) -> String {
    let mut s = String::new();
    s.push_str(
        "SAP-Automate MCP server (Phase 2 + ADT). Tool groups:\n\
         - RAG search: abap.search, bpmn.find_process, eam.search_apps, sap.help.search, sap.docs.search.\n\
         - RFC + tables: sap.system.info, sap.rfc.search, sap.rfc.metadata, sap.rfc.bulk_metadata, sap.rfc.call, sap.table.read, sap.table.structure.\n\
         - ABAP ADT (read-only): abap.adt.get_program, abap.adt.get_class, abap.adt.get_function_module, abap.adt.get_interface, abap.adt.get_include, abap.adt.get_package_contents, abap.adt.get_cds_view, abap.adt.where_used, abap.adt.search, abap.adt.get_table_contents.\n\
         - ABAP ADT (write): abap.adt.activate (hidden in read-only mode).\n\
         Resources: sap-system://info, sap-rfc://{name}, sap-table://{name}/structure, adt-destination://info, agents://guardrails.\n\
         Prompts: sap.review-rfc-call, sap.transport-impact-analysis, abap.review-where-used.\n",
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

fn dirs_config_path(suffix: &str) -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".config").join(suffix)
    } else {
        std::path::PathBuf::from(suffix)
    }
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
