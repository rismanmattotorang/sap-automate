//! Server-side elicitation support (paper §II-B, MCP 2025-06-18).
//!
//! Tools running inside the server can pause and request structured input
//! from the user via `ToolContext::elicit()`.  The handle is exposed as a
//! `tokio::task_local!` so existing `ToolFn`-shaped handlers don't need a
//! signature change.
//!
//! Wire flow:
//!   1. Tool calls `ctx.elicit(message, schema).await`.
//!   2. ElicitationHandle allocates a fresh server-side request id,
//!      registers a oneshot waiter under that id, and pushes an
//!      `elicitation/create` request onto the outbound channel.
//!   3. The server's run loop reads the client's `Response` to that id
//!      and routes it to the waiter.
//!   4. The tool wakes up, deserialises the result, and continues.
//!
//! Clients that do not advertise the `elicitation` capability cause the
//! handle to short-circuit with `ElicitationResult { action: Decline }`,
//! so tools can be written assuming elicitation is available — they just
//! degrade to safe defaults on legacy clients.

use mcp_core::{
    jsonrpc::{Id, Message, Request, Response},
    protocol::{methods, ElicitationAction, ElicitationParams, ElicitationResult},
    Error, Result as CoreResult,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, warn};

tokio::task_local! {
    /// Per-tool-invocation context.  Tools can read this to access the
    /// elicitation handle and any other future per-request state.
    pub static TOOL_CONTEXT: ToolContext;
}

/// Cheap to clone; contains an outbound channel and a pending-waiters map.
#[derive(Clone)]
pub struct ToolContext {
    pub elicit: ElicitationHandle,
}

#[derive(Clone)]
pub struct ElicitationHandle {
    inner: Arc<ElicitationInner>,
}

struct ElicitationInner {
    outbound: mpsc::Sender<Message>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Response>>>,
    next_id: AtomicI64,
    /// If false, every `elicit` call short-circuits with Decline.
    enabled: bool,
}

impl ElicitationHandle {
    pub fn new(outbound: mpsc::Sender<Message>, enabled: bool) -> Self {
        Self {
            inner: Arc::new(ElicitationInner {
                outbound,
                pending: Mutex::new(HashMap::new()),
                // Use a high base so server-initiated ids don't collide
                // with client-initiated ones.
                next_id: AtomicI64::new(1_000_000),
                enabled,
            }),
        }
    }

    /// A no-op handle for transports that can't carry server-initiated
    /// requests (one-shot HTTP, etc.).  Always returns Decline.
    pub fn disabled() -> Self {
        let (tx, _rx) = mpsc::channel::<Message>(1);
        Self::new(tx, false)
    }

    /// Returns true if the connected client advertised elicitation
    /// capability and the transport can carry server-initiated requests.
    pub fn is_enabled(&self) -> bool { self.inner.enabled }

    /// Send an elicitation request and await the user's response.
    /// On any failure (transport closed, client declined, malformed
    /// response), returns an `ElicitationResult` describing the outcome
    /// — the caller decides whether to abort or continue with a safe
    /// default.
    pub async fn elicit(&self, message: &str, requested_schema: Value) -> ElicitationResult {
        if !self.inner.enabled {
            debug!("elicitation requested but disabled; returning Decline");
            return ElicitationResult { action: ElicitationAction::Decline, content: None };
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.inner.pending.lock().await;
            p.insert(id, tx);
        }
        let params = ElicitationParams {
            message: message.into(),
            requested_schema,
        };
        let params_value = match serde_json::to_value(&params) {
            Ok(v) => v,
            Err(e) => {
                warn!("elicitation: failed to serialise params: {e}");
                return ElicitationResult { action: ElicitationAction::Cancel, content: None };
            }
        };
        let req = Request::new(Id::Number(id), methods::ELICITATION_CREATE, Some(params_value));
        if self.inner.outbound.send(Message::Request(req)).await.is_err() {
            warn!("elicitation: outbound channel closed");
            // Drop the waiter, return Cancel.
            self.inner.pending.lock().await.remove(&id);
            return ElicitationResult { action: ElicitationAction::Cancel, content: None };
        }

        match rx.await {
            Ok(resp) => Self::parse_response(resp),
            Err(_) => {
                debug!("elicitation: waiter dropped");
                ElicitationResult { action: ElicitationAction::Cancel, content: None }
            }
        }
    }

    /// Route an incoming Response to the pending waiter, if any.
    /// Returns true if it claimed the response.
    pub async fn deliver_response(&self, resp: Response) -> bool {
        let id_num = match resp.id {
            Id::Number(n) => n,
            _ => return false,
        };
        let waiter = {
            let mut p = self.inner.pending.lock().await;
            p.remove(&id_num)
        };
        if let Some(w) = waiter {
            let _ = w.send(resp);
            true
        } else {
            false
        }
    }

    fn parse_response(resp: Response) -> ElicitationResult {
        if resp.error.is_some() {
            return ElicitationResult { action: ElicitationAction::Cancel, content: None };
        }
        let Some(value) = resp.result else {
            return ElicitationResult { action: ElicitationAction::Cancel, content: None };
        };
        serde_json::from_value::<ElicitationResult>(value).unwrap_or_else(|_| {
            warn!("elicitation: client returned malformed result; treating as Cancel");
            ElicitationResult { action: ElicitationAction::Cancel, content: None }
        })
    }
}

/// Read the current task's `ToolContext`, if any.  Returns `None` when
/// called outside of a `TOOL_CONTEXT.scope(...)` block (e.g. directly
/// from a unit test).
pub fn current_context() -> Option<ToolContext> {
    TOOL_CONTEXT.try_with(|c| c.clone()).ok()
}

/// Convenience wrapper used inside tool handlers:
/// `elicit(msg, schema).await` — returns the result or falls through to
/// the disabled-handle behaviour if the surrounding scope didn't set a
/// context.
pub async fn elicit(message: &str, requested_schema: Value) -> ElicitationResult {
    match current_context() {
        Some(ctx) => ctx.elicit.elicit(message, requested_schema).await,
        None => ElicitationResult { action: ElicitationAction::Decline, content: None },
    }
}

/// Helper: build a `requested_schema` for the typical SAP confirmation
/// flow — a single object with a small set of typed properties.
pub fn object_schema(properties: serde_json::Map<String, Value>, required: Vec<String>) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

#[allow(dead_code)]
fn _silence(_: CoreResult<()>) -> CoreResult<()> {
    Err(Error::Other("placeholder".into()))
}
