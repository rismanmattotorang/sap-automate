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
    execute_write_bapi, ReadTableRequest, RfcCallRequest, MAX_ROWS_HARD_CAP,
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
        tool_kb_navigate(ctx),
    ]
}

// --- sap.kb.navigate ------------------------------------------------------
// OpenKB + PageIndex convergent pattern.  Walks the hierarchical document
// tree built by `sap_automate_kb::build_document_tree`.  When the agent has
// a long SAP Help page (period close, transport release, RAP service
// design), section-by-section navigation beats similarity-blind retrieval.

#[derive(Debug, Deserialize)]
struct KbNavigateArgs {
    document_id: String,
    /// Optional dotted path into the tree (e.g. `"1.2"`).  Defaults to the
    /// root, which lists the top-level sections.
    #[serde(default)]
    path: Option<String>,
    /// How many levels of descendants to include.  Defaults to 1 (just the
    /// immediate children) so a single call is bounded.
    #[serde(default = "default_navigate_depth")]
    depth: u32,
}
fn default_navigate_depth() -> u32 { 1 }

fn tool_kb_navigate(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: KbNavigateArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.kb.navigate: invalid arguments: {e}"))),
            };
            // Use the KnowledgeStore's default tree builder via the RAG engine's store.
            let store = ctx.rag.store();
            let tree = match store.get_document_tree(&args.document_id).await {
                Ok(Some(t)) => t,
                Ok(None) => return Ok(CallToolResult::error(format!("sap.kb.navigate: document '{}' not found", args.document_id))),
                Err(e) => return Ok(CallToolResult::error(format!("sap.kb.navigate: {e}"))),
            };
            let path = args.path.as_deref().unwrap_or("");
            let node = if path.is_empty() {
                &tree.root
            } else {
                match tree.root.find(path) {
                    Some(n) => n,
                    None => return Ok(CallToolResult::error(format!(
                        "sap.kb.navigate: path '{path}' not found in document tree (max_depth={}, leaf_count={})",
                        tree.max_depth, tree.leaf_count,
                    ))),
                }
            };
            let view = serialize_node_bounded(node, args.depth);
            render_json("sap.kb.navigate", &serde_json::json!({
                "document_id": tree.document_id,
                "max_depth": tree.max_depth,
                "leaf_count": tree.leaf_count,
                "node": view,
            }))
        }
    });
    ToolDescriptor::new(
        "sap.kb.navigate",
        Some("Walk the hierarchical document tree (OpenKB + PageIndex pattern) section by section. Pass a document_id and an optional dotted path (e.g. '1.2.1') and depth to bound the returned subtree. Use this for long SAP Help pages / ABAP source files when similarity-blind retrieval would miss the right section.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "document_id": {"type": "string", "description": "Document id, e.g. 'sap_help:FI/period-close'"},
                "path": {"type": "string", "description": "Optional dotted path, e.g. '1.2'. Omit to start at the root."},
                "depth": {"type": "integer", "minimum": 0, "maximum": 4, "default": 1}
            },
            "required": ["document_id"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

fn serialize_node_bounded(node: &sap_automate_kb::DocTreeNode, depth: u32) -> serde_json::Value {
    let children: Vec<serde_json::Value> = if depth == 0 {
        node.children.iter().map(|c| serde_json::json!({
            "path": c.path,
            "title": c.title,
            "summary": c.summary,
            "approx_tokens": c.approx_tokens,
            "child_count": c.children.len(),
        })).collect()
    } else {
        node.children.iter().map(|c| serialize_node_bounded(c, depth - 1)).collect()
    };
    serde_json::json!({
        "path": node.path,
        "depth": node.depth,
        "title": node.title,
        "summary": node.summary,
        "start_index": node.start_index,
        "end_index": node.end_index,
        "approx_tokens": node.approx_tokens,
        "children": children,
    })
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
        tool_system_health(ctx),
        tool_system_cache_stats(ctx),
        tool_system_cache_invalidate(ctx),
        tool_rfc_search(ctx),
        tool_rfc_metadata(ctx),
        tool_rfc_bulk_metadata(ctx),
        tool_rfc_call(ctx),
        tool_table_read(ctx),
        tool_table_structure(ctx),
        tool_docs_search(ctx),
        tool_bapi_parse_return(ctx),
        tool_bp_search(ctx),
        tool_bp_get(ctx),
    ]
}

// --- sap.bp.search / sap.bp.get -------------------------------------------
// Live SAP backend tier: SAP Business Accelerator Hub sandbox.
// Convergent with the OData generic-proxy design discipline shipped as
// `sap.skill.odata_service_design`.

#[derive(Deserialize)]
struct BpSearchArgs {
    query: String,
    #[serde(default = "default_bp_limit")]
    limit: usize,
}
fn default_bp_limit() -> usize { 10 }

fn tool_bp_search(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: BpSearchArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.bp.search: invalid arguments: {e}"))),
            };
            let hub = match &ctx.business_hub {
                Some(h) => h,
                None => return Ok(CallToolResult::error(
                    "sap.bp.search: SAP Business Accelerator Hub disabled. \
                     Set SAP_BUSINESS_HUB_KEY (free key from a SAP Community account) and restart the server.".to_string()
                )),
            };
            match hub.search_business_partners(&args.query, args.limit.clamp(1, 100)).await {
                Ok(rows) => render_json("sap.bp.search", &serde_json::json!({
                    "query": args.query,
                    "count": rows.len(),
                    "results": rows,
                })),
                Err(e) => Ok(CallToolResult::error(format!("sap.bp.search: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.bp.search",
        Some("Search SAP A_BusinessPartner (OData v4) on the SAP Business Accelerator Hub sandbox. \
              Returns matching Business Partner rows with id, full name, category, organization name, \
              and BP creation date. Requires SAP_BUSINESS_HUB_KEY env var (free SAP Community login).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Substring match against BusinessPartnerFullName."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10}
            },
            "required": ["query"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

#[derive(Deserialize)]
struct BpGetArgs {
    id: String,
}

fn tool_bp_get(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: BpGetArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.bp.get: invalid arguments: {e}"))),
            };
            let hub = match &ctx.business_hub {
                Some(h) => h,
                None => return Ok(CallToolResult::error(
                    "sap.bp.get: SAP Business Accelerator Hub disabled. Set SAP_BUSINESS_HUB_KEY.".to_string()
                )),
            };
            match hub.get_business_partner(&args.id).await {
                Ok(bp) => render_json("sap.bp.get", &bp),
                Err(e) => Ok(CallToolResult::error(format!("sap.bp.get: {e}"))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.bp.get",
        Some("Fetch a single SAP A_BusinessPartner by id from the SAP Business Accelerator Hub sandbox (OData v4). \
              Requires SAP_BUSINESS_HUB_KEY.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Business Partner identifier (e.g. '1003764')."}
            },
            "required": ["id"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// --- sap.system.cache_stats ------------------------------------------------
// Convergent with thupalo/sap-rfc-mcp-server `get_metadata_cache_stats`.
// Returns hits/misses/entries/evictions/hit_ratio for the RFC metadata cache.

fn tool_system_cache_stats(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |_args: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            match &ctx.metadata_cache {
                None => render_json("sap.system.cache_stats", &serde_json::json!({
                    "enabled": false,
                    "note": "RFC metadata cache is disabled. Restart the server with --metadata-cache-ttl-secs > 0.",
                })),
                Some(cache) => {
                    let s = cache.stats().await;
                    render_json("sap.system.cache_stats", &serde_json::json!({
                        "enabled": true,
                        "hits": s.hits,
                        "misses": s.misses,
                        "entries": s.entries,
                        "evictions": s.evictions,
                        "hit_ratio": s.hit_ratio(),
                    }))
                }
            }
        }
    });
    ToolDescriptor::new(
        "sap.system.cache_stats",
        Some("Read RFC metadata cache statistics (thupalo/sap-rfc-mcp-server pattern). Returns hits, misses, entries, evictions, and hit_ratio. Always read-only — touches local cache state only, never SAP.".into()),
        ToolInputSchema::from_value(serde_json::json!({"type": "object", "additionalProperties": false})),
        Arc::new(handler),
    )
}

// --- sap.system.cache_invalidate -------------------------------------------
// Operator escape hatch for the case where an upstream transport import has
// changed an RFC signature and the cached metadata is now stale.  Read-only
// from the SAP-state perspective (we never write to SAP), but it does mutate
// the local cache, so the description is explicit.

fn tool_system_cache_invalidate(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |_args: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            match &ctx.metadata_cache {
                None => render_json("sap.system.cache_invalidate", &serde_json::json!({
                    "enabled": false,
                    "note": "RFC metadata cache is disabled. Nothing to invalidate.",
                })),
                Some(cache) => {
                    let before = cache.stats().await.entries;
                    cache.invalidate_all().await;
                    render_json("sap.system.cache_invalidate", &serde_json::json!({
                        "ok": true,
                        "entries_dropped": before,
                    }))
                }
            }
        }
    });
    ToolDescriptor::new(
        "sap.system.cache_invalidate",
        Some("Drop every entry in the RFC metadata cache so the next sap.rfc.metadata / bulk_metadata call re-fetches from SAP. Use after a transport import that changed RFC signatures. Does not touch SAP state.".into()),
        ToolInputSchema::from_value(serde_json::json!({"type": "object", "additionalProperties": false})),
        Arc::new(handler),
    )
}

// --- sap.system.health -----------------------------------------------------

fn tool_system_health(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |_args: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let pool = ctx.sap_client.pool_status();
            let snap = serde_json::json!({
                "pool": { "cap": pool.cap, "available": pool.available, "in_use": pool.cap - pool.available },
                "read_only_mode": ctx.read_only,
                "adt_destination": ctx.adt_client.destination().redacted(),
                "graph": {
                    "nodes": ctx.graph.graph.stats().node_count,
                    "edges": ctx.graph.graph.stats().edge_count,
                    "communities": ctx.graph.communities.communities.len(),
                },
                "protocol_version": mcp_core::PROTOCOL_VERSION,
            });
            render_json("sap.system.health", &snap)
        }
    });
    ToolDescriptor::new(
        "sap.system.health",
        Some("Operator health snapshot: connection pool, read-only mode, ADT destination summary, graph stats. Always read-only.".into()),
        ToolInputSchema::from_value(serde_json::json!({"type": "object", "additionalProperties": false})),
        Arc::new(handler),
    )
}

