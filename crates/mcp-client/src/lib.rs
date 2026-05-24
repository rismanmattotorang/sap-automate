//! MCP client.
//!
//! A single I/O task owns the transport and multiplexes outbound messages
//! (from an mpsc outbox) with inbound messages (via `Transport::recv`).
//! Inbound responses are matched to their pending oneshot senders by id;
//! notifications and server-initiated requests are logged (Phase 1).

use mcp_core::{
    error::ErrorCode,
    jsonrpc::{Message, Request, Response},
    protocol::{
        methods, CallToolParams, CallToolResult, ClientCapabilities, Implementation,
        InitializeParams, InitializeResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult, PROTOCOL_VERSION,
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

/// Asynchronous MCP client.
pub struct Client {
    next_id: AtomicI64,
    pending: Pending,
    outbox: mpsc::Sender<Message>,
    _io_task: JoinHandle<()>,
    server_info: Mutex<Option<InitializeResult>>,
}

impl Client {
    /// Spawn the I/O task and return a client handle.  Does not perform the
    /// MCP initialise handshake — call `initialize` next.
    pub fn spawn<T: Transport>(mut transport: T) -> Arc<Self> {
        let (outbox_tx, mut outbox_rx) = mpsc::channel::<Message>(64);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_io = Arc::clone(&pending);

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
                                debug!(method = %req.method, "server-initiated request (ignored in Phase 1)");
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

    /// Perform the MCP initialise handshake.
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
