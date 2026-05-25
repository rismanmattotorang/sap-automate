//! MCP server: capability router + dispatch loop.

use mcp_core::{
    error::ErrorCode,
    jsonrpc::{ErrorObject, Notification, Request, Response},
    protocol::{
        methods, CallToolParams, CallToolResult, GetPromptParams, GetPromptResult,
        Implementation, InitializeParams, InitializeResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult, PromptsCapability, ReadResourceParams, ReadResourceResult,
        ResourcesCapability, ServerCapabilities, ToolsCapability, PROTOCOL_VERSION,
    },
    Error, Id, Message, Result, ToolContent,
};
use mcp_transport::Transport;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::registry::{
    ExposurePolicy, PromptDescriptor, PromptHandler, ResourceDescriptor, ResourceHandler,
    ToolDescriptor, ToolHandler,
};

/// Builder for an MCP server.
pub struct ServerBuilder {
    info: Implementation,
    instructions: Option<String>,
    tools: HashMap<String, ToolDescriptor>,
    resources: HashMap<String, ResourceDescriptor>,
    prompts: HashMap<String, PromptDescriptor>,
    exposure: ExposurePolicy,
}

impl ServerBuilder {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            info: Implementation { name: name.into(), version: version.into() },
            instructions: None,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            exposure: ExposurePolicy::ReadOnlyOnly,
        }
    }

    /// Set the tool exposure policy.  Defaults to `ReadOnlyOnly` so write
    /// tools registered via `with_writes()` are hidden until the operator
    /// opts in.
    pub fn exposure(mut self, policy: ExposurePolicy) -> Self {
        self.exposure = policy;
        self
    }

    pub fn instructions(mut self, text: impl Into<String>) -> Self {
        self.instructions = Some(text.into());
        self
    }

    pub fn tool(mut self, descriptor: ToolDescriptor) -> Self {
        self.tools.insert(descriptor.tool.name.clone(), descriptor);
        self
    }

    pub fn tool_fn<F, Fut>(
        self,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: mcp_core::ToolInputSchema,
        handler: F,
    ) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<CallToolResult>> + Send + 'static,
    {
        let descriptor = ToolDescriptor::new(
            name,
            Some(description.into()),
            input_schema,
            Arc::new(crate::registry::ToolFn(handler)),
        );
        self.tool(descriptor)
    }

    pub fn resource(mut self, descriptor: ResourceDescriptor) -> Self {
        self.resources.insert(descriptor.resource.uri.clone(), descriptor);
        self
    }

    pub fn prompt(mut self, descriptor: PromptDescriptor) -> Self {
        self.prompts.insert(descriptor.prompt.name.clone(), descriptor);
        self
    }

    pub fn build(self) -> Server {
        // Filter tools by exposure policy at build time so list_tools and
        // call_tool both see the same surface — preventing the agent from
        // discovering a tool that would then be refused.
        let policy = self.exposure;
        let allowed_tools: HashMap<String, ToolDescriptor> = self.tools.into_iter()
            .filter(|(_, d)| d.is_allowed_by(policy))
            .collect();
        Server {
            context: Arc::new(ServerContext {
                info: self.info,
                instructions: self.instructions,
                tools: allowed_tools,
                resources: self.resources,
                prompts: self.prompts,
                exposure: policy,
            }),
        }
    }
}

/// Immutable, shared server state.
pub struct ServerContext {
    pub info: Implementation,
    pub instructions: Option<String>,
    pub tools: HashMap<String, ToolDescriptor>,
    pub resources: HashMap<String, ResourceDescriptor>,
    pub prompts: HashMap<String, PromptDescriptor>,
    pub exposure: ExposurePolicy,
}

impl ServerContext {
    fn capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
            tools: (!self.tools.is_empty()).then_some(ToolsCapability { list_changed: false }),
            resources: (!self.resources.is_empty())
                .then_some(ResourcesCapability { list_changed: false, subscribe: false }),
            prompts: (!self.prompts.is_empty()).then_some(PromptsCapability { list_changed: false }),
            logging: None,
            extra: Default::default(),
        }
    }
}

#[derive(Clone)]
pub struct Server {
    context: Arc<ServerContext>,
}

impl Server {
    pub fn builder(name: impl Into<String>, version: impl Into<String>) -> ServerBuilder {
        ServerBuilder::new(name, version)
    }

    pub fn context(&self) -> &Arc<ServerContext> { &self.context }

