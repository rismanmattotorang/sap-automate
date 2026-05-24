use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// JSON-RPC 2.0 reserved error codes plus MCP-specific extensions.
///
/// See the SAP-Automate paper §IV.I for the structured error taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    // JSON-RPC 2.0 reserved
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,

    // MCP transient (-32100 .. -32199): retryable
    Timeout = -32100,
    ToolBusy = -32004,
    UpstreamUnavailable = -32101,

    // MCP permanent (-32200 .. -32299): not retryable
    AccessDenied = -32200,
    SchemaViolation = -32201,
    UnknownTool = -32202,

    // MCP degraded (-32300 .. -32399): partial result
    PartialResult = -32300,
    StaleCache = -32301,
}

impl ErrorCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("JSON serialisation: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("MCP protocol error ({code}): {message}")]
    Protocol { code: i32, message: String },

    #[error("transport closed")]
    TransportClosed,

    #[error("transport error: {0}")]
    Transport(String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("call timed out")]
    Timeout,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn protocol(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Protocol { code: code.as_i32(), message: message.into() }
    }
}
