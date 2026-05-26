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

use sap_automate_server_lib::{context, prompts, resources, seed, tools};

use clap::Parser;
use mcp_server::Server;
use mcp_transport::{HttpServerConfig, HttpServerTransport, StdioTransport};
use sap_automate_observability::metrics::{MetricKind, MetricsRegistry};
use sap_automate_adt::{AdtClient, AdtDestination, MockAdtClient};
use sap_automate_graph::InMemoryGraph;
use sap_automate_rag::GraphEngine;
use sap_automate_skills::SkillRegistry;
use sap_automate_ingest::{EmbeddingClient, MockEmbedder};
use sap_automate_kb::{InMemoryKb, KnowledgeStore};
use sap_automate_rag::RagEngine;
use sap_automate_rfc::{
    Credentials, CredentialProvider, CredentialSource, EnvCredentialProvider,
    LayeredCredentialProvider, MetadataCache, MockSapClient, SapClient,
    StaticCredentialProvider,
};
use std::time::Duration;
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

    /// Transport: "stdio" (default) or "http".  HTTP mode binds to
    /// --bind and accepts JSON-RPC POSTs at /mcp plus SSE at /mcp/events
    /// (paper §IV-C; convergent pattern from fr0ster/mcp-abap-adt).
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// HTTP listener bind address (used when --transport=http).
    #[arg(long, default_value = "127.0.0.1:3030")]
    bind: String,

    /// Optional bearer token required for HTTP requests.
    #[arg(long)]
    bearer_token: Option<String>,

    /// Allowed `Origin` header values for HTTP transport (MCP 2025-06-18
    /// §4.6 — DNS-rebinding mitigation).  Repeatable.  Example:
    /// `--allowed-origin http://localhost:3000`.  When empty, the origin
    /// check is disabled — only safe for stdio or trusted in-cluster
    /// traffic.
    #[arg(long = "allowed-origin", num_args = 1)]
    allowed_origins: Vec<String>,

    /// TTL in seconds for the RFC metadata cache (thupalo/sap-rfc-mcp-server
    /// pattern).  `0` disables caching — every `sap.rfc.metadata` and
    /// `sap.rfc.bulk_metadata` call falls through to the backend.
    /// Default 300 (5 minutes) matches the inter-transport-import horizon
    /// most SAP basis teams maintain.
    #[arg(long, default_value_t = 300)]
    metadata_cache_ttl_secs: u64,
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

    // Phase 5A: cross-domain knowledge graph + GraphRAG/HippoRAG/RAPTOR.
    let kg = Arc::new(InMemoryGraph::with_demo_corpus());
    let graph_engine = Arc::new(GraphEngine::new(kg));
    tracing::info!(
        nodes = graph_engine.graph.stats().node_count,
        edges = graph_engine.graph.stats().edge_count,
        communities = graph_engine.communities.communities.len(),
        "graph engine ready"
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

    let inner_sap_client = MockSapClient::new(cli.pool_size, creds.redacted());

    // Decorate with a TTL metadata cache when configured.  When TTL=0 we
    // still build the cache (with TTL::ZERO so it acts as a pass-through
    // counter) so the cache-stats tool always has a sink — operators get
    // miss-count visibility even when caching is "off".
    let cache_ttl = Duration::from_secs(cli.metadata_cache_ttl_secs);
    let metadata_cache = MetadataCache::new(inner_sap_client.clone(), cache_ttl);
    let sap_client: Arc<dyn SapClient> = metadata_cache.clone();
    let metadata_cache_handle = Some(metadata_cache);
    tracing::info!(
        cache_ttl_secs = cli.metadata_cache_ttl_secs,
        "RFC metadata cache active (thupalo pattern)"
    );

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
        graph: graph_engine,
        embedder,
        sap_client,
        metadata_cache: metadata_cache_handle,
        adt_client,
        read_only,
        agents_md: agents_md.clone(),
    });

    let server = build_server(ctx.clone(), &agents_md, read_only, &skills);
    tracing::info!(
        read_only = read_only,
        pool_size = cli.pool_size,
        embedding_dim = cli.embedding_dim,
        transport = %cli.transport,
        "SAP-Automate server configured"
    );

    match cli.transport.as_str() {
        "stdio" => {
            // Split into independent read/write halves so a tool that
            // calls elicit().await doesn't block the reader.
            let (reader, writer) = StdioTransport::new(
                tokio::io::stdin(), tokio::io::stdout(),
            ).into_parts();
            server.run_stdio(reader, writer).await?
        }
        "http" => {
            let bind: std::net::SocketAddr = cli.bind.parse()
                .map_err(|e| anyhow::anyhow!("invalid --bind '{}': {e}", cli.bind))?;
            tracing::info!(bind = %bind, "HTTP transport binding");

            // Build the Prometheus metrics registry.  Names follow paper
            // §IV-H (mcp_tool_latency_seconds, rag_retrieval_latency_seconds,
            // sap_rfc_calls_total, sap_authz_denied_total).
            let metrics = Arc::new(MetricsRegistry::new());
            metrics.register("mcp_tool_latency_seconds", MetricKind::Histogram,
                "Per-tool call latency in seconds (paper §X-D gate at 0.080s)");
            metrics.register("mcp_tool_calls_total", MetricKind::Counter,
                "Total MCP tool invocations");
            metrics.register("mcp_tool_errors_total", MetricKind::Counter,
                "Total MCP tool invocations that returned isError=true");
            metrics.register("rag_retrieval_latency_seconds", MetricKind::Histogram,
                "RAG retrieval latency (paper §X-D gate at 0.080s)");
            metrics.register("kb_chunks_total", MetricKind::Gauge,
                "Total chunks currently indexed");
            metrics.register("sap_pool_in_use", MetricKind::Gauge,
                "SAP connection pool slots currently in use");
            metrics.register("sap_authz_denied_total", MetricKind::Counter,
                "Calls denied by the read-only safety gate");
            metrics.register("sap_rfc_calls_total", MetricKind::Counter,
                "RFC calls dispatched to SAP, grouped by function and outcome");

            let metrics_for_render = Arc::clone(&metrics);
            let render: mcp_transport::http::MetricsRenderFn = Arc::new(move || {
                metrics_for_render.render()
            });

            let dispatch_server = server.clone();
            let handle = HttpServerTransport::serve(
                HttpServerConfig {
                    bind,
                    bearer_token: cli.bearer_token.clone(),
                    metrics_renderer: Some(render),
                    allowed_origins: cli.allowed_origins.clone(),
                },
                move |msg| {
                    let server = dispatch_server.clone();
                    async move { server.dispatch_message(msg).await }
                },
            ).await?;
            tracing::info!(
                "HTTP server ready at http://{bind}/mcp  (events: /mcp/events, metrics: /metrics)"
            );
            // Run until SIGINT
            tokio::signal::ctrl_c().await?;
            handle.shutdown().await;
        }
        other => {
            anyhow::bail!("unknown --transport '{other}' (expected: stdio | http)");
        }
    }
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

    // Graph tools (Phase 5A — GraphRAG + HippoRAG + RAPTOR).
    for desc in tools::graph_tools(&ctx) { builder = builder.tool(desc); }

    // Workflow tools (Phase 6 — MCP 2025-06-18 elicitation).
    for desc in tools::workflow_tools(&ctx) { builder = builder.tool(desc); }

    // Resources.
    for desc in resources::all(&ctx) { builder = builder.resource(desc); }

    // Prompts (built-in + skills loaded from disk).
    for desc in prompts::all(skills) { builder = builder.prompt(desc); }

    // MCP 2025-06-18 completion utility — autocomplete for skill arguments.
    builder = sap_automate_server_lib::register_completers(builder);

    builder.build()
}

fn build_instructions(agents_md: &Option<String>, read_only: bool) -> String {
    let mut s = String::new();
    s.push_str(
        "SAP-Automate MCP server (Phase 2 + ADT). Tool groups:\n\
         - RAG search: abap.search, bpmn.find_process, eam.search_apps, sap.help.search, sap.docs.search.\n\
         - RFC + tables: sap.system.info, sap.system.health, sap.system.cache_stats, sap.system.cache_invalidate, sap.rfc.search, sap.rfc.metadata, sap.rfc.bulk_metadata, sap.rfc.call, sap.table.read, sap.table.structure.\n\
         - ABAP ADT (read-only): abap.adt.get_program, abap.adt.get_class, abap.adt.get_function_module, abap.adt.get_interface, abap.adt.get_include, abap.adt.get_package_contents, abap.adt.get_cds_view, abap.adt.where_used, abap.adt.search, abap.adt.get_table_contents.\n\
         - ABAP ADT (write): abap.adt.activate (hidden in read-only mode).\n\
         Resources: sap-system://info, sap-rfc://{name}, sap-table://{name}/structure, adt-destination://info, sap-cache://stats, agents://guardrails.\n\
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