    /// Dispatch a single message and return the response (if any).
    /// Notifications and responses return `None`; requests return `Some`.
    /// Used by transports that own message buffering (one-shot HTTP).
    /// No elicitation support — see `dispatch_message_with` for that.
    pub async fn dispatch_message(&self, message: Message) -> Option<Message> {
        self.dispatch_message_with(message, crate::elicit::ElicitationHandle::disabled()).await
    }

    /// Dispatch with an explicit `ElicitationHandle`.  Tools that call
    /// `mcp_server::elicit(...)` will route their request through this
    /// handle.  HTTP transports that have an SSE side-channel can provide
    /// a real handle; one-shot transports use `disabled()`.
    pub async fn dispatch_message_with(
        &self,
        message: Message,
        elicit: crate::elicit::ElicitationHandle,
    ) -> Option<Message> {
        match message {
            Message::Request(req) => {
                let ctx = crate::elicit::ToolContext { elicit };
                let dispatch = self.dispatch(req);
                let response = crate::elicit::TOOL_CONTEXT.scope(ctx, dispatch).await;
                Some(Message::Response(response))
            }
            Message::Notification(n) => {
                self.on_notification(n);
                None
            }
            Message::Response(_) => None,
        }
    }

    /// Drive the MCP dispatch loop over any `Transport`.  Use this for
    /// generic transports that don't expose split read/write halves;
    /// elicitation is **disabled** because mid-call server-initiated
    /// requests can't be safely interleaved with the single-actor
    /// recv/send pattern (proven by load testing).
    ///
    /// For stdio — and any other transport that exposes `into_parts()`
    /// — prefer `run_stdio()`, which runs reader and writer on separate
    /// tasks and is fully elicitation-safe.
    pub async fn run<T: Transport>(&self, transport: T) -> Result<()> {
        self.run_single_actor(transport).await
    }

    async fn run_single_actor<T: Transport>(&self, mut transport: T) -> Result<()> {
        info!(
            server = %self.context.info.name,
            elicitation = "disabled",
            "MCP server starting (single-actor; elicitation NOT supported on this transport)"
        );
        // Generic transports without a split: serial recv/send.  Tools
        // that need elicitation should be invoked via a transport that
        // implements `into_parts` (stdio does).
        let elicit = crate::elicit::ElicitationHandle::disabled();
        while let Some(message) = transport.recv().await? {
            let response = self.dispatch_message_with(message, elicit.clone()).await;
            if let Some(out) = response {
                transport.send(out).await?;
            }
        }
        info!("MCP server stopping (transport EOF)");
        Ok(())
    }

    /// Drive the dispatch loop over an explicit pair of stdio halves.
    /// Use this whenever elicitation is required (the reader/writer split
    /// is what makes mid-tool server-initiated requests safe).
    pub async fn run_stdio<R, W>(
        &self,
        mut reader: mcp_transport::stdio::StdioReader<R>,
        mut writer: mcp_transport::stdio::StdioWriter<W>,
    ) -> Result<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        use tokio::sync::mpsc;

        info!(server = %self.context.info.name, "MCP server starting (split reader/writer)");

        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Message>(64);
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<Message>(64);
        let elicit_handle = crate::elicit::ElicitationHandle::new(
            outbound_tx.clone(),
            true,
        );

        let reader_task = tokio::spawn(async move {
            loop {
                match reader.recv().await {
                    Ok(Some(msg)) => {
                        if inbound_tx.send(msg).await.is_err() { break; }
                    }
                    Ok(None) => break,
                    Err(e) => { warn!(error = %e, "transport recv failed"); break; }
                }
            }
        });

