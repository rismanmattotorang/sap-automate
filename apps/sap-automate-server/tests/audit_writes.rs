//! Integration test for the transactional write path + audit wiring.
//!
//! Drives an in-process server over `tokio::io::duplex`.  In write mode a
//! `sap.rfc.call` with `commit=true` runs the BAPI then auto
//! commit-or-rollback, recording an audit entry inline.  This test exercises
//! that path end-to-end (the audit `record(...)` is awaited before the tool
//! returns, so a passing call proves the audit wiring runs without panic).
//! Audit content/redaction itself is unit-tested in `sap-automate-observability`.

use mcp_client::Client;
use mcp_core::{ClientCapabilities, Implementation};
use mcp_transport::stdio::StdioTransport;
use sap_automate_server_lib::{build_test_server, TestServerOptions};
use std::sync::Arc;

async fn connect(read_only: bool) -> Arc<Client> {
    let (server, _ctx) = build_test_server(TestServerOptions { read_only, ..Default::default() }).await;
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    let server_transport = StdioTransport::new(s_rx, s_tx);
    tokio::spawn(async move {
        let _ = server.run(server_transport).await;
    });
    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    client
        .initialize(
            Implementation { name: "audit-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    client
}

fn text_of(result: &mcp_core::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            mcp_core::ToolContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_write_runs_transactional_path_and_audit() {
    let client = connect(/* read_only = */ false).await;

    let result = client
        .call_tool(
            "sap.rfc.call",
            Some(serde_json::json!({
                "function": "BAPI_PO_CREATE1",
                "parameters": { "POHEADER": {}, "POHEADERX": {} },
                "commit": true
            })),
        )
        .await
        .expect("call sap.rfc.call commit");

    assert!(!result.is_error, "write call errored: {result:?}");
    let text = text_of(&result);
    let v: serde_json::Value = serde_json::from_str(&text).expect("json result");
    // The mock returns no BAPIRET2, so the fail-closed path must report the
    // write as not committed (and rolled back) rather than committing on faith.
    assert_eq!(v["committed"], serde_json::json!(false), "got: {v}");
    assert!(v.get("rolled_back").is_some(), "expected rolled_back field; got: {v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_write_is_denied_in_read_only_mode() {
    let client = connect(/* read_only = */ true).await;

    let result = client
        .call_tool(
            "sap.rfc.call",
            Some(serde_json::json!({
                "function": "BAPI_PO_CREATE1",
                "parameters": { "POHEADER": {}, "POHEADERX": {} },
                "commit": true
            })),
        )
        .await
        .expect("call sap.rfc.call commit (read-only)");

    assert!(result.is_error, "commit must be refused in read-only mode");
    assert!(
        text_of(&result).to_lowercase().contains("read-only")
            || text_of(&result).to_lowercase().contains("permission")
            || text_of(&result).to_lowercase().contains("write"),
        "expected a read-only/permission message; got: {}",
        text_of(&result)
    );
}
