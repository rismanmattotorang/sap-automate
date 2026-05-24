//! Stdio transport: line-delimited JSON, one message per line.

use async_trait::async_trait;
use mcp_core::{Error, Message, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::Transport;

/// Generic stdio transport over any `AsyncRead`+`AsyncWrite` pair.
///
/// For real stdio use, construct with `StdioTransport::from_stdio()`.  Tests
/// use `StdioTransport::new(reader, writer)` with `tokio::io::duplex` pipes.
pub struct StdioTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
    line: String,
    closed: bool,
}

impl<R, W> StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            line: String::with_capacity(4096),
            closed: false,
        }
    }

    /// Split into independent reader and writer halves so a server can
    /// run them on separate tasks (avoids cancelling a partial `read_line`
    /// inside a `tokio::select!`).
    pub fn into_parts(self) -> (StdioReader<R>, StdioWriter<W>) {
        (
            StdioReader { reader: self.reader, line: self.line },
            StdioWriter { writer: self.writer, closed: false },
        )
    }
}

/// Read-only half of a stdio transport.  Always-blocking `read_line`;
/// never cancelled mid-read.
pub struct StdioReader<R> {
    reader: BufReader<R>,
    line: String,
}

impl<R> StdioReader<R>
where R: AsyncRead + Unpin + Send + 'static
{
    pub async fn recv(&mut self) -> Result<Option<Message>> {
        loop {
            self.line.clear();
            let n = self.reader.read_line(&mut self.line).await?;
            if n == 0 { return Ok(None); }
            let trimmed = self.line.trim();
            if trimmed.is_empty() { continue; }
            return Ok(Some(Message::from_json(trimmed.as_bytes())?));
        }
    }
}

/// Write-only half of a stdio transport.
pub struct StdioWriter<W> {
    writer: W,
    closed: bool,
}

impl<W> StdioWriter<W>
where W: AsyncWrite + Unpin + Send + 'static
{
    pub async fn send(&mut self, message: Message) -> Result<()> {
        if self.closed { return Err(Error::TransportClosed); }
        let bytes = serde_json::to_vec(&message)?;
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn close(&mut self) {
        if !self.closed { self.closed = true; let _ = self.writer.shutdown().await; }
    }
}

impl StdioTransport<tokio::io::Stdin, tokio::io::Stdout> {
    /// Construct a transport bound to the process stdin/stdout.
    pub fn from_stdio() -> Self {
        Self::new(tokio::io::stdin(), tokio::io::stdout())
    }
}

#[async_trait]
impl<R, W> Transport for StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    async fn send(&mut self, message: Message) -> Result<()> {
        if self.closed {
            return Err(Error::TransportClosed);
        }
        let bytes = serde_json::to_vec(&message)?;
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<Message>> {
        if self.closed {
            return Ok(None);
        }
        self.line.clear();
        let n = self.reader.read_line(&mut self.line).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = self.line.trim();
        if trimmed.is_empty() {
            // Tolerate blank keep-alive lines; recurse via loop.
            return Box::pin(self.recv()).await;
        }
        let msg = Message::from_json(trimmed.as_bytes())?;
        Ok(Some(msg))
    }

    async fn close(&mut self) -> Result<()> {
        if !self.closed {
            self.closed = true;
            let _ = self.writer.shutdown().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_core::{Id, Request};

    #[tokio::test]
    async fn round_trip_message() {
        let (a_r, b_w) = tokio::io::duplex(4096);
        let (b_r, a_w) = tokio::io::duplex(4096);
        let mut a = StdioTransport::new(a_r, a_w);
        let mut b = StdioTransport::new(b_r, b_w);

        let req = Message::Request(Request::new(
            Id::Number(1),
            "ping",
            None,
        ));

        a.send(req).await.unwrap();
        let got = b.recv().await.unwrap().unwrap();
        match got {
            Message::Request(r) => assert_eq!(r.method, "ping"),
            _ => panic!("expected request"),
        }
    }
}
