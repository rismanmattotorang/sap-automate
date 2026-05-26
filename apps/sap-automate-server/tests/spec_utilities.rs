//! Integration tests for the MCP 2025-06-18 optional utilities:
//!   - `logging/setLevel`
//!   - `completion/complete`
//!
//! Drives the in-process `build_test_server` over a duplex transport.
//! Notification-side utilities (`notifications/progress`,
//! `notifications/message`) need a different client harness (push-based);
//! those are covered by unit tests in `mcp-server` / `mcp-core`.

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
            Implementation { name: "spec-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logging_setlevel_is_accepted() {
    let client = connect().await;
    // logging/setLevel returns {} (empty result) per spec.
    let r: serde_json::Value = client
        .raw_request("logging/setLevel", Some(serde_json::json!({"level": "warning"})))
        .await
        .expect("setLevel");
    assert_eq!(r, serde_json::json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logging_setlevel_validates_enum() {
    let client = connect().await;
    let r: mcp_core::Result<serde_json::Value> = client
        .raw_request("logging/setLevel", Some(serde_json::json!({"level": "verbose"})))
        .await;
    assert!(r.is_err(), "invalid level should produce a JSON-RPC error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_complete_returns_registered_values() {
    let client = connect().await;
    // sap.skill.security_sod_audit's `scope` argument has a completer
    // registered in lib.rs that returns user / role / system.
    let r: serde_json::Value = client
        .raw_request(
            "completion/complete",
            Some(serde_json::json!({
                "ref": {"type": "ref/prompt", "name": "sap.skill.security_sod_audit"},
                "argument": {"name": "scope", "value": ""},
            })),
        )
        .await
        .expect("complete");
    let values = r["completion"]["values"].as_array().expect("values");
    let strs: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs.len(), 3, "expected 3 completions, got: {strs:?}");
    assert!(strs.contains(&"user"));
    assert!(strs.contains(&"role"));
    assert!(strs.contains(&"system"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_complete_filters_by_prefix() {
    let client = connect().await;
    let r: serde_json::Value = client
        .raw_request(
            "completion/complete",
            Some(serde_json::json!({
                "ref": {"type": "ref/prompt", "name": "sap.skill.security_sod_audit"},
                "argument": {"name": "scope", "value": "u"},
            })),
        )
        .await
        .expect("complete");
    let values = r["completion"]["values"].as_array().expect("values");
    let strs: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(strs, vec!["user"], "only 'user' starts with 'u'; got {strs:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_complete_unknown_prompt_returns_empty_not_error() {
    let client = connect().await;
    let r: serde_json::Value = client
        .raw_request(
            "completion/complete",
            Some(serde_json::json!({
                "ref": {"type": "ref/prompt", "name": "no.such.prompt"},
                "argument": {"name": "x", "value": ""},
            })),
        )
        .await
        .expect("complete");
    let values = r["completion"]["values"].as_array().expect("values");
    assert_eq!(values.len(), 0, "unknown prompt should return [], not error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_advertises_logging_and_completions_capabilities() {
    let (server, _ctx) = build_test_server(TestServerOptions::default()).await;
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    let server_transport = StdioTransport::new(s_rx, s_tx);
    tokio::spawn(async move { let _ = server.run(server_transport).await; });
    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    let init = client
        .initialize(
            Implementation { name: "cap-test".into(), version: "0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    let caps = serde_json::to_value(&init.capabilities).expect("serialize caps");
    assert!(caps.get("logging").is_some(),
        "server must declare `logging` capability when logging/setLevel is supported; got {caps}");
    assert!(caps.get("completions").is_some(),
        "server must declare `completions` capability when completers are registered; got {caps}");
}
