//! Minimal MCP server: showcases the framework with two tools and one prompt.
//!
//! Useful for smoke-testing the protocol layer end-to-end without any SAP
//! domain code in the way.

use mcp_core::{CallToolResult, ToolInputSchema};
use mcp_server::{Server, registry::ToolFn};
use mcp_server::ToolDescriptor;
use mcp_transport::StdioTransport;
use serde::Deserialize;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let server = Server::builder("sample-server", env!("CARGO_PKG_VERSION"))
        .instructions("Sample server with two demonstration tools: echo and add.")
        .tool(echo_tool())
        .tool(add_tool())
        .build();

    server.run(StdioTransport::from_stdio()).await?;
    Ok(())
}

#[derive(Deserialize)]
struct EchoArgs { text: String }

fn echo_tool() -> ToolDescriptor {
    let schema = ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"]
    }));
    let handler = ToolFn(|args: serde_json::Value| async move {
        let parsed: EchoArgs = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return Ok(CallToolResult::error(format!("invalid arguments: {e}"))),
        };
        Ok(CallToolResult::text(parsed.text))
    });
    ToolDescriptor::new("echo", Some("Echo text back".into()), schema, Arc::new(handler))
}

#[derive(Deserialize)]
struct AddArgs { a: f64, b: f64 }

fn add_tool() -> ToolDescriptor {
    let schema = ToolInputSchema::from_value(serde_json::json!({
        "type": "object",
        "properties": {
            "a": {"type": "number"},
            "b": {"type": "number"}
        },
        "required": ["a", "b"]
    }));
    let handler = ToolFn(|args: serde_json::Value| async move {
        let parsed: AddArgs = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return Ok(CallToolResult::error(format!("invalid arguments: {e}"))),
        };
        Ok(CallToolResult::text(format!("{}", parsed.a + parsed.b)))
    });
    ToolDescriptor::new("add", Some("Add two numbers".into()), schema, Arc::new(handler))
}