// --- sap.bapi.parse_return -------------------------------------------------

#[derive(Deserialize)]
struct BapiParseArgs { value: serde_json::Value }

fn tool_bapi_parse_return(_ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let handler = ToolFn(move |arguments: serde_json::Value| {
        async move {
            let args: BapiParseArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.bapi.parse_return: invalid arguments: {e}"))),
            };
            let msgs = sap_automate_rfc::parse_bapiret2(&args.value);
            let any_failure = msgs.iter().any(|m| m.is_failure());
            let summary = serde_json::json!({
                "messages": msgs,
                "any_failure": any_failure,
                "guidance": if any_failure {
                    "At least one message has severity E (error) or A (abort). DO NOT call BAPI_TRANSACTION_COMMIT. Call BAPI_TRANSACTION_ROLLBACK if you've already started writes."
                } else if msgs.is_empty() {
                    "No BAPIRET2 messages found in the supplied value. Either pass the full BAPI result or rerun the BAPI to capture the RETURN table."
                } else {
                    "All BAPIRET2 messages are non-failure (S/W/I). Safe to call BAPI_TRANSACTION_COMMIT."
                }
            });
            render_json("sap.bapi.parse_return", &summary)
        }
    });
    ToolDescriptor::new(
        "sap.bapi.parse_return",
        Some("Parse a BAPIRET2 array (the standard SAP return contract) and surface structured messages. Returns severity, message class, number, text, and a guidance string that tells the agent whether it's safe to commit.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "description": "The full JSON result from a sap.rfc.call invocation, or a bare BAPIRET2 array.",
                }
            },
            "required": ["value"],
            "additionalProperties": false,
        })),
        Arc::new(handler),
    )
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
            // `commit` is read off the raw args (RfcCallRequest ignores it).
            let commit = arguments.get("commit").and_then(|v| v.as_bool()).unwrap_or(false);
            let request: RfcCallRequest = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.rfc.call: invalid arguments: {e}"))),
            };
            if commit {
                // Transactional write: call the BAPI, then auto
                // commit-or-rollback based on its BAPIRET2 (gated by
                // --enable-writes).
                return match execute_write_bapi(ctx.sap_client.as_ref(), request, ctx.read_only).await {
                    Ok(outcome) => {
                        // Audit trail — function + outcome only, never params.
                        tracing::info!(
                            target: "sap_audit",
                            function = %outcome.function,
                            committed = outcome.committed,
                            rolled_back = outcome.rolled_back,
                            messages = outcome.messages.len(),
                            "transactional write executed"
                        );
                        render_json("sap.rfc.call", &serde_json::json!({
                            "function": outcome.function,
                            "committed": outcome.committed,
                            "rolled_back": outcome.rolled_back,
                            "messages": outcome.messages,
                            "result": outcome.result,
                        }))
                    }
                    Err(e) => Ok(CallToolResult::error(format!("sap.rfc.call [{:?}]: {e}", e.code()))),
                };
            }
            match ctx.sap_client.call_rfc(request, ctx.read_only).await {
                Ok(result) => render_json("sap.rfc.call", &result),
                Err(e) => Ok(CallToolResult::error(format!("sap.rfc.call [{:?}]: {e}", e.code()))),
            }
        }
    });
    ToolDescriptor::new(
        "sap.rfc.call",
        Some("Execute an RFC function by name with a parameters object. Read-only mode (default) blocks any RFC not declared safe. Set commit=true to run it as a transactional write: the BAPI is followed by BAPI_TRANSACTION_COMMIT on success or BAPI_TRANSACTION_ROLLBACK on a BAPIRET2 error (requires --enable-writes). Errors carry structured codes (RFC_TIMEOUT, RFC_AUTH_FAILED, etc.).".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "function": {"type": "string", "description": "RFC function name"},
                "parameters": {"type": "object", "description": "Function parameter object"},
                "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 600000, "default": 30000},
                "require_read_only_safe": {"type": "boolean", "default": true},
                "commit": {"type": "boolean", "default": false, "description": "Run as a transactional write with automatic commit/rollback (requires --enable-writes)."}
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
// Graph tools (Phase 5A — GraphRAG L3, HippoRAG L4, RAPTOR L5)
// ===========================================================================

pub fn graph_tools(ctx: &Arc<ServerContext>) -> Vec<ToolDescriptor> {
    vec![
        kb_multi_hop(ctx),
        kb_global_query(ctx),
        kb_summarise(ctx),
        kb_graph_neighborhood(ctx),
    ]
}

#[derive(Deserialize)]
struct MultiHopArgs {
    query: String,
    #[serde(default = "default_max_hops")]
    max_hops: u32,
    #[serde(default = "default_top_k_graph")]
    top_k: usize,
    #[serde(default = "default_max_seeds")]
    max_seeds: usize,
}
fn default_max_hops() -> u32 { 4 }
fn default_top_k_graph() -> usize { 8 }
fn default_max_seeds() -> usize { 3 }

fn kb_multi_hop(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: MultiHopArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("kb.multi_hop: invalid arguments: {e}"))),
            };
            let response = ctx.graph.multi_hop(&args.query, args.max_hops, args.top_k, args.max_seeds);
            render_json("kb.multi_hop", &response)
        }
    });
    ToolDescriptor::new(
        "kb.multi_hop",
        Some("HippoRAG-style multi-hop traversal (Personalised PageRank) across the SAP knowledge graph. Use this for impact / where-used / dependency-chain queries. Returns nodes with hop distance from any seed.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_hops": {"type": "integer", "minimum": 1, "maximum": 6, "default": 4},
                "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 8},
                "max_seeds": {"type": "integer", "minimum": 1, "maximum": 10, "default": 3}
            },
            "required": ["query"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

#[derive(Deserialize)]
struct GlobalQueryArgs {
    query: String,
    #[serde(default = "default_top_k_3")]
    top_k: usize,
}
fn default_top_k_3() -> usize { 3 }

fn kb_global_query(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: GlobalQueryArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("kb.global_query: invalid arguments: {e}"))),
            };
            let response = ctx.graph.community_query(&args.query, args.top_k);
            render_json("kb.global_query", &response)
        }
    });
    ToolDescriptor::new(
        "kb.global_query",
        Some("Microsoft GraphRAG community-level Q&A. Returns the top communities (clusters of related entities) that overlap the query, with their members and synthesised summary. Use this for global / analytical / cross-domain questions.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "minimum": 1, "maximum": 10, "default": 3}
            },
            "required": ["query"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

#[derive(Deserialize)]
struct SummariseArgs {
    #[serde(default = "default_level_2")]
    level: u32,
    #[serde(default = "default_top_k_10")]
    top_k: usize,
}
fn default_level_2() -> u32 { 2 }
fn default_top_k_10() -> usize { 10 }

fn kb_summarise(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: SummariseArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("kb.summarise: invalid arguments: {e}"))),
            };
            let response = ctx.graph.raptor_summary(args.level, args.top_k);
            render_json("kb.summarise", &response)
        }
    });
    ToolDescriptor::new(
        "kb.summarise",
        Some("RAPTOR hierarchical summary at the requested level (0 = leaves, 1 = Louvain communities, 2 = SAP module roll-ups). Use this for granularity-aware orientation queries.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "level": {"type": "integer", "minimum": 0, "maximum": 2, "default": 2},
                "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 10}
            },
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

#[derive(Deserialize)]
struct NeighborhoodArgs {
    seeds: Vec<String>,
    #[serde(default = "default_max_hops")]
    max_hops: u32,
    #[serde(default = "default_top_k_graph")]
    top_k: usize,
}

fn kb_graph_neighborhood(ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let ctx = Arc::clone(ctx);
    let handler = ToolFn(move |arguments: serde_json::Value| {
        let ctx = Arc::clone(&ctx);
        async move {
            let args: NeighborhoodArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("kb.graph_neighborhood: invalid arguments: {e}"))),
            };
            if args.seeds.is_empty() {
                return Ok(CallToolResult::error("kb.graph_neighborhood: seeds must not be empty"));
            }
            let response = ctx.graph.neighborhood(&args.seeds, args.max_hops, args.top_k);
            render_json("kb.graph_neighborhood", &response)
        }
    });
    ToolDescriptor::new(
        "kb.graph_neighborhood",
        Some("Multi-hop neighbourhood of an explicit set of entity IDs (e.g. ['abap:ZFIN_POST_JE', 'rfc:BAPI_ACC_DOCUMENT_POST']). PPR-ranked. Use after sap.docs.search or abap.adt.where_used has identified concrete entities.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "seeds": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 20},
                "max_hops": {"type": "integer", "minimum": 1, "maximum": 6, "default": 4},
                "top_k": {"type": "integer", "minimum": 1, "maximum": 50, "default": 8}
            },
            "required": ["seeds"],
            "additionalProperties": false
        })),
        Arc::new(handler),
    )
}

