//! Tool registrations for the Phase 2 server.
//!
//! Split into two groups:
//!   - `rag_tools` — the four RAG tools introduced in Phase 1 (abap, bpmn,
//!     eam, sap.help).
//!   - `sap_tools` — the eight SAP tools added in Phase 2 (system info,
//!     RFC search/metadata/bulk/call, table read/structure, docs.search).
//!
//! Every tool uses the shared `ServerContext` so they share one
//! `SapClient`, one `RagEngine`, and one `EmbeddingClient`.

use crate::context::ServerContext;
use mcp_core::{CallToolResult, ToolContent, ToolInputSchema};
use mcp_server::{registry::ToolFn, ToolDescriptor};
use sap_automate_kb::Domain;
use sap_automate_rag::Query;
use sap_automate_rfc::{
    ReadTableRequest, RfcCallRequest, MAX_ROWS_HARD_CAP,
};
use serde::Deserialize;
use std::sync::Arc;

// ===========================================================================
// RAG tools (Phase 1) — search the four SAP knowledge domains
// ===========================================================================

pub fn rag_tools(ctx: &Arc<ServerContext>) -> Vec<ToolDescriptor> {
    vec![
        make_rag_tool(ctx, "abap.search", "Hybrid search over the ABAP corpus.", Domain::Abap),
        make_rag_tool(ctx, "bpmn.find_process", "Search the Signavio BPMN process repository.", Domain::Bpmn),
        make_rag_tool(ctx, "eam.search_apps", "Search the LeanIX EAM application fact sheets.", Domain::Leanix),
        make_rag_tool(ctx, "sap.help.search", "Search the SAP Help Portal corpus.", Domain::SapHelp),
    ]
}

#[derive(Debug, Deserialize)]
struct RagSearchArgs {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize { 5 }

fn rag_search_schema() -> ToolInputSchema {
    ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Free-text query"},
            "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 5}
        },
        "required": ["query"],
        "additionalProperties": false
    }))
}

fn make_rag_tool(ctx: &Arc<ServerContext>, name: &str, description: &str, domain: Domain) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let tool_name = name.to_string();
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        let tool_name = tool_name.clone();
        async move {
            let args: RagSearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("{tool_name}: invalid arguments: {e}"))),
            };
            let q_vec = ctx.embedder.embed(&[args.query.clone()]).await.ok().and_then(|mut v| v.pop());
            let hits = ctx.rag.search(Query {
                text: &args.query,
                domain: Some(domain),
                top_k: args.top_k,
                embedding: q_vec,
            }).await;
            match hits {
                Err(e) => Ok(CallToolResult::error(format!("{tool_name}: {e}"))),
                Ok(hits) if hits.is_empty() => {
                    Ok(CallToolResult::text(format!("{tool_name}: no matches for \"{}\"", args.query)))
                }
                Ok(hits) => {
                    let mut lines = vec![format!("{tool_name}: {} hit(s) for \"{}\"", hits.len(), args.query)];
                    for h in &hits {
                        lines.push(format!(
                            "- [{:?}] {} ({:.3}) — {}\n  uri: {}",
                            h.layer, h.hit.chunk.title, h.hit.score,
                            truncate(&h.hit.chunk.text, 160),
                            h.hit.chunk.uri,
                        ));
                    }
                    Ok(CallToolResult { content: vec![ToolContent::text(lines.join("\n"))], is_error: false })
                }
            }
        }
    });
    ToolDescriptor::new(name, Some(description.into()), rag_search_schema(), Arc::new(handler))
}

// ===========================================================================
// SAP tools (Phase 2)
// ===========================================================================

pub fn sap_tools(ctx: &Arc<ServerContext>) -> Vec<ToolDescriptor> {
    vec![
        tool_system_info(ctx),
        tool_rfc_search(ctx),
        tool_rfc_metadata(ctx),
        tool_rfc_bulk_metadata(ctx),
        tool_rfc_call(ctx),
        tool_table_read(ctx),
        tool_table_structure(ctx),
        tool_docs_search(ctx),
    ]
}

