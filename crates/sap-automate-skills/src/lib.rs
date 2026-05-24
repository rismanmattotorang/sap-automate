//! Agentic skill library (paper §IX, Phase 8).
//!
//! A *skill* is an agentskills.io-compatible declarative procedure that the
//! agent can load, invoke, and learn from.  Phase 1 ships only the
//! descriptor type so other phases can reference it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Required MCP tools, by fully-qualified name.
    #[serde(default)]
    pub required_tools: Vec<String>,
    /// Skill body: rendered prompt template (Phase 8 extends to a richer DSL).
    pub body: String,
    #[serde(default)]
    pub maintainer: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}
