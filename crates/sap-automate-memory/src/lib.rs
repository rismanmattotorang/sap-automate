//! Four-tier memory (paper §IX-B, Phase 8).
//!
//! Tier ordering: working → episodic → procedural → semantic, matching the
//! read-cost gradient described in the paper.  Phase 1 ships only the
//! enum + a working-memory in-RAM store so dependent crates can compile.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Working,
    Episodic,
    Semantic,
    Procedural,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub tier: Tier,
    pub key: String,
    pub value: serde_json::Value,
    /// Paper §IX-G: user-confirmed | agent-derived | unverified.
    #[serde(default = "default_trust")]
    pub trust_class: String,
}

fn default_trust() -> String { "unverified".into() }