        let writer_task = tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                if let Err(e) = writer.send(msg).await {
                    warn!(error = %e, "transport send failed");
                    break;
                }
            }
            writer.close().await;
        });

        let server = self.clone();
        while let Some(msg) = inbound_rx.recv().await {
            match msg {
                Message::Request(req) => {
                    let server = server.clone();
                    let elicit = elicit_handle.clone();
                    let out = outbound_tx.clone();
                    tokio::spawn(async move {
                        let ctx = crate::elicit::ToolContext { elicit };
                        let response = crate::elicit::TOOL_CONTEXT
                            .scope(ctx, server.dispatch(req))
                            .await;
                        let _ = out.send(Message::Response(response)).await;
                    });
                }
                Message::Notification(n) => server.on_notification(n),
                Message::Response(resp) => {
                    let claimed = elicit_handle.deliver_response(resp.clone()).await;
                    if !claimed {
                        debug!("server received response with no waiter id={:?}", resp.id);
                    }
                }
            }
        }
        drop(outbound_tx);
        let _ = reader_task.await;
        let _ = writer_task.await;
        info!("MCP server stopping");
        Ok(())
    }

    fn on_notification(&self, n: Notification) {
        debug!(method = %n.method, "notification received");
    }

    async fn dispatch(&self, req: Request) -> Response {
        let id = req.id.clone();
        match self.handle_method(&req).await {
            Ok(result) => Response::success(id, result),
            Err(e) => {
                warn!(method = %req.method, error = %e, "request failed");
                Response::failure(id, error_object(e))
            }
        }
    }

    async fn handle_method(&self, req: &Request) -> Result<Value> {
        match req.method.as_str() {
            methods::INITIALIZE => {
                let params: InitializeParams = parse_params(req.params.clone())?;
                let result = InitializeResult {
                    protocol_version: select_protocol_version(&params.protocol_version),
                    capabilities: self.context.capabilities(),
                    server_info: self.context.info.clone(),
                    instructions: self.context.instructions.clone(),
                };
                Ok(serde_json::to_value(result)?)
            }
            methods::PING => Ok(serde_json::json!({})),
            methods::TOOLS_LIST => {
                let result = ListToolsResult {
                    tools: self.context.tools.values().map(|d| d.tool.clone()).collect(),
                    next_cursor: None,
                };
                Ok(serde_json::to_value(result)?)
            }
            methods::TOOLS_CALL => {
                let params: CallToolParams = parse_params(req.params.clone())?;
                let descriptor = self
                    .context
                    .tools
                    .get(&params.name)
                    .ok_or_else(|| Error::protocol(ErrorCode::UnknownTool, format!("unknown tool '{}'", params.name)))?;
                let args = params.arguments.unwrap_or(Value::Object(Default::default()));
                let result = descriptor.handler.call(args).await.unwrap_or_else(|e| {
                    CallToolResult {
                        content: vec![ToolContent::text(e.to_string())],
                        is_error: true,
                    }
                });
                Ok(serde_json::to_value(result)?)
            }
            methods::RESOURCES_LIST => {
                let result = ListResourcesResult {
                    resources: self.context.resources.values().map(|d| d.resource.clone()).collect(),
                    next_cursor: None,
                };
                Ok(serde_json::to_value(result)?)
            }
            methods::RESOURCES_READ => {
                let params: ReadResourceParams = parse_params(req.params.clone())?;
                let descriptor = self
                    .context
                    .resources
                    .get(&params.uri)
                    .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, format!("unknown resource '{}'", params.uri)))?;
                let result: ReadResourceResult = descriptor.handler.read(&params.uri).await?;
                Ok(serde_json::to_value(result)?)
            }
            methods::PROMPTS_LIST => {
                let result = ListPromptsResult {
                    prompts: self.context.prompts.values().map(|d| d.prompt.clone()).collect(),
                    next_cursor: None,
                };
                Ok(serde_json::to_value(result)?)
            }
            methods::PROMPTS_GET => {
                let params: GetPromptParams = parse_params(req.params.clone())?;
                let descriptor = self
                    .context
                    .prompts
                    .get(&params.name)
                    .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, format!("unknown prompt '{}'", params.name)))?;
                let result: GetPromptResult = descriptor.handler.get(params.arguments).await?;
                Ok(serde_json::to_value(result)?)
            }
            other => Err(Error::protocol(ErrorCode::MethodNotFound, format!("method '{}' not supported", other))),
        }
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Option<Value>) -> Result<T> {
    let value = params.unwrap_or(Value::Object(Default::default()));
    serde_json::from_value(value).map_err(|e| {
        Error::protocol(ErrorCode::InvalidParams, format!("invalid params: {e}"))
    })
}

fn select_protocol_version(client: &str) -> String {
    // If the client speaks our wire version, honour it; otherwise fall back to
    // our supported version and let the client decide whether to proceed.
    if client == PROTOCOL_VERSION {
        client.to_string()
    } else {
        PROTOCOL_VERSION.to_string()
    }
}

fn error_object(e: Error) -> ErrorObject {
    match e {
        Error::Protocol { code, message } => ErrorObject::new(code, message),
        Error::Json(je) => ErrorObject::new(ErrorCode::ParseError.as_i32(), je.to_string()),
        Error::Io(io) => ErrorObject::new(ErrorCode::InternalError.as_i32(), io.to_string()),
        other => ErrorObject::new(ErrorCode::InternalError.as_i32(), other.to_string()),
    }
}

// Silence unused-import warnings for types used in trait bounds only.
#[allow(dead_code)]
fn _trait_objects(_t: &dyn ToolHandler, _r: &dyn ResourceHandler, _p: &dyn PromptHandler) {}

#[allow(unused_imports)]
use Id as _Id; // keep Id in scope for future shutdown handling
