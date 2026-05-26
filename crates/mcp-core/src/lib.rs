//! MCP core: JSON-RPC 2.0 framing and MCP 2025-06-18 protocol types.

pub mod jsonrpc;
pub mod protocol;
pub mod error;

pub use error::{Error, Result};
pub use jsonrpc::{Id, Message, Request, Response, Notification, ErrorObject};
pub use protocol::{
    PROTOCOL_VERSION,
    Implementation, ClientCapabilities, ServerCapabilities,
    InitializeParams, InitializeResult,
    Tool, ToolInputSchema, ListToolsResult, CallToolParams, CallToolResult, ToolContent,
    Resource, ListResourcesResult, ReadResourceParams, ReadResourceResult, ResourceContents,
    Prompt, PromptArgument, ListPromptsResult, GetPromptParams, GetPromptResult, PromptMessage,
    Role,
    ElicitationParams, ElicitationResult, ElicitationAction,
    // MCP 2025-06-18 optional utilities.
    LogLevel, SetLevelParams, LogMessageParams,
    ProgressToken, ProgressParams,
    CancelledParams,
    CompletionRef, CompletionArgumentRef, CompleteParams, CompletionData, CompleteResult,
};
