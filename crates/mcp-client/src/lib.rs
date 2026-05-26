//! MCP client.
//!
//! A single I/O task owns the transport and multiplexes outbound messages
//! (from an mpsc outbox) with inbound messages (via `Transport::recv`).
//! Inbound responses are matched to their pending oneshot senders by id;
//! notifications and server-initiated requests are logged (Phase 1).

use async_trait::async_trait;
use mcp_core::{
    error::ErrorCode,
    jsonrpc::{ErrorObject, Message, Request, Response},
    protocol::{
        methods, CallToolParams, CallToolResult, ClientCapabilities, ElicitationAction,
        ElicitationParams, ElicitationResult, Implementation, InitializeParams,
        InitializeResult, ListPromptsResult, ListResourcesResult, ListToolsResult,
        PROTOCOL_VERSION,
    },
    Error, Id, Result,
};
use mcp_transport::Transport;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Response>>>>;

/// Implement this trait to handle server-initiated `elicitation/create`
/// requests.  Wired via `Client::spawn_with_delegate`.
#[async_trait]
pub trait ElicitationDelegate: Send + Sync + 'static {
    async fn on_elicit(&self, params: ElicitationParams) -> ElicitationResult;
}

/// Default delegate: always declines.  Safe for non-interactive use.
pub struct DeclineAll;

#[async_trait]
impl ElicitationDelegate for DeclineAll {
    async fn on_elicit(&self, _params: ElicitationParams) -> ElicitationResult {
        ElicitationResult { action: ElicitationAction::Decline, content: None }
    }
}

/// Asynchronous MCP client.
pub struct Client {
    next_id: AtomicI64,
    pending: Pending,
    outbox: mpsc::Sender<Message>,
    _io_task: JoinHandle<()>,
    server_info: Mutex<Option<InitializeResult>>,
}

impl Client {
    /// Spawn the I/O task with the default elicitation delegate (declines).
    /// Equivalent to `spawn_with_delegate(transport, Arc::new(DeclineAll))`.
    pub fn spawn<T: Transport>(transport: T) -> Arc<Self> {
        Self::spawn_with_delegate(transport, Arc::new(DeclineAll))
    }

