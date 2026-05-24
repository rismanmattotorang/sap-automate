//! Phase 3 acceptance harness.
//!
//! Ingests a Help corpus, runs N queries through the hybrid RAG engine
//! (dense + sparse + RRF + rerank), and reports P50 / P95 / P99 latency.
//!
//! Paper §X-D gate: P95 hybrid retrieval < 80 ms over the pilot corpus.

use clap::Parser;
use sap_automate_graph::InMemoryGraph;
use sap_automate_ingest::{EmbeddingClient, HelpPortalCrawler, IngestionPipeline, MockEmbedder};
use sap_automate_kb::{InMemoryKb, KnowledgeStore};
use sap_automate_rag::{GraphEngine, MockReranker, Query, RagEngine};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "sap-automate-bench", about = "Phase 3 acceptance harness: P95 retrieval < 80 ms")]
struct Cli {
    /// Directory of *.html files to ingest.
    #[arg(long, default_value = "./docs/sample-help-corpus")]
    input_dir: String,

    /// Number of queries to run.
    #[arg(long, default_value_t = 1000)]
    n: usize,

    /// Embedding vector dimension.
    #[arg(long, default_value_t = 256)]
    embedding_dim: usize,

    /// Disable the reranker stage (baseline measurement).
    #[arg(long)]
    no_rerank: bool,

    /// Disable contextual enrichment during ingestion (baseline).
    #[arg(long)]
    no_contextual_enrichment: bool,

    /// Top-K candidates returned.
    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// Acceptance gate (ms).  Process exits non-zero if P95 exceeds it.
    #[arg(long, default_value_t = 80)]
    gate_p95_ms: u64,

    /// Also benchmark the Phase 5A graph layer (HippoRAG multi-hop).
    /// Paper §X-H gate: P95 < 400 ms for ≤4-hop queries.
    #[arg(long)]
    graph: bool,

    /// Graph multi-hop P95 gate (ms).  Paper §X-H default.
    #[arg(long, default_value_t = 400)]
    graph_gate_p95_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .init();

    let cli = Cli::parse();

    // --- Set up pipeline --------------------------------------------------
    let store: Arc<dyn KnowledgeStore> = Arc::new(InMemoryKb::new());
    let embedder: Arc<dyn EmbeddingClient> = Arc::new(MockEmbedder::new(cli.embedding_dim));

    let mut chunker_cfg = sap_automate_ingest::ChunkerConfig::default();
    if cli.no_contextual_enrichment {
        chunker_cfg.contextual_enrichment = false;
    }

    let pipeline = IngestionPipeline::new(Arc::clone(&embedder), Arc::clone(&store))
        .with_chunker(chunker_cfg);
    let docs = HelpPortalCrawler::new().crawl_directory(&cli.input_dir).await?;
    println!("→ Ingesting {} document(s) from {}", docs.len(), cli.input_dir);
    let report = pipeline.ingest(docs).await?;
    println!("→ {} document(s), {} chunk(s) indexed", report.documents, report.chunks);

    // --- Build engine -----------------------------------------------------
    let mut engine = RagEngine::new(Arc::clone(&store));
    if !cli.no_rerank {
        engine = engine.with_reranker(Arc::new(MockReranker::new()));
    }

    // --- Generate workload -----------------------------------------------
    let workload = workload_queries();
    let mut samples: Vec<u64> = Vec::with_capacity(cli.n);
    let mut layer_samples: Vec<sap_automate_rag::LatencyBreakdown> = Vec::with_capacity(cli.n);

    println!("\n→ Running {} queries ...", cli.n);
    let bench_start = Instant::now();
    for i in 0..cli.n {
        let query_text = &workload[i % workload.len()];
        let qv = embedder.embed(&[query_text.clone()]).await?.into_iter().next();
        let t0 = Instant::now();
        let resp = engine.hybrid_search(Query {
            text: query_text,
            domain: None,
            top_k: cli.top_k,
            embedding: qv,
        }).await?;
        samples.push(t0.elapsed().as_micros() as u64);
        layer_samples.push(resp.latency.clone());
    }
    let wall = bench_start.elapsed();

    // --- Report ----------------------------------------------------------
    samples.sort_unstable();
    let n = samples.len();
    let p = |q: f64| samples[(((n as f64) * q) as usize).min(n - 1)];
    let mean: u64 = samples.iter().sum::<u64>() / n as u64;

    let layer_mean = |f: fn(&sap_automate_rag::LatencyBreakdown) -> u64| -> u64 {
        layer_samples.iter().map(f).sum::<u64>() / n as u64
    };

    println!("\n== Latency over {n} queries ({:.2}s wall, {:.0} q/s)",
        wall.as_secs_f64(), n as f64 / wall.as_secs_f64());
    println!("  Reranker:    {}", if cli.no_rerank { "off" } else { "MockReranker" });
    println!("  Enrichment:  {}", if cli.no_contextual_enrichment { "off" } else { "on" });
    println!("  P50 (median): {:>7} µs   ({:.3} ms)", p(0.50), p(0.50) as f64 / 1000.0);
    println!("  P95:          {:>7} µs   ({:.3} ms)", p(0.95), p(0.95) as f64 / 1000.0);
    println!("  P99:          {:>7} µs   ({:.3} ms)", p(0.99), p(0.99) as f64 / 1000.0);
    println!("  Max:          {:>7} µs   ({:.3} ms)", *samples.last().unwrap(), *samples.last().unwrap() as f64 / 1000.0);
    println!("  Mean:         {:>7} µs   ({:.3} ms)", mean, mean as f64 / 1000.0);
    println!("\n  Layer breakdown (mean):");
    println!("    dense  : {:>5} µs", layer_mean(|l| l.dense_us));
    println!("    sparse : {:>5} µs", layer_mean(|l| l.sparse_us));
    println!("    fusion : {:>5} µs", layer_mean(|l| l.fusion_us));
    println!("    rerank : {:>5} µs", layer_mean(|l| l.rerank_us));

