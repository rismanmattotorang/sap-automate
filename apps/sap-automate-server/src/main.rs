//! SAP-Automate MCP server binary.
//!
//! Phase 1A: the server now sits behind the `KnowledgeStore` trait and an
//! `EmbeddingClient` so it can run against either:
//!   - default: `InMemoryKb` + `MockEmbedder` seeded with the demo corpus
//!     (works offline, used by the Phase 1 demo and CI),
//!   - production: `QdrantStore` + `OpenAiEmbedder` populated by the
//!     `sap-automate-ingest` binary.
//!
//! The same MCP tool surface (abap.search, bpmn.find_process, eam.search_apps,
//! sap.help.search) works against both.

use mcp_core::{CallToolResult, ToolContent, ToolInputSchema};
use mcp_server::{Server, ToolDescriptor};
use mcp_server::registry::ToolFn;
use mcp_transport::StdioTransport;
use sap_automate_ingest::{EmbeddingClient, MockEmbedder};
use sap_automate_kb::{Domain, InMemoryKb, KnowledgeStore};
use sap_automate_rag::{Query, RagEngine};
use serde::Deserialize;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

mod seed;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let store: Arc<dyn KnowledgeStore> = Arc::new(InMemoryKb::new());
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder::new(256));
    seed::populate_with_embeddings(&store, embedder.as_ref()).await?;

    let rag = Arc::new(RagEngine::new(store.clone()));

    let server = build_server(rag, embedder);
    server.run(StdioTransport::from_stdio()).await?;
    Ok(())
}

fn build_server(rag: Arc<RagEngine>, embedder: Arc<dyn EmbeddingClient>) -> Server {
    Server::builder("sap-automate-server", env!("CARGO_PKG_VERSION"))
        .instructions(
            "SAP-Automate MCP server. Tools: abap.search, bpmn.find_process, \
             eam.search_apps, sap.help.search. Backed by the configured \
             KnowledgeStore (default: in-memory; configurable to Qdrant).",
        )
        .tool(make_search_tool("abap.search", "Hybrid search over the ABAP corpus.", Domain::Abap, Arc::clone(&rag), Arc::clone(&embedder)))
        .tool(make_search_tool("bpmn.find_process", "Search the Signavio BPMN process repository.", Domain::Bpmn, Arc::clone(&rag), Arc::clone(&embedder)))
        .tool(make_search_tool("eam.search_apps", "Search the LeanIX EAM application fact sheets.", Domain::Leanix, Arc::clone(&rag), Arc::clone(&embedder)))
        .tool(make_search_tool("sap.help.search", "Search the SAP Help Portal corpus.", Domain::SapHelp, Arc::clone(&rag), Arc::clone(&embedder)))
        .build()
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize { 5 }

fn search_schema() -> ToolInputSchema {
    ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Free-text query"},
            "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 5}
        },
        "required": ["query"]
    }))
}

fn make_search_tool(
    name: &str,
    description: &str,
    domain: Domain,
    rag: Arc<RagEngine>,
    embedder: Arc<dyn EmbeddingClient>,
) -> ToolDescriptor {
    let tool_name = name.to_string();
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let rag = Arc::clone(&rag);
        let embedder = Arc::clone(&embedder);
        let tool_name = tool_name.clone();
        async move {
            let args: SearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("{tool_name}: invalid arguments: {e}"))),
            };

            // Embed the query so vector backends (Qdrant) get the right signal.
            let q_vec = match embedder.embed(&[args.query.clone()]).await {
                Ok(mut v) => v.pop(),
                Err(e) => {
                    tracing::warn!(error = %e, "embedder unavailable; falling back to lexical");
                    None
                }
            };

            let hits = match rag.search(Query {
                text: &args.query,
                domain: Some(domain),
                top_k: args.top_k,
                embedding: q_vec,
            }).await {
                Ok(h) => h,
                Err(e) => return Ok(CallToolResult::error(format!("{tool_name}: store error: {e}"))),
            };

            if hits.is_empty() {
                return Ok(CallToolResult::text(format!("{tool_name}: no matches for \"{}\"", args.query)));
            }

            let mut lines = vec![format!("{tool_name}: {} hit(s) for \"{}\"", hits.len(), args.query)];
            for h in &hits {
                lines.push(format!(
                    "- [{:?}] {} ({:.3}) — {}\n  uri: {}",
                    h.layer,
                    h.hit.chunk.title,
                    h.hit.score,
                    truncate(&h.hit.chunk.text, 160),
                    h.hit.chunk.uri,
                ));
            }
            Ok(CallToolResult { content: vec![ToolContent::text(lines.join("\n"))], is_error: false })
        }
    });

    ToolDescriptor::new(name, Some(description.into()), search_schema(), Arc::new(handler))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
