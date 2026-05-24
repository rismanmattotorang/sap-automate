//! End-to-end integration test: an in-process MCP client drives an in-process
//! MCP server over a paired `tokio::io::duplex` transport.  This validates the
//! full initialise → list_tools → call_tool path without spawning processes.

use mcp_client::Client;
use mcp_core::{CallToolResult, ClientCapabilities, Implementation, ToolContent, ToolInputSchema};
use mcp_server::registry::ToolFn;
use mcp_server::{Server, ToolDescriptor};
use mcp_transport::stdio::StdioTransport;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_list_call_roundtrip() {
    // Server side: a single `add` tool.
    let server = Server::builder("test-server", "0.1.0")
        .tool(make_add_tool())
        .build();

    // Wire a duplex pipe: server reads from `s_rx`, writes to `s_tx`;
    // client reads from `c_rx`, writes to `c_tx`.
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);

    let server_transport = StdioTransport::new(s_rx, s_tx);
    tokio::spawn(async move {
        server.run(server_transport).await.unwrap();
    });

    let client_transport = StdioTransport::new(c_rx, c_tx);
    let client = Client::spawn(client_transport);

    let init = client
        .initialize(
            Implementation { name: "test-client".into(), version: "0.1.0".into() },
            ClientCapabilities::default(),
        )
        .await
        .expect("initialize");
    assert_eq!(init.server_info.name, "test-server");
    assert_eq!(init.protocol_version, mcp_core::protocol::PROTOCOL_VERSION);

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "add");

    let result = client
        .call_tool("add", Some(serde_json::json!({"a": 2.0, "b": 3.0})))
        .await
        .expect("call_tool");
    assert!(!result.is_error);
    match &result.content[0] {
        ToolContent::Text { text } => assert_eq!(text, "5"),
        other => panic!("unexpected content: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_returns_protocol_error() {
    let server = Server::builder("test-server", "0.1.0").build();
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    tokio::spawn(async move { server.run(StdioTransport::new(s_rx, s_tx)).await.unwrap(); });

    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    client
        .initialize(
            Implementation { name: "test-client".into(), version: "0.1.0".into() },
            ClientCapabilities::default(),
        )
        .await
        .unwrap();

    let err = client.call_tool("nope", None).await;
    match err {
        Err(mcp_core::Error::Protocol { code, .. }) => {
            assert_eq!(code, mcp_core::error::ErrorCode::UnknownTool.as_i32());
        }
        other => panic!("expected UnknownTool, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exposure_policy_hides_write_tools() {
    // Read-only policy (default): add tool is hidden because it's marked
    // with_writes(); echo stays visible.
    let server = Server::builder("test-server", "0.1.0")
        .tool(make_add_tool().with_writes())
        .tool(make_echo_tool())
        .build();
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    tokio::spawn(async move { server.run(StdioTransport::new(s_rx, s_tx)).await.unwrap(); });
    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    client.initialize(
        Implementation { name: "test-client".into(), version: "0.1.0".into() },
        ClientCapabilities::default(),
    ).await.unwrap();
    let tools = client.list_tools().await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"), "echo should be visible: {names:?}");
    assert!(!names.contains(&"add"), "add (writes) must be hidden in read-only: {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exposure_policy_all_shows_write_tools() {
    let server = Server::builder("test-server", "0.1.0")
        .exposure(mcp_server::ExposurePolicy::All)
        .tool(make_add_tool().with_writes())
        .tool(make_echo_tool())
        .build();
    let (s_rx, c_tx) = tokio::io::duplex(8192);
    let (c_rx, s_tx) = tokio::io::duplex(8192);
    tokio::spawn(async move { server.run(StdioTransport::new(s_rx, s_tx)).await.unwrap(); });
    let client = Client::spawn(StdioTransport::new(c_rx, c_tx));
    client.initialize(
        Implementation { name: "test-client".into(), version: "0.1.0".into() },
        ClientCapabilities::default(),
    ).await.unwrap();
    let tools = client.list_tools().await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"add"));
}

fn make_echo_tool() -> ToolDescriptor {
    #[derive(serde::Deserialize)]
    struct Args { text: String }
    let handler = ToolFn(|args: serde_json::Value| async move {
        let p: Args = serde_json::from_value(args).unwrap();
        Ok(CallToolResult::text(p.text))
    });
    let schema = ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"]
    }));
    ToolDescriptor::new("echo", Some("echo text".into()), schema, Arc::new(handler))
}

fn make_add_tool() -> ToolDescriptor {
    #[derive(serde::Deserialize)]
    struct Args { a: f64, b: f64 }
    let handler = ToolFn(|args: serde_json::Value| async move {
        let p: Args = serde_json::from_value(args).unwrap();
        Ok(CallToolResult::text(format!("{}", p.a + p.b)))
    });
    let schema = ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
        "required": ["a", "b"]
    }));
    ToolDescriptor::new("add", Some("Add two numbers".into()), schema, Arc::new(handler))
}
