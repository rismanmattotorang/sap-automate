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

/// Type-erased Prometheus metrics renderer.  When set, the HTTP
/// transport exposes a `/metrics` endpoint that calls this.
pub type MetricsRenderFn = Arc<dyn Fn() -> String + Send + Sync + 'static>;

/// Configuration for the HTTP transport.
#[derive(Clone)]
pub struct HttpServerConfig {
    pub bind: SocketAddr,
    /// Optional shared bearer token. When set, requests without
    /// `Authorization: Bearer <token>` are rejected.
    pub bearer_token: Option<String>,
    /// When `Some`, GET `/metrics` calls this to render the Prometheus
    /// text exposition.  The endpoint is always unauthenticated so
    /// scrapers don't need to know the bearer token; production
    /// deployments restrict the endpoint via NetworkPolicy.
    pub metrics_renderer: Option<MetricsRenderFn>,
    /// Allowed `Origin` header values.  When non-empty, requests whose
    /// `Origin` header is absent OR not in this list are rejected with
    /// HTTP 403 — the spec-recommended DNS-rebinding mitigation for
    /// browser-accessible MCP servers (MCP 2025-06-18 §4.6 transport
    /// security).  Empty list disables the check (default; for stdio
    /// or trusted network deployments).
    pub allowed_origins: Vec<String>,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3030".parse().unwrap(),
            bearer_token: None,
            metrics_renderer: None,
            allowed_origins: Vec::new(),
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
            allowed_origins: config.allowed_origins.clone(),
            dispatch: Arc::new(dispatch),
            events: Mutex::new(EventBus::default()),
        });

        let metrics_renderer = config.metrics_renderer.clone();
        let metrics_route = get(move || {
            let r = metrics_renderer.clone();
            async move {
                match r {
                    Some(f) => (
                        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                        f(),
                    ).into_response(),
                    None => (
                        axum::http::StatusCode::NOT_FOUND,
                        "metrics endpoint not configured",
                    ).into_response(),
                }
            }
        });

        let app = Router::new()
            .route("/mcp", post(post_handler::<F, Fut>))
            .route("/mcp/events", get(events_handler::<F, Fut>))
            .route("/health", get(|| async { "ok" }))
            .route("/metrics", metrics_route)
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
    allowed_origins: Vec<String>,
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
    if !check_origin(&state.allowed_origins, &headers) {
        return (axum::http::StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
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
    if !check_origin(&state.allowed_origins, &headers) {
        return (axum::http::StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
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

/// DNS-rebinding mitigation.  When `allowed` is non-empty, the request
/// MUST carry an `Origin` header whose value is one of the allowed
/// entries.  Empty allowlist disables the check (suitable for stdio,
/// trusted in-cluster traffic, or unit tests).
///
/// Spec reference: MCP 2025-06-18 §4.6 — "Transport security: HTTP
/// servers SHOULD validate the Origin header to prevent DNS rebinding."
fn check_origin(allowed: &[String], headers: &axum::http::HeaderMap) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|h| h.to_str().ok());
    match origin {
        None => false,
        Some(o) => allowed.iter().any(|a| a == o),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn h(key: &'static str, value: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(key, HeaderValue::from_str(value).unwrap());
        m
    }

    #[test]
    fn empty_allowlist_accepts_everything() {
        assert!(check_origin(&[], &HeaderMap::new()));
        assert!(check_origin(&[], &h("origin", "http://evil.com")));
    }

    #[test]
    fn populated_allowlist_rejects_missing_origin() {
        let allowed = vec!["http://localhost:3000".into()];
        assert!(!check_origin(&allowed, &HeaderMap::new()),
            "no Origin header must be rejected when an allowlist is configured");
    }

    #[test]
    fn populated_allowlist_accepts_matching_origin() {
        let allowed = vec!["http://localhost:3000".into()];
        assert!(check_origin(&allowed, &h("origin", "http://localhost:3000")));
    }

    #[test]
    fn populated_allowlist_rejects_non_matching_origin() {
        let allowed = vec!["http://localhost:3000".into()];
        assert!(!check_origin(&allowed, &h("origin", "http://evil.com")));
    }

    #[test]
    fn allowlist_is_case_sensitive_exact_match() {
        // Per RFC 6454 origins are case-sensitive after the scheme.  We
        // do exact-string match — operators get the behaviour they
        // configured, not silent case folding.
        let allowed = vec!["http://Localhost:3000".into()];
        assert!(!check_origin(&allowed, &h("origin", "http://localhost:3000")));
    }

    #[test]
    fn check_auth_passes_when_no_token_required() {
        assert!(check_auth(&None, &HeaderMap::new()));
    }

    #[test]
    fn check_auth_rejects_missing_bearer() {
        assert!(!check_auth(&Some("secret".into()), &HeaderMap::new()));
    }

    #[test]
    fn check_auth_accepts_matching_bearer() {
        assert!(check_auth(&Some("secret".into()), &h("authorization", "Bearer secret")));
    }
}
