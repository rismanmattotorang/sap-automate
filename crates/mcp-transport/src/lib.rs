//! MCP transports.
//!
//! Phase 1 delivers stdio (line-delimited JSON).  HTTP+SSE and Streaming HTTP
//! follow in Phase 1 finalisation; their module stubs document the contracts
//! they will satisfy.

use async_trait::async_trait;
use mcp_core::{Message, Result};

pub mod stdio;
#[cfg(feature = "http")]
pub mod http;

pub use stdio::StdioTransport;
#[cfg(feature = "http")]
pub use http::{HttpServerTransport, HttpServerHandle, HttpServerConfig};

/// Bidirectional, framed MCP transport.
///
/// The trait is intentionally minimal: callers send and receive whole
/// `mcp_core::Message` values.  Concrete implementations own the wire framing
/// (line-delimited JSON for stdio; SSE events for HTTP+SSE; chunked JSON-RPC
/// over a single HTTP stream for Streaming HTTP).
#[async_trait]
pub trait Transport: Send + 'static {
    /// Send a single message.  Returns when the message has been flushed onto
    /// the underlying byte stream.
    async fn send(&mut self, message: Message) -> Result<()>;

    /// Receive a single message.  Returns `Ok(None)` on clean EOF, an error
    /// on transport failure.
    async fn recv(&mut self) -> Result<Option<Message>>;

    /// Close the transport.  Idempotent.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}
