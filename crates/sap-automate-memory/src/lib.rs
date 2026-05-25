//! Four-tier memory architecture (paper §IX-B).
//!
//! Tier ordering reflects the read-cost gradient: working (in-RAM ring
//! buffer) is cheapest; semantic (RAG corpus) is most expensive.  Agents
//! assemble context in this order to amortise the embedding-call cost.
//!
//! | Tier        | Backing                       | Persistence | Typical writes  |
//! |-------------|-------------------------------|-------------|-----------------|
//! | Working     | bounded ring per session      | none        | per-tool-call   |
//! | Episodic    | dated record store            | local/Qdrant| per-conversation|
//! | Semantic    | `sap-automate-rag` corpus     | KB-resident | crawler ingestion |
//! | Procedural  | `sap-automate-skills`         | filesystem  | code review     |
//!
//! Phase 8 ships the in-RAM working + episodic implementations; the
//! semantic and procedural tiers are referenced via existing crates.
//! Production deployments swap working / episodic for Redis + Qdrant
//! without touching agent code.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier { Working, Episodic, Semantic, Procedural }

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("session not found: {0}")]
    UnknownSession(String),
    #[error("backend: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub tier: Tier,
    pub key: String,
    pub value: serde_json::Value,
    /// Trust class (paper §IX-G): user-confirmed | agent-derived | unverified.
    #[serde(default = "default_trust")]
    pub trust_class: String,
    /// Optional tenancy bound — only this tenant can read it.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Unix-epoch milliseconds.
    pub created_at_ms: u64,
}

impl MemoryEntry {
    pub fn new(tier: Tier, key: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            tier, key: key.into(), value,
            trust_class: default_trust(),
            tenant: None,
            created_at_ms: now_ms(),
        }
    }
    pub fn with_trust(mut self, trust: &str) -> Self {
        self.trust_class = trust.into(); self
    }
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into()); self
    }
}

fn default_trust() -> String { "unverified".into() }
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Working memory — per-session ring buffer
// ---------------------------------------------------------------------------

const DEFAULT_WORKING_CAP: usize = 64;

#[derive(Debug, Default)]
pub struct WorkingMemory {
    sessions: RwLock<HashMap<String, VecDeque<MemoryEntry>>>,
    cap: usize,
}

impl WorkingMemory {
    pub fn new() -> Self { Self::with_capacity(DEFAULT_WORKING_CAP) }
    pub fn with_capacity(cap: usize) -> Self {
        Self { sessions: RwLock::new(HashMap::new()), cap: cap.max(1) }
    }

    pub fn append(&self, session_id: &str, entry: MemoryEntry) {
        let mut s = self.sessions.write().unwrap();
        let q = s.entry(session_id.into()).or_default();
        if q.len() == self.cap { q.pop_front(); }
        q.push_back(entry);
    }

    /// Read up to `limit` most-recent entries for the session.
    pub fn recent(&self, session_id: &str, limit: usize) -> Vec<MemoryEntry> {
        let s = self.sessions.read().unwrap();
        match s.get(session_id) {
            Some(q) => q.iter().rev().take(limit).cloned().collect(),
            None => Vec::new(),
        }
    }

    pub fn clear(&self, session_id: &str) {
        self.sessions.write().unwrap().remove(session_id);
    }

    pub fn session_count(&self) -> usize { self.sessions.read().unwrap().len() }
}

// ---------------------------------------------------------------------------
// Episodic memory — dated record store with tag + tenant filters
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct EpisodicMemory {
    /// Flat list; production swaps for Qdrant.  The pilot corpus is
    /// small enough that a linear scan with tag indexes is fast enough.
    entries: RwLock<Vec<MemoryEntry>>,
    /// Secondary index: tag → entry indices.
    by_tag: RwLock<HashMap<String, Vec<usize>>>,
}

impl EpisodicMemory {
    pub fn new() -> Self { Self::default() }

    /// Append a new episode with optional tags.  Returns the new entry
    /// index for later retrieval.
    pub fn record(&self, entry: MemoryEntry, tags: &[&str]) -> usize {
        let idx = {
            let mut entries = self.entries.write().unwrap();
            entries.push(entry);
            entries.len() - 1
        };
        let mut by_tag = self.by_tag.write().unwrap();
        for t in tags {
            by_tag.entry((*t).into()).or_default().push(idx);
        }
        idx
    }

