//! Sample MCP client.
//!
//! Spawns a child MCP server process, pipes stdio, performs the initialise
//! handshake, lists the tool catalogue, and invokes a small set of tools.
//! Acts as a Phase 1 acceptance harness for the framework.

use async_trait::async_trait;
use clap::Parser;
use mcp_client::{Client, ElicitationDelegate};
use mcp_core::{ClientCapabilities, ElicitationAction, ElicitationParams, ElicitationResult, Implementation};
use mcp_transport::stdio::StdioTransport;
use std::process::Stdio;
use std::sync::Arc;
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

    /// Elicitation delegate behaviour:
    ///   - `decline` (default) — refuse every elicitation
    ///   - `accept` — auto-accept with the server's `default` values where present
    ///   - `seed:{"key":"value", ...}` — auto-accept with the given object
    ///   - `stdin` — interactively prompt for each field from stdin
    #[arg(long, default_value = "decline")]
    elicit: String,
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
    let delegate = build_delegate(&cli.elicit)?;
    let advertise_elicit = !matches!(cli.elicit.as_str(), "decline");
    // Split-half spawn so server-initiated requests (elicitation) don't
    // deadlock against client-initiated calls under load.
    let client = Client::spawn_stdio(transport, delegate);

    let mut capabilities = ClientCapabilities::default();
    if advertise_elicit {
        capabilities = capabilities.with_elicitation();
    }

    let init = client
        .initialize(
            Implementation {
                name: "sample-client".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities,
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

// ----------------------------------------------------------------------------
// Elicitation delegates
// ----------------------------------------------------------------------------

fn build_delegate(spec: &str) -> anyhow::Result<Arc<dyn ElicitationDelegate>> {
    if spec == "decline" {
        return Ok(Arc::new(mcp_client::DeclineAll));
    }
    if spec == "accept" {
        return Ok(Arc::new(AcceptDefaults));
    }
    if spec == "stdin" {
        return Ok(Arc::new(StdinDelegate));
    }
    if let Some(json) = spec.strip_prefix("seed:") {
        let parsed: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("--elicit seed must be JSON: {e}"))?;
        return Ok(Arc::new(SeededDelegate { content: parsed }));
    }
    anyhow::bail!("--elicit expected one of: decline | accept | stdin | seed:<json>");
}

/// Accept every elicitation with the schema's declared `default` values
/// (when present) and empty strings / zeros otherwise.  Good for
/// non-interactive smoke tests.
struct AcceptDefaults;

#[async_trait]
impl ElicitationDelegate for AcceptDefaults {
    async fn on_elicit(&self, params: ElicitationParams) -> ElicitationResult {
        let content = synthesise_defaults(&params.requested_schema);
        eprintln!("[elicit:accept] {}", params.message);
        eprintln!("[elicit:accept] -> {}", content);
        ElicitationResult { action: ElicitationAction::Accept, content: Some(content) }
    }
}

/// Accept every elicitation with the same hard-coded payload (which may
/// be missing fields the server requires).  Useful for end-to-end demos.
struct SeededDelegate { content: serde_json::Value }

#[async_trait]
impl ElicitationDelegate for SeededDelegate {
    async fn on_elicit(&self, params: ElicitationParams) -> ElicitationResult {
        eprintln!("[elicit:seed] {}", params.message);
        eprintln!("[elicit:seed] -> {}", self.content);
        ElicitationResult { action: ElicitationAction::Accept, content: Some(self.content.clone()) }
    }
}

/// Interactive: print the form to stderr, read each field from stdin.
struct StdinDelegate;

#[async_trait]
impl ElicitationDelegate for StdinDelegate {
    async fn on_elicit(&self, params: ElicitationParams) -> ElicitationResult {
        use std::io::Write;
        eprintln!("\n=== elicitation ===");
        eprintln!("{}", params.message);
        let props = params.requested_schema.get("properties").and_then(|v| v.as_object());
        let required: std::collections::HashSet<String> = params.requested_schema
            .get("required").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let mut out = serde_json::Map::new();
        if let Some(props) = props {
            for (name, spec) in props {
                let typ = spec.get("type").and_then(|v| v.as_str()).unwrap_or("string");
                let desc = spec.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let default = spec.get("default");
                let enum_vals = spec.get("enum").and_then(|v| v.as_array());
                let req = required.contains(name);
                eprint!("  {} ({typ}{}{}{}) {desc}",
                    name,
                    if req { ", required" } else { "" },
                    if let Some(d) = &default { format!(", default {}", d) } else { String::new() },
                    if let Some(e) = enum_vals { format!(", enum {}", serde_json::Value::Array(e.clone())) } else { String::new() },
                );
                if let Some(d) = default { eprint!(" [{d}]"); }
                eprint!(": ");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_err() { break; }
                let trimmed = line.trim();
                let v = if trimmed.is_empty() {
                    if let Some(d) = default { d.clone() } else { continue; }
                } else if typ == "number" || typ == "integer" {
                    if let Ok(n) = trimmed.parse::<f64>() { serde_json::json!(n) } else { serde_json::Value::String(trimmed.into()) }
                } else if typ == "boolean" {
                    serde_json::Value::Bool(matches!(trimmed.to_lowercase().as_str(), "true" | "y" | "yes" | "1"))
                } else {
                    serde_json::Value::String(trimmed.into())
                };
                out.insert(name.clone(), v);
            }
        }
        eprintln!("=== submitting ===\n");
        ElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(serde_json::Value::Object(out)),
        }
    }
}

fn synthesise_defaults(schema: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let Some(props) = schema.get("properties").and_then(|v| v.as_object()) else {
        return serde_json::Value::Object(out);
    };
    for (name, spec) in props {
        if let Some(d) = spec.get("default") {
            out.insert(name.clone(), d.clone());
            continue;
        }
        let typ = spec.get("type").and_then(|v| v.as_str()).unwrap_or("string");
        let v = match typ {
            "number" | "integer" => serde_json::json!(0),
            "boolean" => serde_json::json!(false),
            "array" => serde_json::json!([]),
            "object" => serde_json::json!({}),
            _ => serde_json::Value::String(String::new()),
        };
        out.insert(name.clone(), v);
    }
    serde_json::Value::Object(out)
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
