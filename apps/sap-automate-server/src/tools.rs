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
use sap_automate_adt::{
    AbapObjectKind, AdtCallContext, AdtSearchRequest, ActivationRequest, WhereUsedRequest,
};
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
// ADT tools (Phase 2 finalisation — informed by mcp-abap-adt projects)
// ===========================================================================

pub fn adt_tools(ctx: &Arc<ServerContext>) -> Vec<ToolDescriptor> {
    vec![
        adt_get_program(ctx),
        adt_get_class(ctx),
        adt_get_interface(ctx),
        adt_get_include(ctx),
        adt_get_function_module(ctx),
        adt_get_package_contents(ctx),
        adt_get_cds_view(ctx),
        adt_search(ctx),
        adt_where_used(ctx),
        adt_get_table_contents(ctx),
        adt_activate(ctx), // write — hidden in read-only mode by exposure policy
    ]
}

#[derive(Deserialize)]
struct NameArgs { name: String }

fn name_schema() -> ToolInputSchema {
    ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {"name": {"type": "string", "description": "ABAP object name"}},
        "required": ["name"],
        "additionalProperties": false,
    }))
}

fn adt_get_program(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NameArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_program: {e}"))),
            };
            match ctx.adt_client.get_program(&args.name).await {
                Ok(p) => render_json("abap.adt.get_program", &p),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_program [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_program",
        Some("Retrieve ABAP program source by name via ADT REST. Returns source, line count, active flag.".into()),
        name_schema(), Arc::new(handler))
}

fn adt_get_class(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NameArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_class: {e}"))),
            };
            match ctx.adt_client.get_class(&args.name).await {
                Ok(p) => render_json("abap.adt.get_class", &p),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_class [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_class",
        Some("Retrieve ABAP class source via ADT.".into()),
        name_schema(), Arc::new(handler))
}

fn adt_get_interface(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NameArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_interface: {e}"))),
            };
            match ctx.adt_client.get_interface(&args.name).await {
                Ok(p) => render_json("abap.adt.get_interface", &p),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_interface [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_interface",
        Some("Retrieve ABAP interface source via ADT.".into()),
        name_schema(), Arc::new(handler))
}

fn adt_get_include(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NameArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_include: {e}"))),
            };
            match ctx.adt_client.get_include(&args.name).await {
                Ok(p) => render_json("abap.adt.get_include", &p),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_include [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_include",
        Some("Retrieve ABAP include source via ADT.".into()),
        name_schema(), Arc::new(handler))
}

#[derive(Deserialize)]
struct FmArgs { group: String, name: String }

fn adt_get_function_module(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: FmArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_function_module: {e}"))),
            };
            match ctx.adt_client.get_function_module(&args.group, &args.name).await {
                Ok(p) => render_json("abap.adt.get_function_module", &p),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_function_module [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_function_module",
        Some("Retrieve ABAP function module source. Requires both function group and module name (the ADT URL nests them).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "group": {"type": "string", "description": "Function group, e.g. ZFIN_UTIL"},
                "name": {"type": "string", "description": "Function module name, e.g. Z_FIN_VALIDATE_BUKRS"}
            },
            "required": ["group", "name"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
}

#[derive(Deserialize)]
struct PackageArgs { package: String }

fn adt_get_package_contents(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: PackageArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_package_contents: {e}"))),
            };
            match ctx.adt_client.get_package_contents(&args.package).await {
                Ok(c) => render_json("abap.adt.get_package_contents", &c),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_package_contents [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_package_contents",
        Some("List the objects under an ABAP package (programs, classes, interfaces, CDS views, ...).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {"package": {"type": "string"}},
            "required": ["package"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
}

fn adt_get_cds_view(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NameArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_cds_view: {e}"))),
            };
            match ctx.adt_client.get_cds_view(&args.name).await {
                Ok(v) => render_json("abap.adt.get_cds_view", &v),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_cds_view [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_cds_view",
        Some("Retrieve a Core Data Services (CDS) view source via ADT.".into()),
        name_schema(), Arc::new(handler))
}

#[derive(Deserialize)]
struct AdtSearchArgs {
    query: String,
    #[serde(default)]
    kind: Option<AbapObjectKind>,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

fn default_max_results() -> usize { 25 }

fn adt_search(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: AdtSearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.search: {e}"))),
            };
            let req = AdtSearchRequest { query: args.query, kind: args.kind, max_results: args.max_results };
            match ctx.adt_client.search(req).await {
                Ok(hits) => render_json("abap.adt.search", &serde_json::json!({"hits": hits})),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.search [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.search",
        Some("Live ABAP object search via ADT (different from abap.search which queries the RAG-indexed corpus). Constrained kind enum.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "kind": {"type": "string", "enum": [
                    "program","class","interface","include","function_group","function_module",
                    "table","structure","data_element","domain","package","cds_view",
                    "behavior_definition","service_definition","metadata_extension",
                    "enhancement_spot","transaction"
                ]},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 100, "default": 25}
            },
            "required": ["query"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
}

#[derive(Deserialize)]
struct WhereUsedArgs { name: String, kind: AbapObjectKind }

fn adt_where_used(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: WhereUsedArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.where_used: {e}"))),
            };
            let req = WhereUsedRequest { name: args.name, kind: args.kind };
            match ctx.adt_client.where_used(req).await {
                Ok(hits) => render_json("abap.adt.where_used", &serde_json::json!({"hits": hits})),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.where_used [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.where_used",
        Some("Impact analysis: list places that use a given ABAP object. Returns object name, kind, location, and usage type (implements / call / include / etc.).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": [
                    "program","class","interface","include","function_group","function_module",
                    "table","structure","data_element","domain","package","cds_view"
                ]}
            },
            "required": ["name", "kind"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
}

#[derive(Deserialize)]
struct TableContentsArgs { table: String, #[serde(default = "default_max_rows_100")] max_rows: usize }
fn default_max_rows_100() -> usize { 100 }

fn adt_get_table_contents(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: TableContentsArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.get_table_contents: {e}"))),
            };
            match ctx.adt_client.get_table_contents(&args.table, args.max_rows).await {
                Ok(rows) => render_json("abap.adt.get_table_contents", &serde_json::json!({
                    "rows": rows, "count": rows.len()
                })),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.get_table_contents [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.get_table_contents",
        Some("Table data via the ADT Data Preview API. Some tables are blocked on SAP BTP backends — error code DataPreviewBlocked tells the agent to fall back to sap.table.read (RFC).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "table": {"type": "string"},
                "max_rows": {"type": "integer", "minimum": 1, "maximum": 1000, "default": 100}
            },
            "required": ["table"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
}

#[derive(Deserialize)]
struct ActivateArgs { name: String, kind: AbapObjectKind }

fn adt_activate(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: ActivateArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("abap.adt.activate: {e}"))),
            };
            let req = ActivationRequest { name: args.name, kind: args.kind };
            let call_ctx = AdtCallContext { read_only: ctx.read_only };
            match ctx.adt_client.activate(req, call_ctx).await {
                Ok(outcome) => render_json("abap.adt.activate", &outcome),
                Err(e) => Ok(CallToolResult::error(format!("abap.adt.activate [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new("abap.adt.activate",
        Some("Activate an ABAP object (state-mutating). Hidden in read-only mode by the server exposure policy. When exposed, it still re-checks the per-request read-only flag.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": [
                    "program","class","interface","function_group","function_module","cds_view"
                ]}
            },
            "required": ["name", "kind"],
            "additionalProperties": false,
        })),
        Arc::new(handler))
        .with_writes()
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
