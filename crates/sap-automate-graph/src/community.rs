//! Community detection (paper §VII-F GraphRAG).
//!
//! Implements a single-pass Louvain modularity step: each node is moved
//! to the community of its strongest-connected neighbour if that move
//! increases modularity.  Iterated to convergence.  Good enough for a
//! pilot-scale corpus; production deployments swap in `leiden-rs` when
//! the graph crosses ~10⁵ edges (paper §X-H exit gate).
//!
//! Output: `Communities { membership, summaries }`.  Each community gets
//! a deterministic summary built from its highest-weighted nodes — a
//! placeholder for the LLM-generated community summary the paper
//! describes (the prompt template is documented in §VII-F).

use crate::entity::NodeId;
use crate::store::InMemoryGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub id: u32,
    pub members: Vec<NodeId>,
    /// Concatenated, deduplicated labels of the most central members.
    /// Used as the "community summary" surface in `kb.global_query`.
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Communities {
    pub communities: Vec<Community>,
    /// Map node → community id.
    pub membership: HashMap<NodeId, u32>,
}

impl Communities {
    pub fn for_node(&self, id: &str) -> Option<&Community> {
        let cid = *self.membership.get(id)?;
        self.communities.iter().find(|c| c.id == cid)
    }
}

/// One-pass modularity-greedy community detection.  Converges in a few
/// iterations on the pilot graph (~25 nodes); for larger graphs the
/// caller should swap in a proper Leiden implementation.
pub fn detect_communities(g: &InMemoryGraph) -> Communities {
    // 1. Initialise: every node in its own community.  Sort node IDs so
    //    Louvain is deterministic regardless of HashMap iteration order.
    let mut sorted_nodes: Vec<&crate::entity::Entity> = g.nodes().collect();
    sorted_nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let mut membership: HashMap<NodeId, u32> = HashMap::new();
    for (i, e) in sorted_nodes.iter().enumerate() {
        membership.insert(e.id.clone(), i as u32);
    }

    // 2. Pre-compute weighted degree per node.
    let degree: HashMap<NodeId, f32> = sorted_nodes.iter()
        .map(|e| {
            let mut neigh = g.undirected_neighbours(&e.id);
            neigh.sort_by(|a, b| a.0.cmp(&b.0));
            let w: f32 = neigh.iter().map(|(_, w)| *w).sum();
            (e.id.clone(), w)
        })
        .collect();
    let total_weight: f32 = degree.values().sum::<f32>() / 2.0;

    if total_weight == 0.0 {
        // No edges — every node is its own community.
        let communities = build_communities_from_membership(g, &membership);
        return Communities { communities, membership };
    }

    // 3. Repeated passes: for each node, move to the neighbour community
    //    that maximises Δmodularity (Newman-Girvan formulation, weighted).
    //    Sorted node order = deterministic outcome.
    let nodes: Vec<NodeId> = sorted_nodes.iter().map(|e| e.id.clone()).collect();
    let mut moved = true;
    let mut passes = 0;
    while moved && passes < 8 {
        moved = false;
        passes += 1;
        for node in &nodes {
            let cur_comm = membership[node];
            let mut neighbours = g.undirected_neighbours(node);
            neighbours.sort_by(|a, b| a.0.cmp(&b.0));
            if neighbours.is_empty() { continue; }

            // For each candidate community (cur + each neighbour's
            // community), compute the modularity gain of switching.
            let k_i = *degree.get(node).unwrap_or(&0.0);
            let mut best_comm = cur_comm;
            let mut best_gain = 0.0f32;

            // Sum of edge weights from `node` to each neighbour community.
            let mut to_comm: HashMap<u32, f32> = HashMap::new();
            for (n, w) in &neighbours {
                let c = membership[n];
                *to_comm.entry(c).or_insert(0.0) += w;
            }
            // Σ_tot per community.
            let mut sigma_tot: HashMap<u32, f32> = HashMap::new();
            for n in &nodes {
                if n == node { continue; }
                let c = membership[n];
                *sigma_tot.entry(c).or_insert(0.0) += *degree.get(n).unwrap_or(&0.0);
            }

            // Visit candidates in sorted order for determinism.
            let mut cands: Vec<(u32, f32)> = to_comm.into_iter().collect();
            cands.sort_by_key(|(c, _)| *c);
            for (cand, k_i_to_c) in cands {
                if cand == cur_comm { continue; }
                let sigma_c = *sigma_tot.get(&cand).unwrap_or(&0.0);
                // Δ Q ≈ [k_i_to_c / m] − [sigma_c * k_i / (2 m^2)]
                let m = total_weight;
                let gain = k_i_to_c / m - (sigma_c * k_i) / (2.0 * m * m);
                if gain > best_gain {
                    best_gain = gain;
                    best_comm = cand;
                }
            }
            if best_comm != cur_comm {
                membership.insert(node.clone(), best_comm);
                moved = true;
            }
        }
    }

    let communities = build_communities_from_membership(g, &membership);
    let renumbered = renumber(communities, &mut membership);
    Communities { communities: renumbered, membership }
}

fn build_communities_from_membership(g: &InMemoryGraph, membership: &HashMap<NodeId, u32>) -> Vec<Community> {
    let mut grouped: HashMap<u32, Vec<NodeId>> = HashMap::new();
    for (id, c) in membership {
        grouped.entry(*c).or_default().push(id.clone());
    }
    grouped.into_iter().map(|(id, mut members)| {
        members.sort();
        // Summary: list the highest-degree members' labels.
        let mut by_label: Vec<String> = members.iter()
            .filter_map(|m| g.node(m).map(|e| e.label.clone()))
            .collect();
        by_label.sort();
        by_label.dedup();
        let summary = by_label.join(" · ");
        Community { id, members, summary }
    }).collect()
}

fn renumber(mut communities: Vec<Community>, membership: &mut HashMap<NodeId, u32>) -> Vec<Community> {
    communities.sort_by_key(|c| std::cmp::Reverse(c.members.len()));
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for (new_id, c) in communities.iter_mut().enumerate() {
        remap.insert(c.id, new_id as u32);
        c.id = new_id as u32;
    }
    for v in membership.values_mut() { *v = remap[v]; }
    communities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_communities_on_demo_corpus_produces_modules() {
        let g = InMemoryGraph::with_demo_corpus();
        let result = detect_communities(&g);
        // Single-pass Louvain on 25 nodes / 34 edges should produce
        // a small handful of communities, not one giant blob.
        assert!(result.communities.len() >= 2 && result.communities.len() <= 12,
            "expected 2..=12 communities, got {}", result.communities.len());

        // The FI cluster (program + class + interface + concept + journal
        // BAPI + Help page) should not all be in 6 different communities.
        let fi_nodes = ["abap:ZFIN_POST_JE", "abap:ZCL_FIN_POSTER",
                        "rfc:BAPI_ACC_DOCUMENT_POST", "concept:journal_entry",
                        "concept:period_close", "help:FI/period-close"];
        let comms: std::collections::HashSet<u32> = fi_nodes.iter()
            .filter_map(|n| result.membership.get(*n).copied())
            .collect();
        assert!(comms.len() <= 3,
            "FI-related entities scattered across {} communities; algorithm is under-clustering", comms.len());
    }
}
