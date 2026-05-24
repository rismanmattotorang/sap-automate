//! SAP-Automate MCP server binary.
//!
//! Exposes a curated catalogue of SAP-Automate tools over stdio.  This is the
//! Phase 1 entry point; later phases swap the `InMemoryKb` for the Qdrant +
//! Postgres + ArangoDB stack from paper §VI.

use mcp_core::{CallToolResult, ToolContent, ToolInputSchema};
use mcp_server::{Server, ToolDescriptor};
use mcp_server::registry::ToolFn;
use mcp_transport::StdioTransport;
use sap_automate_kb::{Domain, InMemoryKb};
use sap_automate_rag::{Query, RagEngine};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

mod seed;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stderr-only logging so it does not corrupt the stdio MCP framing.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let kb = Arc::new(InMemoryKb::new());
    seed::populate(&kb);

    let rag = Arc::new(RagEngine::new(Arc::clone(&kb)));

    let server = build_server(rag);
    server.run(StdioTransport::from_stdio()).await?;
    Ok(())
}

fn build_server(rag: Arc<RagEngine>) -> Server {
    let rag_for_abap = Arc::clone(&rag);
    let rag_for_bpmn = Arc::clone(&rag);
    let rag_for_leanix = Arc::clone(&rag);
    let rag_for_help = Arc::clone(&rag);

    Server::builder("sap-automate-server", env!("CARGO_PKG_VERSION"))
        .instructions(
            "SAP-Automate MCP server. Tools: abap.search, bpmn.find_process, \
             eam.search_apps, sap.help.search. Backed by an in-memory pilot KB \
             until Phase 1A (Qdrant + Postgres) is wired up.",
        )
        .tool(make_search_tool(
            "abap.search",
            "Hybrid search over the ABAP corpus.",
            Domain::Abap,
            rag_for_abap,
        ))
        .tool(make_search_tool(
            "bpmn.find_process",
            "Search the Signavio BPMN process repository.",
            Domain::Bpmn,
            rag_for_bpmn,
        ))
        .tool(make_search_tool(
            "eam.search_apps",
            "Search the LeanIX EAM application fact sheets.",
            Domain::Leanix,
            rag_for_leanix,
        ))
        .tool(make_search_tool(
            "sap.help.search",
            "Search the SAP Help Portal corpus.",
            Domain::SapHelp,
            rag_for_help,
        ))
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
    let value = serde_json::json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Free-text query"},
            "top_k": {"type": "integer", "description": "Top-K results", "minimum": 1, "maximum": 50, "default": 5}
        },
        "required": ["query"]
    });
    ToolInputSchema::from_value(value)
}

fn make_search_tool(
    name: &str,
    description: &str,
    domain: Domain,
    rag: Arc<RagEngine>,
) -> ToolDescriptor {
    let name_owned = name.to_string();
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let rag = Arc::clone(&rag);
        let tool_name = name_owned.clone();
        async move {
            let args: SearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(CallToolResult::error(format!(
                        "{tool_name}: invalid arguments: {e}"
                    )));
                }
            };

            let hits = rag
                .search(Query {
                    text: &args.query,
                    domain: Some(domain),
                    top_k: args.top_k,
                })
                .await;

            if hits.is_empty() {
                return Ok(CallToolResult::text(format!(
                    "{tool_name}: no matches for \"{}\"",
                    args.query
                )));
            }

            let mut lines = vec![format!(
                "{tool_name}: {} hit(s) for \"{}\"",
                hits.len(),
                args.query
            )];
            for hit in &hits {
                lines.push(format!(
                    "- [{:?}] {} ({:.3}) — {}\n  uri: {}",
                    hit.layer,
                    hit.document.title,
                    hit.score,
                    truncate(&hit.document.body, 140),
                    hit.document.uri,
                ));
            }
            Ok(CallToolResult {
                content: vec![ToolContent::text(lines.join("\n"))],
                is_error: false,
            })
        }
    });

    ToolDescriptor::new(
        name,
        Some(description.into()),
        search_schema(),
        Arc::new(handler),
    )
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

// Re-export to silence dead-code on HashMap import used only in seed.rs.
#[allow(dead_code)]
fn _keep_hashmap_used(_: HashMap<String, String>) {}