// ===========================================================================
// Workflow tools (Phase 6 — MCP 2025-06-18 elicitation)
// ===========================================================================
//
// Each tool walks an SAP-typical workflow that pauses mid-execution to
// confirm a high-stakes parameter with the user (cost centre, customer
// number, transport target).  This is the killer use case for the
// elicitation primitive from paper §II-B.

pub fn workflow_tools(ctx: &Arc<ServerContext>) -> Vec<ToolDescriptor> {
    vec![
        workflow_create_purchase_order(ctx),
        workflow_maintain_customer_master(ctx),
        workflow_release_transport(ctx),
    ]
}

fn workflow_create_purchase_order(_ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let handler = ToolFn(move |arguments: serde_json::Value| {
        async move {
            #[derive(Deserialize)]
            struct PoArgs {
                #[serde(default)] vendor: Option<String>,
                #[serde(default)] material: Option<String>,
                #[serde(default)] quantity: Option<f64>,
            }
            let args: PoArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.workflow.create_purchase_order: {e}"))),
            };

            // Elicit the high-stakes confirmation parameters.
            let schema = mcp_server::object_schema(
                serde_json::json!({
                    "vendor":      { "type": "string",  "description": "Vendor (LIFNR), e.g. 'V-100100'", "default": args.vendor.unwrap_or_default() },
                    "material":    { "type": "string",  "description": "Material (MATNR)", "default": args.material.unwrap_or_default() },
                    "quantity":    { "type": "number",  "description": "Order quantity", "default": args.quantity.unwrap_or(0.0) },
                    "cost_centre": { "type": "string",  "description": "Cost centre (KOSTL), e.g. '1000'" },
                    "company_code":{ "type": "string",  "description": "Company code (BUKRS)", "enum": ["1000", "2000", "3000"], "default": "1000" },
                    "currency":    { "type": "string",  "description": "Document currency", "enum": ["USD", "EUR", "GBP", "JPY", "SGD"], "default": "USD" },
                    "delivery_date":{ "type": "string", "description": "Requested delivery date (YYYY-MM-DD)" },
                }).as_object().unwrap().clone(),
                vec!["vendor".into(), "material".into(), "quantity".into(), "cost_centre".into(), "company_code".into(), "delivery_date".into()],
            );
            let elicit = mcp_server::elicit(
                "Confirm purchase-order details before posting. Cost centre + delivery date are mandatory.",
                schema,
            ).await;

            use mcp_core::ElicitationAction;
            match elicit.action {
                ElicitationAction::Accept => {
                    let content = elicit.content.unwrap_or_else(|| serde_json::json!({}));
                    Ok(CallToolResult::text(format!(
                        "Purchase order confirmed (mock execution; no real BAPI fired):\n\n{}\n\nNext step (when enable-writes): sap.rfc.call BAPI_PO_CREATE1.",
                        serde_json::to_string_pretty(&content).unwrap_or_default(),
                    )))
                }
                ElicitationAction::Decline => {
                    Ok(CallToolResult::text("Purchase order cancelled by user (declined elicitation)."))
                }
                ElicitationAction::Cancel => {
                    Ok(CallToolResult::error("Purchase order cancelled (user aborted or elicitation unavailable). No action taken."))
                }
            }
        }
    });
    ToolDescriptor::new(
        "sap.workflow.create_purchase_order",
        Some("Walk the user through a guided purchase-order creation. Mid-execution the tool elicits vendor, material, quantity, cost centre, company code, currency, and delivery date — declining the form cancels the operation without side-effects. Wires BAPI_PO_CREATE1 in write mode.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "vendor": {"type": "string", "description": "Initial vendor hint"},
                "material": {"type": "string", "description": "Initial material hint"},
                "quantity": {"type": "number", "description": "Initial quantity hint"}
            },
            "additionalProperties": false
        })),
        Arc::new(handler),
    ).with_writes()
}