    /// Spawn a client over a `StdioTransport` after splitting it into
    /// independent read/write halves.  Required for any usage involving
    /// server-initiated requests (elicitation) — without the split, the
    /// I/O loop is not cancellation-safe.
    pub fn spawn_stdio<R, W>(
        transport: mcp_transport::stdio::StdioTransport<R, W>,
        delegate: Arc<dyn ElicitationDelegate>,
    ) -> Arc<Self>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = transport.into_parts();
        Self::spawn_with_halves(reader, writer, delegate)
    }

    /// Spawn from explicit reader/writer halves.  Internal helper —
    /// callers typically use `spawn_stdio`.
    pub fn spawn_with_halves<R, W>(
        mut reader: mcp_transport::stdio::StdioReader<R>,
        mut writer: mcp_transport::stdio::StdioWriter<W>,
        delegate: Arc<dyn ElicitationDelegate>,
    ) -> Arc<Self>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (outbox_tx, mut outbox_rx) = mpsc::channel::<Message>(64);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_io = Arc::clone(&pending);
        let outbox_inbound = outbox_tx.clone();

        // Reader task: always-blocking recv, never cancelled.
        let reader_task = tokio::spawn(async move {
            loop {
                match reader.recv().await {
                    Ok(Some(Message::Response(r))) => {
                        if let Id::Number(n) = r.id {
                            let waiter = {
                                let mut p = pending_io.lock().await;
                                p.remove(&n)
                            };
                            if let Some(w) = waiter { let _ = w.send(r); }
                            else { debug!(id = n, "response with no waiter"); }
                        } else {
                            warn!("client: response with non-numeric id");
                        }
                    }
                    Ok(Some(Message::Notification(n))) => {
                        debug!(method = %n.method, "notification");
                    }
                    Ok(Some(Message::Request(req))) => {
                        let delegate = Arc::clone(&delegate);
                        let outbox = outbox_inbound.clone();
                        tokio::spawn(async move {
                            handle_server_request(req, delegate, outbox).await;
                        });
                    }
                    Ok(None) => {
                        debug!("transport EOF; client shutting down");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "client reader: recv failed");
                        break;
                    }
                }
            }
            let mut p = pending_io.lock().await;
            p.clear();
        });

        // Writer task: drains outbox.
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = outbox_rx.recv().await {
                if let Err(e) = writer.send(msg).await {
                    warn!(error = %e, "client writer: send failed");
                    break;
                }
            }
        });

        // Combine into one join handle so the Drop semantics stay the same.
        let io_task = tokio::spawn(async move {
            let _ = reader_task.await;
            let _ = writer_task.await;
        });

        Arc::new(Self {
            next_id: AtomicI64::new(1),
            pending,
            outbox: outbox_tx,
            _io_task: io_task,
            server_info: Mutex::new(None),
        })
    }

    /// Legacy: spawn with a generic transport (single-task; not safe for
    /// elicitation under load).  Kept for backward compatibility.
    pub fn spawn_with_delegate<T: Transport>(
        mut transport: T,
        delegate: Arc<dyn ElicitationDelegate>,
    ) -> Arc<Self> {
        let (outbox_tx, mut outbox_rx) = mpsc::channel::<Message>(64);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_io = Arc::clone(&pending);
        let outbox_inbound = outbox_tx.clone();

        // Single I/O task owns the transport.  Multiplexes outbound (from the
        // mpsc outbox) and inbound (from `transport.recv`) using `select!`.
        let io_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    msg = outbox_rx.recv() => {
                        match msg {
                            Some(m) => {
                                if let Err(e) = transport.send(m).await {
                                    warn!(error = %e, "client I/O: send failed");
                                    break;
                                }
                            }
                            None => {
                                debug!("outbox closed; client shutting down");
                                break;
                            }
                        }
                    }

                    incoming = transport.recv() => {
                        match incoming {
                            Ok(Some(Message::Response(r))) => {
                                let id_num = match r.id {
                                    Id::Number(n) => Some(n),
                                    _ => None,
                                };
                                if let Some(n) = id_num {
                                    let waiter = {
                                        let mut p = pending_io.lock().await;
                                        p.remove(&n)
                                    };
                                    if let Some(w) = waiter {
                                        let _ = w.send(r);
                                    } else {
                                        debug!(id = n, "response with no waiter");
                                    }
                                } else {
                                    warn!("client I/O: response with non-numeric id");
                                }
                            }
                            Ok(Some(Message::Notification(n))) => {
                                debug!(method = %n.method, "notification");
                            }
                            Ok(Some(Message::Request(req))) => {
                                let delegate = Arc::clone(&delegate);
                                let outbox = outbox_inbound.clone();
                                tokio::spawn(async move {
                                    handle_server_request(req, delegate, outbox).await;
                                });
                            }
                            Ok(None) => {
                                debug!("transport EOF; client shutting down");
                                break;
                            }
                            Err(e) => {
                                warn!(error = %e, "client I/O: recv failed");
                                break;
                            }
                        }
                    }
                }
            }

            // Drain pending waiters so callers see TransportClosed instead of hanging.
            let mut p = pending_io.lock().await;
            p.clear();
        });

        Arc::new(Self {
            next_id: AtomicI64::new(1),
            pending,
            outbox: outbox_tx,
            _io_task: io_task,
            server_info: Mutex::new(None),
        })
    }

    /// Perform the MCP initialise handshake with `elicitation` capability
    /// advertised by default — flip it off via `initialize_with(...)` if
    /// the delegate is the default `DeclineAll`.
    pub async fn initialize(
        self: &Arc<Self>,
        client_info: Implementation,
        capabilities: ClientCapabilities,
    ) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.into(),
            capabilities,
            client_info,
        };
        let result: InitializeResult = self.call(methods::INITIALIZE, Some(serde_json::to_value(params)?)).await?;
        self.notify(methods::INITIALIZED, None).await?;
        *self.server_info.lock().await = Some(result.clone());
        Ok(result)
    }

    pub async fn server_info(&self) -> Option<InitializeResult> {
        self.server_info.lock().await.clone()
    }

    pub async fn list_tools(&self) -> Result<ListToolsResult> {
        self.call(methods::TOOLS_LIST, None).await
    }

    pub async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let params = CallToolParams { name: name.into(), arguments };
        self.call(methods::TOOLS_CALL, Some(serde_json::to_value(params)?)).await
    }

    pub async fn list_resources(&self) -> Result<ListResourcesResult> {
        self.call(methods::RESOURCES_LIST, None).await
    }

    pub async fn read_resource(&self, uri: &str) -> Result<mcp_core::ReadResourceResult> {
        let params = mcp_core::ReadResourceParams { uri: uri.into() };
        self.call(methods::RESOURCES_READ, Some(serde_json::to_value(params)?)).await
    }

    pub async fn list_prompts(&self) -> Result<ListPromptsResult> {
        self.call(methods::PROMPTS_LIST, None).await
    }

    pub async fn get_prompt(&self, name: &str, arguments: Option<Value>) -> Result<mcp_core::protocol::GetPromptResult> {
        let params = mcp_core::protocol::GetPromptParams { name: name.into(), arguments };
        self.call(methods::PROMPTS_GET, Some(serde_json::to_value(params)?)).await
    }

    /// Set the minimum log severity the server will emit via
    /// `notifications/message`.  MCP 2025-06-18 logging utility.
    pub async fn set_log_level(&self, level: mcp_core::LogLevel) -> Result<()> {
        let params = mcp_core::SetLevelParams { level };
        let _: Value = self.call(methods::LOGGING_SET_LEVEL, Some(serde_json::to_value(params)?)).await?;
        Ok(())
    }

    /// Autocomplete a prompt argument value.  MCP 2025-06-18 completion
    /// utility.  Returns `(values, has_more)` — empty list is a
    /// spec-compliant "no suggestions" response.
    pub async fn complete_prompt_argument(
        &self,
        prompt_name: &str,
        argument_name: &str,
        typed_value: &str,
    ) -> Result<mcp_core::CompleteResult> {
        let params = mcp_core::CompleteParams {
            reference: mcp_core::CompletionRef::Prompt { name: prompt_name.into() },
            argument: mcp_core::CompletionArgumentRef {
                name: argument_name.into(),
                value: typed_value.into(),
            },
        };
        self.call(methods::COMPLETION_COMPLETE, Some(serde_json::to_value(params)?)).await
    }

    /// Issue an arbitrary JSON-RPC call.  Useful for raw spec exercises
    /// in tests and for forwards-compat methods not yet wrapped by a
    /// typed helper.
    pub async fn raw_request<R: DeserializeOwned>(&self, method: &str, params: Option<Value>) -> Result<R> {
        self.call(method, params).await
    }

    pub async fn ping(&self) -> Result<()> {
        let _: Value = self.call(methods::PING, None).await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let n = mcp_core::Notification::new(method, params);
        self.outbox
            .send(Message::Notification(n))
            .await
            .map_err(|_| Error::TransportClosed)?;
        Ok(())
    }

    async fn call<R: DeserializeOwned>(&self, method: &str, params: Option<Value>) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.pending.lock().await;
            p.insert(id, tx);
        }
        let req = Request::new(Id::Number(id), method, params);
        self.outbox
            .send(Message::Request(req))
            .await
            .map_err(|_| Error::TransportClosed)?;

        let response = rx.await.map_err(|_| Error::TransportClosed)?;
        if let Some(err) = response.error {
            return Err(Error::Protocol { code: err.code, message: err.message });
        }
        let result_value = response.result.ok_or_else(|| {
            Error::protocol(ErrorCode::InvalidRequest, "response missing result")
        })?;
        let parsed: R = serde_json::from_value(result_value)?;
        Ok(parsed)
    }
}

