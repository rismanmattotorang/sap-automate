//! Integration test for the RFC metadata cache wiring.
//!
//! Drives an in-process server over `tokio::io::duplex` — no subprocess,
//! no embedding seed cost. Verifies:
//!   1. `sap.system.cache_stats` + `sap.system.cache_invalidate` appear
//!      in `tools/list`.
//!   2. `sap-cache://stats` appears in `resources/list`.
//!   3. Two `sap.rfc.metadata` calls for the same function move the hit
//!      counter forward — the cache decorator is in the live request path.
//!   4. `sap.system.cache_invalidate` drops every entry.
//!
//! Verify (Karpathy goal-driven execution): these are the success criteria
//! that close the loop on the metadata-cache wiring through the server.

use mcp_client::Client;
use mcp_core::{ClientCapabilities, Implementation};
use mcp_transport::stdio::StdioTransport;
use sap_automate_server_lib::{build_test_server, TestServerOptions};
use std::sync::Arc;

async fn connect() -> Arc<Client> {
    let (server, _ctx) = build_test_server(TestServerOptions::default()).await;

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
            Implementation { name: "cache-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_tools_and_resource_present() {
    let client = connect().await;

    let tools = client.list_tools().await.expect("list_tools");
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"sap.system.cache_stats"), "missing sap.system.cache_stats; have: {names:?}");
    assert!(names.contains(&"sap.system.cache_invalidate"), "missing sap.system.cache_invalidate; have: {names:?}");

    let resources = client.list_resources().await.expect("list_resources");
    let uris: Vec<&str> = resources.resources.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"sap-cache://stats"), "missing sap-cache://stats; have: {uris:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_metadata_call_hits_cache() {
    let client = connect().await;

    let _ = client
        .call_tool(
            "sap.rfc.metadata",
            Some(serde_json::json!({"function": "BAPI_MATERIAL_GET_DETAIL"})),
        )
        .await
        .expect("first metadata call");

    let stats1 = extract_json(
        &client.call_tool("sap.system.cache_stats", Some(serde_json::json!({}))).await.expect("stats 1"),
    );
    assert!(stats1["enabled"].as_bool().unwrap_or(false), "cache should be enabled by default");
    let hits1 = stats1["hits"].as_u64().unwrap_or(0);

    let _ = client
        .call_tool(
            "sap.rfc.metadata",
            Some(serde_json::json!({"function": "BAPI_MATERIAL_GET_DETAIL"})),
        )
        .await
        .expect("second metadata call");

    let stats2 = extract_json(
        &client.call_tool("sap.system.cache_stats", Some(serde_json::json!({}))).await.expect("stats 2"),
    );
    let hits2 = stats2["hits"].as_u64().unwrap_or(0);

    assert!(hits2 > hits1, "expected hits to grow on repeat call: {hits1} -> {hits2}");
    assert!(stats2["entries"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalidate_drops_entries() {
    let client = connect().await;

    let _ = client
        .call_tool(
            "sap.rfc.metadata",
            Some(serde_json::json!({"function": "BAPI_MATERIAL_GET_DETAIL"})),
        )
        .await
        .expect("warm");
    let before = extract_json(
        &client.call_tool("sap.system.cache_stats", Some(serde_json::json!({}))).await.expect("stats before"),
    );
    assert!(before["entries"].as_u64().unwrap_or(0) >= 1);

    let inv = extract_json(
        &client.call_tool("sap.system.cache_invalidate", Some(serde_json::json!({}))).await.expect("invalidate"),
    );
    assert_eq!(inv["ok"].as_bool(), Some(true));
    assert!(inv["entries_dropped"].as_u64().unwrap_or(0) >= 1);

    let after = extract_json(
        &client.call_tool("sap.system.cache_stats", Some(serde_json::json!({}))).await.expect("stats after"),
    );
    assert_eq!(after["entries"].as_u64().unwrap_or(99), 0);
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
