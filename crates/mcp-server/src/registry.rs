//! Tool / resource / prompt registries.

use async_trait::async_trait;
use mcp_core::{
    CallToolResult, GetPromptResult, Prompt, ReadResourceResult, Resource, Result, Tool,
    ToolInputSchema,
};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Async tool handler trait.
///
/// `arguments` is the raw JSON value passed by the client; handlers are
/// responsible for validating against their declared schema.  Schema-driven
/// validation is a Phase 1 finalisation item once the derive macro lands.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    async fn call(&self, arguments: Value) -> Result<CallToolResult>;
}

/// Convenience adapter for plain async closures.
pub struct ToolFn<F>(pub F);

#[async_trait]
impl<F, Fut> ToolHandler for ToolFn<F>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<CallToolResult>> + Send + 'static,
{
    async fn call(&self, arguments: Value) -> Result<CallToolResult> {
        (self.0)(arguments).await
    }
}

/// Tool exposure group — pattern adopted from `fr0ster/mcp-abap-adt`'s
/// `IReadOnlyDedupStrategy`.  Lets operators run the same server binary
/// in a strict read-only mode (group = `ReadOnly`), enable write tools
/// (`Writes`), or expose everything (`All`).  Defaults to `ReadOnly` for
/// safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    /// Safe to expose in read-only deployments.
    ReadOnly,
    /// Mutates SAP state.
    Writes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposurePolicy {
    /// Only tools tagged `ReadOnly`.
    ReadOnlyOnly,
    /// Read-only + write tools (typical when `--enable-writes` is set).
    All,
}

#[derive(Clone)]
pub struct ToolDescriptor {
    pub tool: Tool,
    pub handler: Arc<dyn ToolHandler>,
    pub exposure: ToolExposure,
}

impl ToolDescriptor {
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        input_schema: ToolInputSchema,
        handler: Arc<dyn ToolHandler>,
    ) -> Self {
        Self {
            tool: Tool { name: name.into(), description, input_schema },
            handler,
            exposure: ToolExposure::ReadOnly,
        }
    }

    /// Builder-style mutator: mark this tool as writing to SAP state.
    pub fn with_writes(mut self) -> Self {
        self.exposure = ToolExposure::Writes;
        self
    }

    pub fn is_allowed_by(&self, policy: ExposurePolicy) -> bool {
        match (policy, self.exposure) {
            (ExposurePolicy::ReadOnlyOnly, ToolExposure::ReadOnly) => true,
            (ExposurePolicy::ReadOnlyOnly, ToolExposure::Writes) => false,
            (ExposurePolicy::All, _) => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait ResourceHandler: Send + Sync {
    fn read(&self, uri: &str) -> BoxFuture<'_, Result<ReadResourceResult>>;
}

#[derive(Clone)]
pub struct ResourceDescriptor {
    pub resource: Resource,
    pub handler: Arc<dyn ResourceHandler>,
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

pub trait PromptHandler: Send + Sync {
    fn get(&self, arguments: Option<Value>) -> BoxFuture<'_, Result<GetPromptResult>>;
}

#[derive(Clone)]
pub struct PromptDescriptor {
    pub prompt: Prompt,
    pub handler: Arc<dyn PromptHandler>,
}