fn workflow_maintain_customer_master(_ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let handler = ToolFn(move |arguments: serde_json::Value| {
        async move {
            #[derive(Deserialize)]
            struct CmArgs {
                #[serde(default)] customer: Option<String>,
            }
            let args: CmArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.workflow.maintain_customer_master: {e}"))),
            };
            // First elicit: which fields to change.
            let pick_schema = mcp_server::object_schema(
                serde_json::json!({
                    "customer":     { "type": "string", "description": "Customer (KUNNR)", "default": args.customer.unwrap_or_default() },
                    "scope":        { "type": "string", "description": "Which view to maintain", "enum": ["general_data", "company_code_data", "sales_area_data"], "default": "general_data" },
                    "company_code": { "type": "string", "description": "Company code (only required for company_code_data scope)" },
                }).as_object().unwrap().clone(),
                vec!["customer".into(), "scope".into()],
            );
            let pick = mcp_server::elicit("Select customer and which data view to maintain.", pick_schema).await;

            use mcp_core::ElicitationAction;
            if pick.action != ElicitationAction::Accept {
                return Ok(CallToolResult::error("Customer master maintenance cancelled at scope selection."));
            }
            let picked = pick.content.unwrap_or(serde_json::Value::Null);
            let customer = picked.get("customer").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let scope = picked.get("scope").and_then(|v| v.as_str()).unwrap_or("general_data").to_string();

            // Second elicit: scoped fields.
            let fields_schema = match scope.as_str() {
                "general_data" => serde_json::json!({
                    "name1": {"type": "string", "description": "Name 1"},
                    "city":  {"type": "string", "description": "City"},
                    "country": {"type": "string", "description": "Country (ISO-2)", "enum": ["DE", "US", "GB", "JP", "SG"]},
                }),
                "company_code_data" => serde_json::json!({
                    "recon_account": {"type": "string", "description": "Reconciliation account (HKONT), e.g. 140000"},
                    "payment_terms": {"type": "string", "description": "Payment terms (ZTERM)"},
                    "dunning_area":  {"type": "string", "description": "Dunning area"},
                }),
                "sales_area_data" => serde_json::json!({
                    "sales_org":      {"type": "string", "description": "Sales organisation"},
                    "distribution_channel": {"type": "string", "description": "Distribution channel"},
                    "division":       {"type": "string", "description": "Division"},
                    "incoterms":      {"type": "string", "enum": ["EXW", "FCA", "CIF", "DAP", "DDP"]},
                }),
                _ => serde_json::json!({}),
            };
            let confirm_schema = mcp_server::object_schema(
                fields_schema.as_object().unwrap().clone(),
                Vec::new(),
            );
            let confirm = mcp_server::elicit(
                &format!("Enter new values for customer {customer} ({scope} view). Leave fields blank to keep current values."),
                confirm_schema,
            ).await;

            match confirm.action {
                ElicitationAction::Accept => {
                    let changes = confirm.content.unwrap_or(serde_json::json!({}));
                    Ok(CallToolResult::text(format!(
                        "Customer master change confirmed (mock):\n  customer: {customer}\n  scope:    {scope}\n  changes:\n{}\n\nNext step (when enable-writes): sap.rfc.call BAPI_CUSTOMER_CHANGEFROMDATA.",
                        serde_json::to_string_pretty(&changes).unwrap_or_default(),
                    )))
                }
                ElicitationAction::Decline => Ok(CallToolResult::text("Customer master change declined; no action taken.")),
                ElicitationAction::Cancel  => Ok(CallToolResult::error("Customer master cancelled.")),
            }
        }
    });
    ToolDescriptor::new(
        "sap.workflow.maintain_customer_master",
        Some("Two-step elicitation walking the user through a customer master change: pick the data view, then fill in the scoped fields. Demonstrates *chained* elicitation calls — each step has its own form and either step can be declined safely.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {"customer": {"type": "string", "description": "Customer hint"}},
            "additionalProperties": false
        })),
        Arc::new(handler),
    ).with_writes()
}

