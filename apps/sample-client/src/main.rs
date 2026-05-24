//! Sample MCP client.
//!
//! Spawns a child MCP server process, pipes stdio, performs the initialise
//! handshake, lists the tool catalogue, and invokes a small set of tools.
//! Acts as a Phase 1 acceptance harness for the framework.

use clap::Parser;
use mcp_client::Client;
use mcp_core::{ClientCapabilities, Implementation};
use mcp_transport::stdio::StdioTransport;
use std::process::Stdio;
use tokio::process::{ChildStdin, ChildStdout, Command};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "sample-client",
    about = "Drive an MCP server over stdio for smoke-testing and demos."
)]
struct Cli {
    /// Path to the MCP server binary to spawn.
    #[arg(long, default_value = "target/debug/sap-automate-server")]
    server: String,

    /// Optional tool to call after `tools/list`.  Format: name=key=val,key=val.
    #[arg(long)]
    call: Option<String>,

    /// Optional second tool to call.
    #[arg(long)]
    then: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let mut child = Command::new(&cli.server)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn '{}': {e}", cli.server))?;

    let stdin: ChildStdin = child.stdin.take().expect("piped stdin");
    let stdout: ChildStdout = child.stdout.take().expect("piped stdout");

    let transport = StdioTransport::new(stdout, stdin);
    let client = Client::spawn(transport);

    let init = client
        .initialize(
            Implementation {
                name: "sample-client".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            ClientCapabilities::default(),
        )
        .await?;

    println!(
        "== Connected to {} v{} (protocol {})",
        init.server_info.name, init.server_info.version, init.protocol_version
    );
    if let Some(instr) = &init.instructions {
        println!("Server: {instr}\n");
    }

    let tools = client.list_tools().await?;
    println!("== Tools ({})", tools.tools.len());
    for t in &tools.tools {
        println!(
            "  - {}{}",
            t.name,
            t.description.as_deref().map(|d| format!(" — {d}")).unwrap_or_default()
        );
    }
    println!();

    for spec in [cli.call.as_deref(), cli.then.as_deref()].into_iter().flatten() {
        invoke(&client, spec).await?;
    }

    // Gracefully end.  Closing the child's stdin causes its event loop to exit.
    drop(client);
    let _ = child.wait().await;
    Ok(())
}

async fn invoke(client: &std::sync::Arc<Client>, spec: &str) -> anyhow::Result<()> {
    let (name, args) = parse_call_spec(spec)?;
    println!("== Calling {name} with {args}");
    let result = client.call_tool(&name, Some(args)).await?;
    for c in &result.content {
        match c {
            mcp_core::ToolContent::Text { text } => println!("{text}"),
            other => println!("<non-text content: {other:?}>"),
        }
    }
    if result.is_error {
        println!("(tool reported error)");
    }
    println!();
    Ok(())
}

/// Parse `name=k=v,k=v` into (name, json object).  Values are parsed as JSON
/// (so numbers stay numeric); fallback to string.
fn parse_call_spec(spec: &str) -> anyhow::Result<(String, serde_json::Value)> {
    let (name, rest) = spec.split_once('=').unwrap_or((spec, ""));
    let mut obj = serde_json::Map::new();
    if !rest.is_empty() {
        for pair in rest.split(',') {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("invalid pair '{pair}'; expected key=value"))?;
            let parsed: serde_json::Value =
                serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into()));
            obj.insert(k.into(), parsed);
        }
    }
    Ok((name.into(), serde_json::Value::Object(obj)))
}
