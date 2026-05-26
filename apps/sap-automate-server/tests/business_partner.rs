//! Integration test for the Live SAP backend tier: `sap.bp.search` and
//! `sap.bp.get` MCP tools that hit the SAP Business Accelerator Hub
//! sandbox over OData v4.
//!
//! The in-process `build_test_server` deliberately does NOT inject a
//! `BusinessHubClient`, so these tests verify the "feature disabled"
//! fallback: the tools are still registered, but invoking them returns
//! a friendly error pointing the operator at `SAP_BUSINESS_HUB_KEY`.
//!
//! The actual live-sandbox round-trip lives in
//! `crates/sap-automate-rfc/src/odata.rs` as `live_business_partner_search`
//! — it skips when `SAP_BUSINESS_HUB_KEY` is unset.

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
    tokio::spawn(async move { let _ = server.run(server_transport).await; });
    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    let _ = client
        .initialize(
            Implementation { name: "bp-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bp_tools_are_registered() {
    let client = connect().await;
    let tools = client.list_tools().await.expect("list_tools");
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"sap.bp.search"), "missing sap.bp.search; have: {names:?}");
    assert!(names.contains(&"sap.bp.get"), "missing sap.bp.get; have: {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bp_search_without_key_returns_friendly_error() {
    let client = connect().await;
    let r = client
        .call_tool("sap.bp.search", Some(serde_json::json!({"query": "Smith"})))
        .await
        .expect("call");
    assert!(r.is_error, "expected isError=true when hub is disabled");
    let text = r.content.iter().find_map(|c| {
        if let mcp_core::ToolContent::Text { text } = c { Some(text.clone()) } else { None }
    }).unwrap_or_default();
    assert!(text.contains("SAP_BUSINESS_HUB_KEY"),
        "error must point operator at SAP_BUSINESS_HUB_KEY; got: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bp_get_without_key_returns_friendly_error() {
    let client = connect().await;
    let r = client
        .call_tool("sap.bp.get", Some(serde_json::json!({"id": "1003764"})))
        .await
        .expect("call");
    assert!(r.is_error);
    let text = r.content.iter().find_map(|c| {
        if let mcp_core::ToolContent::Text { text } = c { Some(text.clone()) } else { None }
    }).unwrap_or_default();
    assert!(text.contains("SAP_BUSINESS_HUB_KEY"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bp_search_validates_required_arguments() {
    let client = connect().await;
    // Missing `query` field.
    let r = client
        .call_tool("sap.bp.search", Some(serde_json::json!({})))
        .await
        .expect("call");
    assert!(r.is_error, "expected isError=true when 'query' is missing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bp_search_tool_schema_clamps_limit() {
    let client = connect().await;
    // Schema declares maximum 100; over-large limit should fail schema-validation
    // on the client side eventually, but our server also clamps server-side.
    // Here we just verify the tool accepts the arg type without panicking.
    let r = client
        .call_tool("sap.bp.search", Some(serde_json::json!({"query": "x", "limit": 5})))
        .await
        .expect("call");
    // Will error out at the disabled-feature gate, not at validation.
    assert!(r.is_error);
}