fn workflow_release_transport(_ctx: &Arc<ServerContext>) -> ToolDescriptor {
    let handler = ToolFn(move |arguments: serde_json::Value| {
        async move {
            #[derive(Deserialize)]
            struct TrArgs { transport: Option<String> }
            let args: TrArgs = match serde_json::from_value(arguments) {
                Ok(a) => a,
                Err(e) => return Ok(CallToolResult::error(format!("sap.workflow.release_transport: {e}"))),
            };
            let initial = args.transport.unwrap_or_default();
            let schema = mcp_server::object_schema(
                serde_json::json!({
                    "transport":           { "type": "string", "description": "Transport request ID (TRKORR), e.g. ZTRA01K900123", "default": initial },
                    "target_system":       { "type": "string", "description": "Target system", "enum": ["DEV", "QA", "PRODUCTION"], "default": "QA" },
                    "release_dependents":  { "type": "boolean", "description": "Release dependent transports?", "default": false },
                    "skip_atc":            { "type": "boolean", "description": "Skip ATC checks (dangerous)", "default": false },
                    "confirmation_phrase": { "type": "string", "description": "Type the transport ID again to confirm" },
                }).as_object().unwrap().clone(),
                vec!["transport".into(), "target_system".into(), "confirmation_phrase".into()],
            );
            let elicit = mcp_server::elicit(
                "Transport release is irreversible in production. Confirm details and re-enter the transport ID to proceed.",
                schema,
            ).await;

            use mcp_core::ElicitationAction;
            match elicit.action {
                ElicitationAction::Accept => {
                    let v = elicit.content.unwrap_or(serde_json::Value::Null);
                    let tr     = v.get("transport").and_then(|x| x.as_str()).unwrap_or("");
                    let phrase = v.get("confirmation_phrase").and_then(|x| x.as_str()).unwrap_or("");
                    let target = v.get("target_system").and_then(|x| x.as_str()).unwrap_or("QA");
                    if tr != phrase {
                        return Ok(CallToolResult::error(format!(
                            "Confirmation phrase '{phrase}' does not match transport '{tr}'. Release aborted.",
                        )));
                    }
                    Ok(CallToolResult::text(format!(
                        "Transport release plan confirmed (mock):\n  transport:           {tr}\n  target_system:       {target}\n  release_dependents:  {}\n  skip_atc:            {}\n\nNext step (when enable-writes): TMS_MGR_FORWARD_TR_REQUEST.",
                        v.get("release_dependents").and_then(|x| x.as_bool()).unwrap_or(false),
                        v.get("skip_atc").and_then(|x| x.as_bool()).unwrap_or(false),
                    )))
                }
                ElicitationAction::Decline => Ok(CallToolResult::text("Transport release declined; no action taken.")),
                ElicitationAction::Cancel  => Ok(CallToolResult::error("Transport release cancelled (client lacks elicitation capability — refusing to proceed without confirmation).")),
            }
        }
    });
    ToolDescriptor::new(
        "sap.workflow.release_transport",
        Some("Release a transport request with a confirmation form. Requires the user to re-type the transport ID and explicitly opt in to dangerous flags (skip_atc, release_dependents). Refuses entirely on clients that don't advertise the elicitation capability.".into()),
        ToolInputSchema::from_value(serde_json::json!({
            "type": "object",
            "properties": {"transport": {"type": "string", "description": "Initial transport hint"}},
            "additionalProperties": false
        })),
        Arc::new(handler),
    ).with_writes()
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
