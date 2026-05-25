//! In-memory graph store seeded with realistic cross-domain SAP fixtures.
//!
//! The same fixtures the RFC + ADT + KB mocks expose, but stitched into
//! one graph so multi-hop traversal demos are meaningful offline.
//!
//! Example dependency chain (encoded below):
//!
//!   `ZIF_FIN_POSTABLE` (interface)
//!       ←implements← `ZCL_FIN_POSTER` (class)
//!           ←calls← `ZFIN_POST_JE` (program)
//!               ↓includes
//!           `ZFIN_TOP`, `ZFIN_F01`
//!       ↓calls
//!     `BAPI_ACC_DOCUMENT_POST` (RFC)
//!       ↓reads_table
//!     `T001`, `T001B`
//!       ↓describes
//!     `Concept: period_close`
//!       ←contained_in← `BPMN: Order-to-Cash`
//!       ←depends_on← `LeanIX: S/4HANA Finance`

use crate::entity::{Edge, EdgeKind, Entity, EntityKind, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct InMemoryGraph {
    nodes: HashMap<NodeId, Entity>,
    /// Adjacency: id → list of (neighbour, edge kind, weight)
    out_edges: HashMap<NodeId, Vec<(NodeId, EdgeKind, f32)>>,
    in_edges:  HashMap<NodeId, Vec<(NodeId, EdgeKind, f32)>>,
    /// Raw edge list for community-detection algorithms that prefer it.
    edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub by_kind: HashMap<String, usize>,
}

impl InMemoryGraph {
    pub fn new() -> Self { Self::default() }

    /// Seed with the cross-domain SAP fixture set.  Idempotent.
    pub fn with_demo_corpus() -> Self {
        let mut g = Self::new();
        g.seed();
        g
    }

    pub fn add_entity(&mut self, e: Entity) {
        self.nodes.insert(e.id.clone(), e);
    }

    pub fn add_edge(&mut self, e: Edge) {
        self.out_edges.entry(e.from.clone()).or_default().push((e.to.clone(), e.kind, e.weight));
        self.in_edges .entry(e.to.clone())  .or_default().push((e.from.clone(), e.kind, e.weight));
        self.edges.push(e);
    }

    pub fn node(&self, id: &str) -> Option<&Entity> { self.nodes.get(id) }
    pub fn nodes(&self) -> impl Iterator<Item = &Entity> { self.nodes.values() }
    pub fn edges(&self) -> &[Edge] { &self.edges }

    /// Outgoing neighbours: `id → (to, kind, weight)`.
    pub fn outbound(&self, id: &str) -> &[(NodeId, EdgeKind, f32)] {
        self.out_edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Incoming neighbours.
    pub fn inbound(&self, id: &str) -> &[(NodeId, EdgeKind, f32)] {
        self.in_edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Undirected adjacency for community detection / PPR.
    pub fn undirected_neighbours(&self, id: &str) -> Vec<(NodeId, f32)> {
        let mut seen: HashMap<NodeId, f32> = HashMap::new();
        for (n, _, w) in self.outbound(id) { *seen.entry(n.clone()).or_insert(0.0) += w; }
        for (n, _, w) in self.inbound(id)  { *seen.entry(n.clone()).or_insert(0.0) += w; }
        seen.into_iter().collect()
    }

    pub fn stats(&self) -> GraphStats {
        let mut by_kind: HashMap<String, usize> = HashMap::new();
        for e in self.nodes.values() {
            *by_kind.entry(format!("{:?}", e.kind)).or_insert(0) += 1;
        }
        GraphStats {
            node_count: self.nodes.len(),
            edge_count: self.edges.len(),
            by_kind,
        }
    }

    /// Find nodes by free-text match over label + description + tags.
    /// Used by the HippoRAG seeding step.
    pub fn find_seeds(&self, query: &str, max_seeds: usize) -> Vec<NodeId> {
        let q = query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().filter(|t| t.len() >= 2).collect();
        if terms.is_empty() { return Vec::new(); }
        let mut scored: Vec<(usize, &Entity)> = self.nodes.values().filter_map(|e| {
            let hay = format!(
                "{} {} {}",
                e.label.to_lowercase(),
                e.description.as_deref().unwrap_or("").to_lowercase(),
                e.tags.join(" ").to_lowercase(),
            );
            let score: usize = terms.iter().map(|t| hay.matches(t).count()).sum();
            if score == 0 { None } else { Some((score, e)) }
        }).collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(max_seeds).map(|(_, e)| e.id.clone()).collect()
    }

    fn seed(&mut self) {
        let add = |g: &mut Self, id: &str, kind: EntityKind, label: &str, desc: &str, uri: Option<&str>, tags: &[&str]| {
            g.add_entity(Entity {
                id: id.into(), kind, label: label.into(),
                description: Some(desc.into()),
                uri: uri.map(String::from),
                tags: tags.iter().map(|s| s.to_string()).collect(),
            });
        };

        // ABAP objects
        add(self, "abap:ZFIN_POST_JE", EntityKind::AbapObject, "ZFIN_POST_JE",
            "ABAP report that posts FI journal entries via BAPI_ACC_DOCUMENT_POST.",
            Some("abap-obj://ZFIN/ZFIN_POST_JE"), &["module:FI", "package:ZFIN", "kind:program"]);
        add(self, "abap:ZCL_FIN_POSTER", EntityKind::AbapObject, "ZCL_FIN_POSTER",
            "Helper class implementing ZIF_FIN_POSTABLE.",
            Some("abap-obj://ZFIN/ZCL_FIN_POSTER"), &["module:FI", "package:ZFIN", "kind:class"]);
        add(self, "abap:ZIF_FIN_POSTABLE", EntityKind::AbapObject, "ZIF_FIN_POSTABLE",
            "FI posting contract interface.",
            Some("abap-obj://ZFIN/ZIF_FIN_POSTABLE"), &["module:FI", "package:ZFIN", "kind:interface"]);
        add(self, "abap:ZFIN_TOP", EntityKind::AbapObject, "ZFIN_TOP",
            "Global data definitions for ZFIN programs.",
            Some("abap-obj://ZFIN/ZFIN_TOP"), &["module:FI", "package:ZFIN", "kind:include"]);
        add(self, "abap:ZFIN_F01", EntityKind::AbapObject, "ZFIN_F01",
            "Form routines for FI posting.",
            Some("abap-obj://ZFIN/ZFIN_F01"), &["module:FI", "package:ZFIN", "kind:include"]);
        add(self, "abap:ZMM_GRN_CHECK", EntityKind::AbapObject, "ZMM_GRN_CHECK",
            "Goods receipt reconciliation.",
            Some("abap-obj://ZMM/ZMM_GRN_CHECK"), &["module:MM", "package:ZMM", "kind:program"]);

        // RFCs
        add(self, "rfc:BAPI_ACC_DOCUMENT_POST", EntityKind::Rfc, "BAPI_ACC_DOCUMENT_POST",
            "Post an accounting document (FI journal entry).",
            Some("sap-rfc://BAPI_ACC_DOCUMENT_POST"), &["module:FI", "group:FBAS"]);
        add(self, "rfc:BAPI_GOODSMVT_CREATE", EntityKind::Rfc, "BAPI_GOODSMVT_CREATE",
            "Post a goods movement (MM).",
            Some("sap-rfc://BAPI_GOODSMVT_CREATE"), &["module:MM"]);
        add(self, "rfc:BAPI_MATERIAL_GET_DETAIL", EntityKind::Rfc, "BAPI_MATERIAL_GET_DETAIL",
            "Read material master detail.",
            Some("sap-rfc://BAPI_MATERIAL_GET_DETAIL"), &["module:MM", "group:MGV3"]);
        add(self, "rfc:BAPI_SALESORDER_CREATEFROMDAT2", EntityKind::Rfc, "BAPI_SALESORDER_CREATEFROMDAT2",
            "Create a sales order.",
            Some("sap-rfc://BAPI_SALESORDER_CREATEFROMDAT2"), &["module:SD"]);

        // Tables
        add(self, "tab:T001", EntityKind::Table, "T001",
            "Company codes.", Some("sap-table://T001/structure"), &["module:FI"]);
        add(self, "tab:T001B", EntityKind::Table, "T001B",
            "Posting periods.", Some("sap-table://T001B/structure"), &["module:FI"]);
        add(self, "tab:MARA", EntityKind::Table, "MARA",
            "General material data.", Some("sap-table://MARA/structure"), &["module:MM"]);
        add(self, "tab:VBAK", EntityKind::Table, "VBAK",
            "Sales document header.", Some("sap-table://VBAK/structure"), &["module:SD"]);
        add(self, "tab:BSEG", EntityKind::Table, "BSEG",
            "Accounting document segment.", Some("sap-table://BSEG/structure"), &["module:FI"]);
        add(self, "tab:FAGLFLEXA", EntityKind::Table, "FAGLFLEXA",
            "Actual line items of general ledger.", Some("sap-table://FAGLFLEXA/structure"), &["module:FI"]);

        // BPMN processes
        add(self, "bpmn:P2P-001", EntityKind::BpmnProcess, "Procure-to-Pay (P2P)",
            "Purchase requisition through invoice verification.",
            Some("bpmn-proc://core/P2P-001"), &["process:p2p"]);
        add(self, "bpmn:O2C-002", EntityKind::BpmnProcess, "Order-to-Cash (O2C)",
            "Sales order through cash application.",
            Some("bpmn-proc://core/O2C-002"), &["process:o2c"]);

        // LeanIX apps
        add(self, "leanix:FS-12001", EntityKind::LeanixApp, "S/4HANA Finance",
            "Finance application running general ledger, AP, AR.",
            Some("leanix-fs://FS-12001"), &["lifecycle:active"]);
        add(self, "leanix:FS-08823", EntityKind::LeanixApp, "Legacy Billing Engine",
            "Phase-out billing engine.", Some("leanix-fs://FS-08823"), &["lifecycle:phase_out"]);

        // Help pages
        add(self, "help:FI/period-close", EntityKind::HelpPage, "Period-End Close in SAP FI",
            "Procedure for FI period-end close.", Some("sap-help://FI/period-close"), &["module:FI"]);
        add(self, "help:MM/goods-movement", EntityKind::HelpPage, "Goods Movement Posting",
            "Procedure for MM goods movements.", Some("sap-help://MM/goods-movement"), &["module:MM"]);

        // Concepts (cross-domain hubs)
        add(self, "concept:period_close", EntityKind::Concept, "Period Close",
            "FI period-end close: open/close posting periods, foreign currency revaluation, reconciliation.",
            None, &["module:FI"]);
        add(self, "concept:goods_movement", EntityKind::Concept, "Goods Movement",
            "Posting goods receipts, issues, and transfer postings against material master.",
            None, &["module:MM"]);
        add(self, "concept:journal_entry", EntityKind::Concept, "Journal Entry",
            "Accounting document posting that creates FI documents.",
            None, &["module:FI"]);

        // Edges
        let edges: Vec<(&str, &str, EdgeKind, f32)> = vec![
            // ABAP class implements interface
            ("abap:ZCL_FIN_POSTER", "abap:ZIF_FIN_POSTABLE", EdgeKind::Implements, 1.0),
            // Program uses class
            ("abap:ZFIN_POST_JE", "abap:ZCL_FIN_POSTER", EdgeKind::Calls, 1.0),
            // Program includes data + form
            ("abap:ZFIN_POST_JE", "abap:ZFIN_TOP", EdgeKind::Includes, 1.0),
            ("abap:ZFIN_POST_JE", "abap:ZFIN_F01", EdgeKind::Includes, 1.0),
            // Class + program call RFC
            ("abap:ZCL_FIN_POSTER", "rfc:BAPI_ACC_DOCUMENT_POST", EdgeKind::Calls, 2.0),
            ("abap:ZFIN_POST_JE",   "rfc:BAPI_ACC_DOCUMENT_POST", EdgeKind::Calls, 1.0),
            ("abap:ZMM_GRN_CHECK",  "rfc:BAPI_GOODSMVT_CREATE",   EdgeKind::Calls, 1.0),
            // RFC reads tables
            ("rfc:BAPI_ACC_DOCUMENT_POST", "tab:T001",     EdgeKind::ReadsTable, 1.0),
            ("rfc:BAPI_ACC_DOCUMENT_POST", "tab:T001B",    EdgeKind::ReadsTable, 1.0),
            ("rfc:BAPI_ACC_DOCUMENT_POST", "tab:BSEG",     EdgeKind::WritesTable, 1.0),
            ("rfc:BAPI_ACC_DOCUMENT_POST", "tab:FAGLFLEXA",EdgeKind::WritesTable, 1.0),
            ("rfc:BAPI_GOODSMVT_CREATE",   "tab:MARA",     EdgeKind::ReadsTable, 1.0),
            ("rfc:BAPI_SALESORDER_CREATEFROMDAT2", "tab:VBAK", EdgeKind::WritesTable, 1.0),
            // BPMN depends on RFCs
            ("bpmn:P2P-001", "rfc:BAPI_GOODSMVT_CREATE",        EdgeKind::DependsOn, 1.0),
            ("bpmn:P2P-001", "rfc:BAPI_ACC_DOCUMENT_POST",      EdgeKind::DependsOn, 1.0),
            ("bpmn:O2C-002", "rfc:BAPI_SALESORDER_CREATEFROMDAT2", EdgeKind::DependsOn, 1.0),
            ("bpmn:O2C-002", "rfc:BAPI_ACC_DOCUMENT_POST",      EdgeKind::DependsOn, 1.0),
            // LeanIX apps depend on tables
            ("leanix:FS-12001", "tab:BSEG",     EdgeKind::DependsOn, 1.0),
            ("leanix:FS-12001", "tab:FAGLFLEXA",EdgeKind::DependsOn, 1.0),
            ("leanix:FS-12001", "tab:T001",     EdgeKind::DependsOn, 1.0),
            ("leanix:FS-08823", "tab:VBAK",     EdgeKind::DependsOn, 1.0),
            // Concepts describe entities (the cross-domain hubs)
            ("concept:period_close",   "help:FI/period-close",   EdgeKind::Describes, 2.0),
            ("concept:period_close",   "tab:T001B",              EdgeKind::Describes, 2.0),
            ("concept:period_close",   "tab:FAGLFLEXA",          EdgeKind::Describes, 1.5),
            ("concept:period_close",   "leanix:FS-12001",        EdgeKind::Describes, 1.0),
            ("concept:journal_entry",  "rfc:BAPI_ACC_DOCUMENT_POST", EdgeKind::Describes, 2.0),
            ("concept:journal_entry",  "tab:BSEG",               EdgeKind::Describes, 1.5),
            ("concept:journal_entry",  "abap:ZFIN_POST_JE",      EdgeKind::Describes, 1.5),
            ("concept:goods_movement", "help:MM/goods-movement", EdgeKind::Describes, 2.0),
            ("concept:goods_movement", "rfc:BAPI_GOODSMVT_CREATE", EdgeKind::Describes, 2.0),
            ("concept:goods_movement", "abap:ZMM_GRN_CHECK",     EdgeKind::Describes, 1.0),
            // Help pages reference tables / RFCs
            ("help:FI/period-close",   "tab:T001B",                  EdgeKind::References, 1.0),
            ("help:FI/period-close",   "tab:FAGLFLEXA",              EdgeKind::References, 1.0),
            ("help:MM/goods-movement", "rfc:BAPI_GOODSMVT_CREATE",   EdgeKind::References, 1.0),
        ];
        let mut seen: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();
        for (from, to, kind, weight) in edges {
            let key = (from.to_string(), to.to_string(), kind);
            if seen.insert(key) {
                self.add_edge(Edge {
                    from: from.into(), to: to.into(), kind, weight,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_corpus_has_cross_domain_edges() {
        let g = InMemoryGraph::with_demo_corpus();
        let stats = g.stats();
        assert!(stats.node_count >= 20, "expected >= 20 nodes, got {}", stats.node_count);
        assert!(stats.edge_count >= 25, "expected >= 25 edges, got {}", stats.edge_count);
        // The period_close concept should reach LeanIX FS-12001 in two hops:
        // concept:period_close → tab:FAGLFLEXA ← leanix:FS-12001
        assert!(g.outbound("concept:period_close").iter().any(|(n, _, _)| n == "tab:FAGLFLEXA"));
        assert!(g.inbound("tab:FAGLFLEXA").iter().any(|(n, _, _)| n == "leanix:FS-12001"));
    }

    #[test]
    fn find_seeds_locates_relevant_entities() {
        let g = InMemoryGraph::with_demo_corpus();
        let seeds = g.find_seeds("period close FAGLFLEXA", 5);
        assert!(seeds.iter().any(|s| s == "concept:period_close" || s == "tab:FAGLFLEXA" || s == "help:FI/period-close"));
    }
}
