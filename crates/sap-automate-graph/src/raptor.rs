//! RAPTOR-style hierarchical clusters — paper §VII-E.
//!
//! Production RAPTOR clusters chunk embeddings, summarises each cluster
//! with an LLM, then recursively re-embeds + clusters the summaries.
//! This crate ships a deterministic substitute keyed off entity metadata
//! (community + tag).  The shape of `RaptorTree` matches what the paper
//! describes so the LLM-driven implementation drops in as a backend swap.

use crate::community::Communities;
use crate::entity::{Entity, NodeId};
use crate::store::InMemoryGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorNode {
    pub id: String,
    /// 0 = leaves (entities); >0 = synthesis nodes.
    pub level: u32,
    pub summary: String,
    pub members: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorLevel {
    pub level: u32,
    pub nodes: Vec<RaptorNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorTree {
    pub levels: Vec<RaptorLevel>,
}

impl RaptorTree {
    /// Return the leaves (level 0).
    pub fn leaves(&self) -> &[RaptorNode] {
        self.levels.iter().find(|l| l.level == 0).map(|l| l.nodes.as_slice()).unwrap_or(&[])
    }
    /// Return the highest summary level.
    pub fn root(&self) -> Option<&RaptorLevel> {
        self.levels.iter().max_by_key(|l| l.level)
    }
}

/// Build a RAPTOR tree from the graph + community detection result.
///
/// Levels:
///   - L0: one node per entity (label as summary).
///   - L1: one node per Louvain community (members + concatenated labels).
///   - L2: cross-domain roll-up by `module:` tag.
pub fn build_raptor_tree(g: &InMemoryGraph, communities: &Communities) -> RaptorTree {
    let mut levels = Vec::new();

    // L0 — leaves
    let leaves: Vec<RaptorNode> = g.nodes().map(|e: &Entity| RaptorNode {
        id: format!("leaf:{}", e.id),
        level: 0,
        summary: format!("{} — {}", e.label, e.description.as_deref().unwrap_or("")),
        members: vec![e.id.clone()],
    }).collect();
    levels.push(RaptorLevel { level: 0, nodes: leaves });

    // L1 — one node per community
    let l1: Vec<RaptorNode> = communities.communities.iter().map(|c| RaptorNode {
        id: format!("community:{}", c.id),
        level: 1,
        summary: format!("Community #{} ({} members): {}", c.id, c.members.len(), c.summary),
        members: c.members.clone(),
    }).collect();
    levels.push(RaptorLevel { level: 1, nodes: l1 });

    // L2 — module roll-ups (FI / MM / SD / HCM, plus a catch-all).
    let mut module_groups: HashMap<String, Vec<NodeId>> = HashMap::new();
    for e in g.nodes() {
        let module = e.tags.iter()
            .find(|t| t.starts_with("module:"))
            .map(|t| t["module:".len()..].to_string())
            .unwrap_or_else(|| "other".into());
        module_groups.entry(module).or_default().push(e.id.clone());
    }
    let l2: Vec<RaptorNode> = module_groups.into_iter().map(|(module, members)| {
        let labels: Vec<String> = members.iter()
            .filter_map(|m| g.node(m).map(|e| e.label.clone()))
            .collect();
        RaptorNode {
            id: format!("module:{}", module),
            level: 2,
            summary: format!("Module {}: {} entities — {}", module.to_uppercase(), members.len(), labels.join(", ")),
            members,
        }
    }).collect();
    levels.push(RaptorLevel { level: 2, nodes: l2 });

    RaptorTree { levels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::community::detect_communities;

    #[test]
    fn raptor_tree_has_three_levels() {
        let g = InMemoryGraph::with_demo_corpus();
        let cs = detect_communities(&g);
        let tree = build_raptor_tree(&g, &cs);
        assert_eq!(tree.levels.len(), 3);
        // L2 should include a "FI" module roll-up.
        let l2 = &tree.levels[2];
        assert!(l2.nodes.iter().any(|n| n.id == "module:FI"));
    }
}
