//! SAP-Automate cross-domain knowledge graph.
//!
//! Implements the substrate that paper §VII-F (GraphRAG), §VII-G
//! (HippoRAG), and §VII-E (RAPTOR) all sit on top of:
//!
//!   - Typed entities spanning ABAP / RFC / table / BPMN / LeanIX / Help
//!   - Typed edges (calls, implements, reads_table, references, etc.)
//!   - **Louvain-style community detection** for GraphRAG (paper §VII-F)
//!   - **Personalised PageRank** for HippoRAG multi-hop traversal
//!     (paper §VII-G).  Implements the seeds-and-restart formulation
//!     from the HippoRAG paper.
//!   - **RAPTOR-style hierarchical clusters** over chunks for
//!     multi-granularity retrieval (paper §VII-E).
//!
//! Backend abstraction: every analytical method takes `&InMemoryGraph` for
//! now.  An `ArangoGraph` (paper §VIII-C) drops in behind a future
//! `GraphStore` trait without touching callers.

pub mod community;
pub mod entity;
pub mod ppr;
pub mod raptor;
pub mod store;

pub use community::{Communities, Community, detect_communities};
pub use entity::{Edge, EdgeKind, Entity, EntityKind, NodeId};
pub use ppr::{personalised_pagerank, PprConfig, PprResult, multi_hop_search};
pub use raptor::{RaptorTree, RaptorLevel, RaptorNode, build_raptor_tree};
pub use store::{InMemoryGraph, GraphStats};
