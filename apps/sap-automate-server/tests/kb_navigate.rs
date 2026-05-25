//! Integration test for `sap.kb.navigate` — the OpenKB + PageIndex
//! document-tree navigation tool.
//!
//! Drives an in-process server (via the same `build_test_server` helper as
//! cache_tools.rs).  Seeds a document with a 3-level heading structure,
//! verifies:
//!   1. `sap.kb.navigate` is registered.
//!   2. Calling it with no `path` returns the root + its top-level
//!      children.
//!   3. Calling it with `path=1.2` returns the nested section verbatim.
//!   4. A missing document yields a clean error, not a crash.

use mcp_client::Client;
use mcp_core::{ClientCapabilities, Implementation};
use mcp_transport::stdio::StdioTransport;
use sap_automate_kb::{Document, Domain, UpsertBatch};
use sap_automate_server_lib::{build_test_server, TestServerOptions};
use std::sync::Arc;

const SEED_BODY: &str = r#"
# Period-End Close
Open posting periods via T001B. Close periods at month-end.

## Foreign Currency Revaluation
Run F.05 against company code BUKRS. Posts to FAGLFLEXA.

### Posting Logic
The clearing document is written through BAPI_ACC_DOCUMENT_POST.

## Reconciliation
Reconcile BSEG to FAGLFLEXA before final close.
"#;

async fn connect_and_seed() -> Arc<Client> {
    let (server, ctx) = build_test_server(TestServerOptions::default()).await;

    // Seed one document directly through the in-process store.
    let store = ctx.rag.store();
    let doc = Document::new("sap_help:demo-pec", Domain::SapHelp, "u://demo", "Period-End Close", SEED_BODY);
    store.upsert(UpsertBatch { documents: vec![doc], chunks: vec![] })
        .await
        .expect("seed upsert");

    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    let server_transport = StdioTransport::new(s_rx, s_tx);
    tokio::spawn(async move {
        let _ = server.run(server_transport).await;
    });

    let client_transport = StdioTransport::new(c_rx, c_tx);
    let client = Client::spawn(client_transport);
    let _ = client
        .initialize(
            Implementation { name: "kb-nav-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_tool_is_registered() {
    let client = connect_and_seed().await;
    let tools = client.list_tools().await.expect("list_tools");
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"sap.kb.navigate"), "missing sap.kb.navigate; have: {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_root_returns_top_level_children() {
    let client = connect_and_seed().await;
    let result = client
        .call_tool("sap.kb.navigate", Some(serde_json::json!({
            "document_id": "sap_help:demo-pec",
        })))
        .await
        .expect("call");
    let body = extract_json(&result);
    assert_eq!(body["document_id"].as_str(), Some("sap_help:demo-pec"));
    assert!(body["max_depth"].as_u64().unwrap_or(0) >= 3);
    // The root's children should be the H1s — just "Period-End Close".
    let node = &body["node"];
    let children = node["children"].as_array().expect("children");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["title"].as_str(), Some("Period-End Close"));
    assert_eq!(children[0]["path"].as_str(), Some("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_subpath_returns_named_section() {
    let client = connect_and_seed().await;
    let result = client
        .call_tool("sap.kb.navigate", Some(serde_json::json!({
            "document_id": "sap_help:demo-pec",
            "path": "1.1",
            "depth": 2,
        })))
        .await
        .expect("call");
    let body = extract_json(&result);
    let node = &body["node"];
    assert_eq!(node["title"].as_str(), Some("Foreign Currency Revaluation"));
    assert_eq!(node["path"].as_str(), Some("1.1"));
    // Should include the "Posting Logic" sub-section.
    let children = node["children"].as_array().expect("children");
    assert!(children.iter().any(|c| c["title"] == "Posting Logic"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_missing_doc_returns_clean_error() {
    let client = connect_and_seed().await;
    let result = client
        .call_tool("sap.kb.navigate", Some(serde_json::json!({
            "document_id": "sap_help:does-not-exist",
        })))
        .await
        .expect("call");
    assert!(result.is_error, "expected isError=true for missing document");
    // Error message should mention the missing doc.
    let text = result.content.iter().find_map(|c| {
        if let mcp_core::ToolContent::Text { text } = c { Some(text.clone()) } else { None }
    }).unwrap_or_default();
    assert!(text.contains("does-not-exist") || text.contains("not found"), "got: {text}");
}

fn extract_json(result: &mcp_core::CallToolResult) -> serde_json::Value {
    assert!(!result.is_error, "tool returned error: {result:?}");
    for c in &result.content {
        if let mcp_core::ToolContent::Text { text } = c {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                return v;
            }
        }
    }
    panic!("no JSON text content in result");
}
