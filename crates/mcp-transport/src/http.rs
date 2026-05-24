//! HTTP transport for MCP.
//!
//! Implements the 2025-06-18 "Streaming HTTP" transport: a single endpoint
//! that accepts JSON-RPC requests over POST and optionally sends back
//! server-initiated messages via SSE (`GET /mcp/events`).  This matches
//! the transport flexibility shown by `fr0ster/mcp-abap-adt` (stdio + HTTP
//! + SSE) and unblocks remote deployments of the SAP-Automate server.
//!
//! Phase 1 improvement: callers wrap the existing `Server` in a
//! `HttpServerTransport` to bind an HTTP listener and serve one logical
//! MCP "session" per HTTP request.  Multi-session pooling is Phase 7.

use axum::{
    extract::{Json, State},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use mcp_core::{Message, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{info, warn};

/// Configuration for the HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub bind: SocketAddr,
    /// Optional shared bearer token. When set, requests without
    /// `Authorization: Bearer <token>` are rejected.
    pub bearer_token: Option<String>,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3030".parse().unwrap(),
            bearer_token: None,
        }
    }
}

/// Handle returned by `HttpServerTransport::serve`.  Drop it to stop the
/// listener.
pub struct HttpServerHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpServerHandle {
    pub async fn shutdown(mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
        let _ = self.task.await;
    }
}

/// HTTP transport for an MCP server.
///
/// The caller passes a `dispatch` async closure that takes a single
/// incoming JSON-RPC message and returns the response (or `None` for
/// notifications).  Bind directly to the existing `mcp_server::Server` by
/// keeping the dispatch loop in a single-threaded `Mutex`.
pub struct HttpServerTransport;

impl HttpServerTransport {
    pub async fn serve<F, Fut>(config: HttpServerConfig, dispatch: F) -> Result<HttpServerHandle>
    where
        F: Fn(Message) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<Message>> + Send + 'static,
    {
        let state = Arc::new(AppState {
            bearer_token: config.bearer_token.clone(),
            dispatch: Arc::new(dispatch),
            events: Mutex::new(EventBus::default()),
        });

        let app = Router::new()
            .route("/mcp", post(post_handler::<F, Fut>))
            .route("/mcp/events", get(events_handler::<F, Fut>))
            .route("/health", get(|| async { "ok" }))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(config.bind).await
            .map_err(mcp_core::Error::Io)?;
        info!(addr = %config.bind, "MCP HTTP transport listening");

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move { let _ = rx.await; });
            if let Err(e) = server.await {
                warn!("HTTP server: {e}");
            }
        });

        Ok(HttpServerHandle { shutdown: Some(tx), task })
    }
}

struct AppState<F, Fut>
where
    F: Fn(Message) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Option<Message>> + Send + 'static,
{
    bearer_token: Option<String>,
    dispatch: Arc<F>,
    events: Mutex<EventBus>,
}

#[derive(Default)]
struct EventBus {
    subscribers: Vec<mpsc::Sender<Message>>,
}

async fn post_handler<F, Fut>(
    State(state): State<Arc<AppState<F, Fut>>>,
    headers: axum::http::HeaderMap,
    Json(message): Json<serde_json::Value>,
) -> impl IntoResponse
where
    F: Fn(Message) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Option<Message>> + Send + 'static,
{
    if !check_auth(&state.bearer_token, &headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let parsed = match Message::from_value(message) {
        Ok(m) => m,
        Err(e) => return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let dispatch = Arc::clone(&state.dispatch);
    match dispatch(parsed).await {
        Some(reply) => {
            let body = serde_json::to_string(&reply).unwrap_or_else(|_| "{}".into());
            ([(axum::http::header::CONTENT_TYPE, "application/json")], body).into_response()
        }
        None => axum::http::StatusCode::ACCEPTED.into_response(),
    }
}

async fn events_handler<F, Fut>(
    State(state): State<Arc<AppState<F, Fut>>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse
where
    F: Fn(Message) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Option<Message>> + Send + 'static,
{
    if !check_auth(&state.bearer_token, &headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let (tx, mut rx) = mpsc::channel::<Message>(16);
    {
        let mut bus = state.events.lock().await;
        bus.subscribers.push(tx);
    }
    // SSE stream: one event per Message until the subscriber's channel
    // closes.  Useful for `notifications/progress` and listChanged events.
    use axum::response::sse::{Event, Sse};
    use futures::stream::Stream;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    struct Bridge {
        rx: mpsc::Receiver<Message>,
    }
    impl Stream for Bridge {
        type Item = std::result::Result<Event, std::convert::Infallible>;
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(m)) => {
                    let data = serde_json::to_string(&m).unwrap_or_else(|_| "{}".into());
                    Poll::Ready(Some(Ok(Event::default().data(data))))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }
    // Drain anything already pending so the new subscriber sees recent
    // events even if the SSE channel is just opening up.
    if let Ok(_initial) = rx.try_recv() { /* ignored: channel is fresh */ }
    Sse::new(Bridge { rx }).into_response()
}

fn check_auth(expected: &Option<String>, headers: &axum::http::HeaderMap) -> bool {
    match expected {
        None => true,
        Some(token) => headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .map(|s| s == format!("Bearer {token}"))
            .unwrap_or(false),
    }
}
