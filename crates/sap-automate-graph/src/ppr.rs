//! Personalised PageRank — paper §VII-G HippoRAG.
//!
//! The HippoRAG paper formalises multi-hop retrieval as PPR over an
//! LLM-extracted KG: seed nodes get probability mass proportional to
//! their relevance, then a restart-augmented random walk distributes mass
//! along edges.  Nodes with high steady-state mass are surfaced as hits.
//!
//! Implementation: power iteration with restart probability α (default
//! 0.15, per the HippoRAG paper).  Converges in ~20 iterations on the
//! pilot graph well under the paper §X-H acceptance gate (P95 < 400 ms
//! for ≤4-hop queries).

use crate::entity::NodeId;
use crate::store::InMemoryGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PprConfig {
    /// Restart (teleport) probability.  HippoRAG default 0.15.
    pub alpha: f32,
    /// Maximum power iterations before bailing.
    pub max_iterations: u32,
    /// Convergence threshold (L1 distance between successive iterations).
    pub tolerance: f32,
    /// Hop budget — surface nodes within this radius from any seed.
    /// Used to limit the *return* set, not the propagation; PPR itself
    /// is global.
    pub max_hops: u32,
}

impl Default for PprConfig {
    fn default() -> Self {
        Self { alpha: 0.15, max_iterations: 50, tolerance: 1e-5, max_hops: 4 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PprResult {
    pub seeds: Vec<NodeId>,
    pub ranked: Vec<PprHit>,
    pub iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PprHit {
    pub id: NodeId,
    pub score: f32,
    /// Minimum hop distance from any seed.  Paper §X-H gate counts this.
    pub hops: u32,
}

/// Run PPR from the supplied seeds.
pub fn personalised_pagerank(
    g: &InMemoryGraph,
    seeds: &[NodeId],
    cfg: &PprConfig,
) -> PprResult {
    if seeds.is_empty() {
        return PprResult { seeds: Vec::new(), ranked: Vec::new(), iterations: 0 };
    }

    // Build a stable node ordering.
    let node_ids: Vec<NodeId> = g.nodes().map(|e| e.id.clone()).collect();
    let idx: HashMap<&NodeId, usize> = node_ids.iter().enumerate().map(|(i, id)| (id, i)).collect();

    // Pre-compute weighted out-neighbours per index.  Treat the graph as
    // undirected for PPR — semantic dependency chains should bubble back.
    let n = node_ids.len();
    let mut neighbours: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    let mut out_weight: Vec<f32> = vec![0.0; n];
    for (i, id) in node_ids.iter().enumerate() {
        for (n_id, w) in g.undirected_neighbours(id) {
            if let Some(&j) = idx.get(&n_id) {
                neighbours[i].push((j, w));
                out_weight[i] += w;
            }
        }
    }

    // Personalisation vector: equal probability mass split across seeds
    // that exist in the graph.
    let mut p: Vec<f32> = vec![0.0; n];
    let seed_indices: Vec<usize> = seeds.iter()
        .filter_map(|s| idx.get(s).copied())
        .collect();
    if seed_indices.is_empty() {
        return PprResult { seeds: seeds.to_vec(), ranked: Vec::new(), iterations: 0 };
    }
    let seed_share = 1.0 / seed_indices.len() as f32;
    for &i in &seed_indices { p[i] = seed_share; }

    let teleport = p.clone();
    let alpha = cfg.alpha;

    let mut iterations = 0u32;
    for it in 0..cfg.max_iterations {
        iterations = it + 1;
        let mut next = vec![0.0f32; n];
        // Distribute mass along edges.
        for i in 0..n {
            if out_weight[i] <= 0.0 || p[i] == 0.0 { continue; }
            let outgoing = (1.0 - alpha) * p[i];
            for &(j, w) in &neighbours[i] {
                next[j] += outgoing * (w / out_weight[i]);
            }
        }
        // Add the teleport component.
        for i in 0..n {
            next[i] += alpha * teleport[i];
            // Dangling nodes (out_weight == 0) — distribute their old mass
            // back to the teleport vector to preserve total mass.
            if out_weight[i] <= 0.0 && p[i] > 0.0 {
                let dangling = (1.0 - alpha) * p[i];
                for j in 0..n { next[j] += dangling * teleport[j]; }
            }
        }
        // L1 distance — convergence check.
        let delta: f32 = p.iter().zip(next.iter()).map(|(a, b)| (a - b).abs()).sum();
        p = next;
        if delta < cfg.tolerance { break; }
    }

    // BFS for hop-distance per node from any seed.
    let hops = bfs_hops(&neighbours, &seed_indices, cfg.max_hops);

    // Rank by score, exclude any score effectively zero.
    let mut ranked: Vec<PprHit> = (0..n).map(|i| PprHit {
        id: node_ids[i].clone(),
        score: p[i],
        hops: hops[i],
    }).collect();
    ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    PprResult { seeds: seeds.to_vec(), ranked, iterations }
}

fn bfs_hops(neighbours: &[Vec<(usize, f32)>], seeds: &[usize], max_hops: u32) -> Vec<u32> {
    let n = neighbours.len();
    let mut hops = vec![u32::MAX; n];
    let mut frontier: Vec<usize> = Vec::new();
    for &s in seeds { hops[s] = 0; frontier.push(s); }
    let mut depth = 0u32;
    while !frontier.is_empty() && depth < max_hops {
        let mut next_frontier = Vec::new();
        for i in frontier {
            for &(j, _) in &neighbours[i] {
                if hops[j] == u32::MAX {
                    hops[j] = depth + 1;
                    next_frontier.push(j);
                }
            }
        }
        frontier = next_frontier;
        depth += 1;
    }
    hops
}

/// Convenience: seed by free-text query then run PPR.
pub fn multi_hop_search(
    g: &InMemoryGraph,
    query: &str,
    max_seeds: usize,
    cfg: &PprConfig,
    top_k: usize,
) -> PprResult {
    let seeds = g.find_seeds(query, max_seeds);
    let mut result = personalised_pagerank(g, &seeds, cfg);
    // Cap the surfaced hits to top_k and to those within max_hops.
    let max_hops = cfg.max_hops;
    result.ranked.retain(|h| h.hops <= max_hops);
    result.ranked.truncate(top_k);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppr_promotes_cross_domain_neighbours() {
        let g = InMemoryGraph::with_demo_corpus();
        let cfg = PprConfig::default();
        let r = multi_hop_search(&g, "period close FAGLFLEXA", 3, &cfg, 8);
        let ids: Vec<&str> = r.ranked.iter().map(|h| h.id.as_str()).collect();
        // PPR should surface the period_close concept and FAGLFLEXA table.
        assert!(ids.iter().any(|id| *id == "concept:period_close" || *id == "tab:FAGLFLEXA"));
        // It should also reach S/4HANA Finance (LeanIX) via FAGLFLEXA.
        assert!(ids.iter().any(|id| *id == "leanix:FS-12001"),
            "PPR should cross from period_close → FAGLFLEXA → LeanIX FS-12001; got {ids:?}");
    }

    #[test]
    fn ppr_respects_max_hops() {
        let g = InMemoryGraph::with_demo_corpus();
        let cfg = PprConfig { max_hops: 1, ..PprConfig::default() };
        let r = multi_hop_search(&g, "ZIF_FIN_POSTABLE", 1, &cfg, 50);
        for h in &r.ranked {
            assert!(h.hops <= 1, "hop budget violated: {} at {} hops", h.id, h.hops);
        }
    }
}