/// Dispatch a server-initiated request to the registered delegate and
/// send the response back.  Only `elicitation/create` is supported in
/// Phase 6; everything else replies with MethodNotFound.
async fn handle_server_request(
    req: Request,
    delegate: Arc<dyn ElicitationDelegate>,
    outbox: mpsc::Sender<Message>,
) {
    let id = req.id.clone();
    let response = match req.method.as_str() {
        methods::ELICITATION_CREATE => {
            let params: ElicitationParams = match req.params
                .and_then(|p| serde_json::from_value(p).ok())
            {
                Some(p) => p,
                None => {
                    let err = ErrorObject::new(ErrorCode::InvalidParams.as_i32(), "invalid elicitation params");
                    return drop(outbox.send(Message::Response(Response::failure(id, err))).await);
                }
            };
            let result = delegate.on_elicit(params).await;
            match serde_json::to_value(result) {
                Ok(v) => Response::success(id, v),
                Err(e) => Response::failure(id, ErrorObject::new(
                    ErrorCode::InternalError.as_i32(), e.to_string()
                )),
            }
        }
        other => {
            warn!(method = %other, "server-initiated request: method not supported by this client");
            Response::failure(id, ErrorObject::new(
                ErrorCode::MethodNotFound.as_i32(),
                format!("method '{other}' not supported by client"),
            ))
        }
    };
    let _ = outbox.send(Message::Response(response)).await;
}
