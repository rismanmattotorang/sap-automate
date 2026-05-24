//! MCP server framework.
//!
//! Provides a builder-style `ServerBuilder` for registering tools, resources,
//! and prompts, and a `Server::run` driver that owns the MCP dispatch loop on
//! top of any `Transport`.
//!
//! Aligns with paper §IV.D: capability router with typed handlers and a
//! method table.  We use trait-objects + a sync `HashMap` registry rather than
//! a derive macro at this stage — that is a Phase 1 finalisation item.

pub mod elicit;
pub mod registry;
mod server;

pub use elicit::{
    elicit, current_context, object_schema, ElicitationHandle, ToolContext, TOOL_CONTEXT,
};
pub use registry::{
    ToolHandler, ToolFn, ResourceHandler, PromptHandler,
    ToolDescriptor, ResourceDescriptor, PromptDescriptor,
    ToolExposure, ExposurePolicy,
};
pub use server::{Server, ServerBuilder, ServerContext};

pub use mcp_core as core;
