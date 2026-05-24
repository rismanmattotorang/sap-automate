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

    /// Extra arguments to pass through to the server binary (repeatable).
    #[arg(long = "server-arg", num_args = 1)]
    server_args: Vec<String>,

    /// Optional tool to call after `tools/list`.
    /// Two formats supported:
    ///   - `name=key=val,key=val` (legacy; values are JSON-parsed)
    ///   - `name='{"key": "val", ...}'` (full JSON object; preferred for
    ///     nested / array-valued arguments)
    #[arg(long)]
    call: Option<String>,

    /// Optional second tool to call.
    #[arg(long)]
    then: Option<String>,

    /// List tools / resources / prompts and exit (no tool calls).
    #[arg(long)]
    list: bool,

    /// Read a resource by URI and print it.
    #[arg(long)]
    read_resource: Option<String>,

    /// Instantiate a prompt / skill by name with optional JSON arguments.
    /// Format: `name` or `name={"k": "v"}`.
    #[arg(long)]
    get_prompt: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let mut child = Command::new(&cli.server)
        .args(&cli.server_args)
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

    if cli.list {
        let resources = client.list_resources().await?;
        println!("== Resources ({})", resources.resources.len());
        for r in &resources.resources { println!("  - {} ({})", r.uri, r.name); }
        println!();
        let prompts = client.list_prompts().await?;
        println!("== Prompts ({})", prompts.prompts.len());
        for p in &prompts.prompts {
            println!("  - {} — {}", p.name, p.description.as_deref().unwrap_or(""));
        }
        println!();
    }

    if let Some(uri) = cli.read_resource.as_deref() {
        println!("== Reading resource {uri}");
        let r = client.read_resource(uri).await?;
        for c in &r.contents {
            if let Some(text) = &c.text { println!("{text}"); }
        }
        println!();
    }

    if let Some(spec) = cli.get_prompt.as_deref() {
        let (name, args) = parse_call_spec(spec)?;
        let arg_val = if matches!(args, serde_json::Value::Object(ref m) if m.is_empty()) { None } else { Some(args) };
        println!("== Prompt {name}");
        let result = client.get_prompt(&name, arg_val).await?;
        if let Some(d) = &result.description { println!("Description: {d}"); }
        for m in &result.messages {
            if let mcp_core::ToolContent::Text { text } = &m.content {
                println!("\n{text}");
            }
        }
        println!();
    }

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

/// Parse a call spec.  Two forms accepted:
///
///   `name={"k": "v", ...}`  -- the JSON object after `=` is parsed as-is.
///   `name=k=v,k=v`          -- legacy form; pairs are comma-separated and
///                              the values are JSON-parsed individually.
///
/// The JSON form wins whenever the rest after the first `=` parses as a
/// JSON object — this lets nested objects / arrays / strings-with-commas
/// pass through without escaping.
fn parse_call_spec(spec: &str) -> anyhow::Result<(String, serde_json::Value)> {
    let (name, rest) = spec.split_once('=').unwrap_or((spec, ""));
    if rest.is_empty() {
        return Ok((name.into(), serde_json::Value::Object(serde_json::Map::new())));
    }
    // Form 1: full JSON object.
    let trimmed = rest.trim();
    if trimmed.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| anyhow::anyhow!("invalid JSON for tool '{name}': {e}"))?;
        return Ok((name.into(), parsed));
    }
    // Form 2: legacy comma-separated key=value list.
    let mut obj = serde_json::Map::new();
    for pair in rest.split(',') {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid pair '{pair}'; expected key=value or use JSON form"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into()));
        obj.insert(k.into(), parsed);
    }
    Ok((name.into(), serde_json::Value::Object(obj)))
}