// --- sap.system.info -------------------------------------------------------

fn tool_system_info(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |_args: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            match ctx.sap_client.system_info().await {
                Ok(info) => render_json("sap.system.info", &info),
                Err(e) => Ok(CallToolResult::error(format!("sap.system.info: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.system.info",
        Some("Retrieve SAP system identity (SID, client, release, host). Always read-only.".into()),
        ToolInputSchema::from_value(serde_json::json!({"type": "object", "additionalProperties": false})),
        Arc::new(handler),
    )
}

// --- sap.rfc.search --------------------------------------------------------

#[derive(Deserialize)]
struct RfcSearchArgs { query: String, #[serde(default = "default_limit_20")] limit: usize }
fn default_limit_20() -> usize { 20 }

fn tool_rfc_search(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: RfcSearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.rfc.search: invalid arguments: {e}"))),
            };
            match ctx.sap_client.search_rfc(&args.query, args.limit).await {
                Ok(result) => render_json("sap.rfc.search", &result),
                Err(e) => Ok(CallToolResult::error(format!("sap.rfc.search: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.rfc.search",
        Some("Search the RFC catalogue by keyword. Returns ranked function names with descriptions and read-only flag.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Keyword query"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20}
            },
            "required": ["query"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.rfc.metadata ------------------------------------------------------

#[derive(Deserialize)]
struct RfcMetaArgs { function: String, #[serde(default = "default_lang")] language: String }
fn default_lang() -> String { "EN".into() }

fn tool_rfc_metadata(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: RfcMetaArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.rfc.metadata: invalid arguments: {e}"))),
            };
            match ctx.sap_client.rfc_metadata(&args.function, &args.language).await {
                Ok(meta) => render_json("sap.rfc.metadata", &meta),
                Err(e) => Ok(CallToolResult::error(format!("sap.rfc.metadata: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.rfc.metadata",
        Some("Get full parameter signature, function group, and read-only flag for an RFC function.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "function": {"type": "string", "description": "RFC function name (e.g. BAPI_MATERIAL_GET_DETAIL)"},
                "language": {"type": "string", "default": "EN", "minLength": 1, "maxLength": 2}
            },
            "required": ["function"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.rfc.bulk_metadata -------------------------------------------------

#[derive(Deserialize)]
struct RfcBulkArgs { functions: Vec<String>, #[serde(default = "default_lang")] language: String }

fn tool_rfc_bulk_metadata(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: RfcBulkArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.rfc.bulk_metadata: invalid arguments: {e}"))),
            };
            if args.functions.is_empty() || args.functions.len() > 100 {
                return Ok(CallToolResult::error(
                    "sap.rfc.bulk_metadata: provide 1..=100 function names",
                ));
            }
            match ctx.sap_client.bulk_rfc_metadata(&args.functions, &args.language).await {
                Ok(out) => render_json("sap.rfc.bulk_metadata", &out),
                Err(e) => Ok(CallToolResult::error(format!("sap.rfc.bulk_metadata: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.rfc.bulk_metadata",
        Some("Batch-fetch metadata for up to 100 RFCs in one call. Mirrors `bulk_load_metadata` from the Python reference project, avoiding N round-trips.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "functions": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 100},
                "language": {"type": "string", "default": "EN"}
            },
            "required": ["functions"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.rfc.call ----------------------------------------------------------

fn tool_rfc_call(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let request: RfcCallRequest = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.rfc.call: invalid arguments: {e}"))),
            };
            match ctx.sap_client.call_rfc(request, ctx.read_only).await {
                Ok(result) => render_json("sap.rfc.call", &result),
                Err(e) => Ok(CallToolResult::error(format!("sap.rfc.call [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.rfc.call",
        Some("Execute an RFC function by name with a parameters object. Read-only mode (default) blocks any RFC not declared safe. Errors carry structured codes (RFC_TIMEOUT, RFC_AUTH_FAILED, etc.).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "function": {"type": "string", "description": "RFC function name"},
                "parameters": {"type": "object", "description": "Function parameter object"},
                "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 600000, "default": 30000},
                "require_read_only_safe": {"type": "boolean", "default": true}
            },
            "required": ["function"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.table.read --------------------------------------------------------

fn tool_table_read(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let request: ReadTableRequest = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.table.read: invalid arguments: {e}"))),
            };
            match ctx.sap_client.read_table(request).await {
                Ok(rows) => render_json("sap.table.read", &serde_json::json!({"rows": rows, "count": rows.len()})),
                Err(e) => Ok(CallToolResult::error(format!("sap.table.read [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.table.read",
        Some("Read rows from a SAP table with optional field projection and SQL-like WHERE clauses. Hard-capped at 1000 rows to avoid buffer overflow.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "table": {"type": "string", "description": "Table name (e.g. MARA, T001, VBAK)"},
                "fields": {"type": "array", "items": {"type": "string"}, "description": "Field projection; empty = all fields"},
                "where_conditions": {"type": "array", "items": {"type": "string"}, "description": "WHERE clauses, e.g. \"WAERS = 'EUR'\""},
                "max_rows": {"type": "integer", "minimum": 1, "maximum": MAX_ROWS_HARD_CAP, "default": 100}
            },
            "required": ["table"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.table.structure ---------------------------------------------------

#[derive(Deserialize)]
struct TableStructArgs { table: String }

fn tool_table_structure(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: TableStructArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.table.structure: invalid arguments: {e}"))),
            };
            match ctx.sap_client.table_structure(&args.table).await {
                Ok(s) => render_json("sap.table.structure", &s),
                Err(e) => Ok(CallToolResult::error(format!("sap.table.structure: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.table.structure",
        Some("Get DDIC field metadata for an SAP table or structure (name, type, length, key flag).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {"table": {"type": "string"}},
            "required": ["table"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.docs.search -------------------------------------------------------

#[derive(Deserialize)]
struct DocsSearchArgs {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    domain: Option<String>,
}

fn tool_docs_search(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: DocsSearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.docs.search: invalid arguments: {e}"))),
            };
            let domain = match args.domain.as_deref() {
                None | Some("all") => None,
                Some("sap_help") => Some(Domain::SapHelp),
                Some("abap") => Some(Domain::Abap),
                Some("bpmn") => Some(Domain::Bpmn),
                Some("leanix") => Some(Domain::Leanix),
                Some(other) => return Ok(CallToolResult::error(format!("sap.docs.search: unknown domain '{other}'"))),
            };
            let q_vec = ctx.embedder.embed(&[args.query.clone()]).await.ok().and_then(|mut v| v.pop());
            match ctx.rag.search(Query { text: &args.query, domain, top_k: args.top_k, embedding: q_vec }).await {
                Ok(hits) => {
                    let body: Vec<_> = hits.iter().map(|h| {
                        serde_json::json!({
                            "uri": h.hit.chunk.uri,
                            "title": h.hit.chunk.title,
                            "score": h.hit.score,
                            "snippet": truncate(&h.hit.chunk.text, 200),
                        })
                    }).collect();
                    render_json("sap.docs.search", &serde_json::json!({"hits": body}))
                }
                Err(e) => Ok(CallToolResult::error(format!("sap.docs.search: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.docs.search",
        Some("Semantic search across the unified SAP knowledge base (Help Portal + ABAP + BPMN + LeanIX). Use this instead of guessing which domain holds the answer.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 5},
                "domain": {"type": "string", "enum": ["all", "sap_help", "abap", "bpmn", "leanix"], "default": "all"}
            },
            "required": ["query"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// ===========================================================================
// helpers
// ===========================================================================

fn render_json<T: serde::Serialize>(tool: &str, value: &T) -> mcp_core::Result<CallToolResult> {
    match serde_json::to_string_pretty(value) {
        Ok(s) => Ok(CallToolResult { content: vec![ToolContent::text(s)], is_error: false }),
        Err(e) => Ok(CallToolResult::error(format!("{tool}: serialise: {e}"))),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() }
    else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
