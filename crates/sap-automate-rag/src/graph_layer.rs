//! Graph-aware retrieval layers (paper §VII-E/F/G).
//!
//! L3 GraphRAG — community-level synthesis for global/analytical queries.
//! L4 HippoRAG — Personalised PageRank multi-hop traversal.
//! L5 RAPTOR  — hierarchical summary at the requested granularity.
//!
//! Every layer shares the same response shape so callers can render
//! results regardless of which layer fired.

use sap_automate_graph::{
    detect_communities, multi_hop_search, build_raptor_tree, Communities, InMemoryGraph,
    PprConfig, RaptorTree,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSearchHit {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub uri: Option<String>,
    pub score: f32,
    pub hops: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSearchResponse {
    pub layer: String,
    pub seeds: Vec<String>,
    pub hits: Vec<GraphSearchHit>,
    pub community_summary: Option<String>,
    pub elapsed_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityQueryResponse {
    pub query: String,
    pub matched_communities: Vec<CommunityView>,
    pub elapsed_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityView {
    pub id: u32,
    pub members: Vec<String>,
    pub summary: String,
    pub overlap_score: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorSummaryResponse {
    pub level: u32,
    pub nodes: Vec<RaptorView>,
    pub elapsed_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorView {
    pub id: String,
    pub level: u32,
    pub summary: String,
    pub member_count: usize,
}

pub struct GraphEngine {
    pub graph: Arc<InMemoryGraph>,
    pub communities: Arc<Communities>,
    pub raptor: Arc<RaptorTree>,
}

impl GraphEngine {
    pub fn new(graph: Arc<InMemoryGraph>) -> Self {
        let communities = Arc::new(detect_communities(&graph));
        let raptor = Arc::new(build_raptor_tree(&graph, &communities));
        Self { graph, communities, raptor }
    }

    /// L4 HippoRAG path-based Q&A.
    pub fn multi_hop(&self, query: &str, max_hops: u32, top_k: usize, max_seeds: usize) -> GraphSearchResponse {
        let t0 = Instant::now();
        let cfg = PprConfig { max_hops, ..PprConfig::default() };
        let r = multi_hop_search(&self.graph, query, max_seeds, &cfg, top_k);
        let hits = r.ranked.iter().filter_map(|h| {
            self.graph.node(&h.id).map(|e| GraphSearchHit {
                id: e.id.clone(),
                label: e.label.clone(),
                kind: format!("{:?}", e.kind),
                uri: e.uri.clone(),
                score: h.score,
                hops: h.hops,
            })
        }).collect();
        GraphSearchResponse {
            layer: "L4 HippoRAG".into(),
            seeds: r.seeds,
            hits,
            community_summary: None,
            elapsed_us: t0.elapsed().as_micros() as u64,
        }
    }

    /// L4 HippoRAG over an explicit list of seed entity IDs (used by
    /// `kb.graph_neighborhood`).
    pub fn neighborhood(&self, seeds: &[String], max_hops: u32, top_k: usize) -> GraphSearchResponse {
        let t0 = Instant::now();
        let cfg = PprConfig { max_hops, ..PprConfig::default() };
        let r = sap_automate_graph::personalised_pagerank(&self.graph, seeds, &cfg);
        let hits: Vec<GraphSearchHit> = r.ranked.iter()
            .filter(|h| h.hops <= max_hops)
            .take(top_k)
            .filter_map(|h| self.graph.node(&h.id).map(|e| GraphSearchHit {
                id: e.id.clone(),
                label: e.label.clone(),
                kind: format!("{:?}", e.kind),
                uri: e.uri.clone(),
                score: h.score,
                hops: h.hops,
            }))
            .collect();
        GraphSearchResponse {
            layer: "L4 HippoRAG (explicit seeds)".into(),
            seeds: seeds.to_vec(),
            hits,
            community_summary: None,
            elapsed_us: t0.elapsed().as_micros() as u64,
        }
    }

    /// L3 GraphRAG global query — finds the communities that overlap the
    /// query terms and returns their summaries.
    pub fn community_query(&self, query: &str, top_k: usize) -> CommunityQueryResponse {
        let t0 = Instant::now();
        let q = query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().filter(|t| t.len() >= 2).collect();
        let mut scored: Vec<CommunityView> = self.communities.communities.iter().map(|c| {
            let hay = c.summary.to_lowercase();
            let overlap: usize = terms.iter().map(|t| hay.matches(t).count()).sum();
            CommunityView {
                id: c.id,
                members: c.members.clone(),
                summary: c.summary.clone(),
                overlap_score: overlap,
            }
        }).filter(|v| v.overlap_score > 0).collect();
        scored.sort_by(|a, b| b.overlap_score.cmp(&a.overlap_score));
        scored.truncate(top_k);
        CommunityQueryResponse {
            query: query.into(),
            matched_communities: scored,
            elapsed_us: t0.elapsed().as_micros() as u64,
        }
    }

    /// L5 RAPTOR summarisation at the requested level.
    pub fn raptor_summary(&self, level: u32, top_k: usize) -> RaptorSummaryResponse {
        let t0 = Instant::now();
        let nodes = self.raptor.levels.iter()
            .find(|l| l.level == level)
            .map(|l| l.nodes.clone())
            .unwrap_or_default();
        let views: Vec<RaptorView> = nodes.into_iter().take(top_k).map(|n| RaptorView {
            id: n.id,
            level: n.level,
            summary: n.summary,
            member_count: n.members.len(),
        }).collect();
        RaptorSummaryResponse {
            level,
            nodes: views,
            elapsed_us: t0.elapsed().as_micros() as u64,
        }
    }

    /// Paper §VII Algorithm 1 router.  Returns the layer(s) the engine
    /// should fire for this query.
    pub fn route(&self, query: &str, scope_hint: Option<&str>) -> Vec<&'static str> {
        let lc = query.to_lowercase();
        let global = matches!(scope_hint, Some("global"))
            || ["across", "all", "every", "summarise", "summarize", "overall", "in general"]
                .iter().any(|kw| lc.contains(kw));
        let multi_hop = ["impact", "where used", "depends on", "calls", "callers", "downstream", "upstream", "trace"]
            .iter().any(|kw| lc.contains(kw));

        let mut layers = Vec::new();
        if global && !multi_hop { layers.push("L3"); layers.push("L5"); }
        if multi_hop { layers.push("L4"); }
        if layers.is_empty() { layers.push("L2"); }
        layers
    }
}