    /// Look up episodes by tag (any tenant matches if `tenant` is None;
    /// otherwise only entries with matching tenant or no tenant are
    /// returned).
    pub fn by_tag(&self, tag: &str, tenant: Option<&str>, limit: usize) -> Vec<MemoryEntry> {
        let idxs = {
            let by_tag = self.by_tag.read().unwrap();
            by_tag.get(tag).cloned().unwrap_or_default()
        };
        let entries = self.entries.read().unwrap();
        let mut out: Vec<MemoryEntry> = idxs.into_iter().rev()
            .filter_map(|i| entries.get(i))
            .filter(|e| match (tenant, &e.tenant) {
                (Some(t), Some(et)) => t == et,
                (Some(_), None) => true,    // public entries readable everywhere
                (None, _) => true,           // no tenant filter
            })
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    pub fn total(&self) -> usize { self.entries.read().unwrap().len() }
}

// ---------------------------------------------------------------------------
// Unified MemoryManager — the agent's read entry point
// ---------------------------------------------------------------------------

/// Read-cost gradient ordering: working → episodic → procedural → semantic.
/// The semantic tier is accessed through the RAG engine; agents call it
/// directly via the `sap.docs.search` MCP tool, so MemoryManager doesn't
/// own it — it just signals to the caller that semantic comes last.
pub struct MemoryManager {
    pub working: WorkingMemory,
    pub episodic: EpisodicMemory,
}

impl Default for MemoryManager {
    fn default() -> Self { Self::new() }
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            working: WorkingMemory::new(),
            episodic: EpisodicMemory::new(),
        }
    }

    /// Assemble per-tier context for a session.  Returns a flat list
    /// preserving the gradient order so the agent's prompt-builder can
    /// concatenate it as-is.
    pub fn assemble(&self, session_id: &str, tag: Option<&str>, tenant: Option<&str>) -> Vec<MemoryEntry> {
        let mut out = Vec::new();
        out.extend(self.working.recent(session_id, 8));
        if let Some(tag) = tag {
            out.extend(self.episodic.by_tag(tag, tenant, 8));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_memory_ring_buffer_evicts_oldest() {
        let m = WorkingMemory::with_capacity(2);
        for i in 0..3 {
            m.append("S1", MemoryEntry::new(Tier::Working, format!("k{i}"), serde_json::json!(i)));
        }
        let r = m.recent("S1", 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].key, "k2");
        assert_eq!(r[1].key, "k1");
    }

    #[test]
    fn episodic_tag_filter_with_tenant() {
        let m = EpisodicMemory::new();
        m.record(MemoryEntry::new(Tier::Episodic, "po1", serde_json::json!({"po": "4500001234"})).with_tenant("T1"), &["po", "fi"]);
        m.record(MemoryEntry::new(Tier::Episodic, "po2", serde_json::json!({"po": "4500001235"})).with_tenant("T2"), &["po"]);
        m.record(MemoryEntry::new(Tier::Episodic, "public", serde_json::json!({"note": "shared"})), &["po"]);
        let t1 = m.by_tag("po", Some("T1"), 10);
        assert_eq!(t1.len(), 2, "T1 sees its own + public");
        let t2 = m.by_tag("po", Some("T2"), 10);
        assert_eq!(t2.len(), 2, "T2 sees its own + public");
        assert_eq!(m.by_tag("po", None, 10).len(), 3);
    }

    #[test]
    fn manager_assembles_in_gradient_order() {
        let mm = MemoryManager::new();
        mm.working.append("S1", MemoryEntry::new(Tier::Working, "user_intent", serde_json::json!("FI close")));
        mm.episodic.record(MemoryEntry::new(Tier::Episodic, "last_close", serde_json::json!("M02 closed 2026-03-02")), &["fi"]);
        let ctx = mm.assemble("S1", Some("fi"), None);
        assert_eq!(ctx.len(), 2);
        assert!(matches!(ctx[0].tier, Tier::Working));
        assert!(matches!(ctx[1].tier, Tier::Episodic));
    }
}