    let p95_ms = p(0.95) as f64 / 1000.0;
    let gate = cli.gate_p95_ms as f64;
    if p95_ms <= gate {
        println!("\n✓ Phase 3 ACCEPTANCE GATE PASSED: P95 = {p95_ms:.3} ms ≤ {gate:.0} ms");
    } else {
        println!("\n✗ Phase 3 ACCEPTANCE GATE FAILED: P95 = {p95_ms:.3} ms > {gate:.0} ms");
        return Err(anyhow::anyhow!("P95 gate failed"));
    }

    // ---------------------------------------------------------------------
    // Phase 5A graph bench (paper §X-H acceptance gate).
    // ---------------------------------------------------------------------
    if cli.graph {
        println!("\n→ Phase 5A graph bench: HippoRAG multi-hop P95 < {} ms gate", cli.graph_gate_p95_ms);
        let kg = Arc::new(InMemoryGraph::with_demo_corpus());
        let engine = GraphEngine::new(kg);
        println!("→ Graph: {} nodes, {} edges, {} communities",
            engine.graph.stats().node_count,
            engine.graph.stats().edge_count,
            engine.communities.communities.len(),
        );
        let graph_workload = graph_workload_queries();
        let mut g_samples: Vec<u64> = Vec::with_capacity(cli.n);
        let g_start = Instant::now();
        for i in 0..cli.n {
            let q = &graph_workload[i % graph_workload.len()];
            let t0 = Instant::now();
            let _ = engine.multi_hop(q, 4, 8, 3);
            g_samples.push(t0.elapsed().as_micros() as u64);
        }
        let g_wall = g_start.elapsed();
        g_samples.sort_unstable();
        let gn = g_samples.len();
        let gp = |q: f64| g_samples[(((gn as f64) * q) as usize).min(gn - 1)];
        let gmean: u64 = g_samples.iter().sum::<u64>() / gn as u64;
        println!("\n== Multi-hop latency over {gn} queries ({:.2}s wall, {:.0} q/s)",
            g_wall.as_secs_f64(), gn as f64 / g_wall.as_secs_f64());
        println!("  P50: {:>7} µs   ({:.3} ms)", gp(0.50), gp(0.50) as f64 / 1000.0);
        println!("  P95: {:>7} µs   ({:.3} ms)", gp(0.95), gp(0.95) as f64 / 1000.0);
        println!("  P99: {:>7} µs   ({:.3} ms)", gp(0.99), gp(0.99) as f64 / 1000.0);
        println!("  Max: {:>7} µs   ({:.3} ms)", *g_samples.last().unwrap(), *g_samples.last().unwrap() as f64 / 1000.0);
        println!("  Mean: {:>5} µs   ({:.3} ms)", gmean, gmean as f64 / 1000.0);
        let gp95_ms = gp(0.95) as f64 / 1000.0;
        let ggate = cli.graph_gate_p95_ms as f64;
        if gp95_ms <= ggate {
            println!("\n✓ Phase 5A ACCEPTANCE GATE PASSED: P95 = {gp95_ms:.3} ms ≤ {ggate:.0} ms");
        } else {
            println!("\n✗ Phase 5A ACCEPTANCE GATE FAILED: P95 = {gp95_ms:.3} ms > {ggate:.0} ms");
            return Err(anyhow::anyhow!("graph P95 gate failed"));
        }
    }

    Ok(())
}

/// Realistic multi-hop / impact-analysis queries.
fn graph_workload_queries() -> Vec<String> {
    vec![
        "impact of changing BAPI_ACC_DOCUMENT_POST".into(),
        "where is FAGLFLEXA used".into(),
        "callers of ZFIN_POST_JE".into(),
        "downstream from ZIF_FIN_POSTABLE".into(),
        "trace from period_close to LeanIX applications".into(),
        "what depends on table T001B".into(),
        "objects that touch goods movement".into(),
        "what does the O2C process call".into(),
        "from ZMM_GRN_CHECK to material master".into(),
        "S/4HANA Finance dependencies".into(),
    ]
}

/// A mix of realistic intent queries spanning the four corpus domains.
fn workload_queries() -> Vec<String> {
    vec![
        "period close foreign currency revaluation".into(),
        "open and close posting periods T001B".into(),
        "goods movement type 101 receipt".into(),
        "MIGO transaction reverse posting".into(),
        "VF01 billing document creation invoice".into(),
        "billing pricing procedure KONP KONV".into(),
        "payroll wage type PA0008 PA0014".into(),
        "PC00_M99_CALC payroll cluster B2".into(),
        "BAPI_ACC_DOCUMENT_POST journal entry".into(),
        "FAGLFLEXA BSEG reconciliation".into(),
        "material master MARA MTART".into(),
        "ATC quality scan ABAP test cockpit".into(),
        "what is the difference between FB60 and FB70".into(),
        "MIGO movement types 122 return delivery".into(),
        "general ledger account determination posting".into(),
        "company code customising T001 currency".into(),
    ]
}
