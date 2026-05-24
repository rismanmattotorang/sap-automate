# SAP-Automate

A Rust-native [Model Context Protocol](https://modelcontextprotocol.io) server,
client, and layered RAG stack for SAP S/4HANA, implementing the architecture
described in *SAP-Automate: An MCP-Native RAG Architecture for SAP S/4HANA*
(ParagonCorp TPO R&D Technical Review, 2026).

This repository ships the **Phase 1 foundation**: a complete MCP protocol
implementation in Rust, a server framework, a client, and two sample
applications. Subsequent phases (knowledge base, hybrid RAG, GraphRAG,
agentic layer) extend this foundation — see [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Quick start

```bash
# Build everything
cargo build --release

# Run the test suite (16 tests across protocol, KB, ingestion, MCP integration)
cargo test --workspace

# Demo 1: spawn the SAP-Automate MCP server and call a tool
cargo run --release -p sample-client -- \
    --server target/release/sap-automate-server \
    --call 'abap.search=query="BAPI_ACC_DOCUMENT_POST",top_k=2' \
    --then 'sap.help.search=query="period close FAGLFLEXA"'

# Demo 2 (Phase 1A): crawl a Help Portal HTML corpus, embed, then search by intent
cargo run --release --bin sap-automate-ingest -- \
    --input-dir ./docs/sample-help-corpus \
    --backend memory --embedder mock --embedding-dim 256 \
    --verify-query "period close foreign currency revaluation"

# Demo 3 (production wiring): run against Qdrant + OpenAI
cargo run --release --bin sap-automate-ingest -- \
    --input-dir ./docs/sample-help-corpus \
    --backend qdrant --qdrant-url http://localhost:6333 \
    --embedder openai --openai-model text-embedding-3-large --embedding-dim 3072 \
    --verify-query "period close foreign currency revaluation"

# Demo 4 (Phase 2): list + read resources + call SAP RFC tools
cargo run --release -p sample-client -- --server target/release/sap-automate-server --list
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --read-resource 'sap-rfc://BAPI_MATERIAL_GET_DETAIL'
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --call 'sap.table.read={"table":"T001","fields":["BUKRS","BUTXT","WAERS"],"where_conditions":["WAERS = '"'"'EUR'"'"'"]}'

# Demo 5: read-only safety gate; flip with --enable-writes
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --call 'sap.rfc.call={"function":"BAPI_ACC_DOCUMENT_POST","parameters":{}}'
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --server-arg=--enable-writes \
    --call 'sap.rfc.call={"function":"BAPI_ACC_DOCUMENT_POST","parameters":{"DOCUMENTHEADER":{}}}'

# Demo 6 (Phase 2 ADT): ABAP source retrieval + impact analysis
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --call 'abap.adt.get_class={"name":"ZCL_FIN_POSTER"}'
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --call 'abap.adt.where_used={"name":"ZIF_FIN_POSTABLE","kind":"interface"}'

# Demo 7 (Phase 2 ADT): BTP data-preview block surfaces fallback advice
cargo run --release -p sample-client -- --server target/release/sap-automate-server \
    --call 'abap.adt.get_table_contents={"table":"BSEG","max_rows":10}'

# Demo 8 (Phase 3): P95 hybrid retrieval acceptance benchmark
cargo run --release --bin sap-automate-bench
# → P95 ~0.16 ms over the pilot corpus (gate: 80 ms). Layer breakdown shows
#   dense+sparse+RRF+rerank costs in μs each.
```

See [`docs/COMPARISON.md`](docs/COMPARISON.md) for the comparative analysis
against `thupalo/sap-rfc-mcp-server`, `CDataSoftware/sap-erp-mcp-server`,
and `SAP/mdk-mcp-server`.

Expected output:

```
== Connected to sap-automate-server v0.1.0 (protocol 2025-06-18)
== Tools (4)
  - abap.search — Hybrid search over the ABAP corpus.
  - bpmn.find_process — Search the Signavio BPMN process repository.
  - eam.search_apps — Search the LeanIX EAM application fact sheets.
  - sap.help.search — Search the SAP Help Portal corpus.

== Calling abap.search with {"query":"material master","top_k":3}
abap.search: 1 hit(s) for "material master"
- [Hybrid] ZMM_GRN_CHECK (1.000) — Function module ZMM_GRN_CHECK reconciles goods receipt with ...
  uri: abap-obj://ZMM/ZMM_GRN_CHECK
```

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Transports: stdio · HTTP+SSE · Streaming HTTP          │  mcp-transport
├─────────────────────────────────────────────────────────┤
│  JSON-RPC 2.0 codec · MCP 2025-06-18 types              │  mcp-core
├─────────────────────────────────────────────────────────┤
│  Capability router · tools/resources/prompts            │  mcp-server, mcp-client
├─────────────────────────────────────────────────────────┤
│  RAG engine: hybrid · GraphRAG · HippoRAG · RAPTOR      │  sap-automate-rag
├─────────────────────────────────────────────────────────┤
│  Knowledge base: Qdrant · Postgres · ArangoDB           │  sap-automate-kb
├─────────────────────────────────────────────────────────┤
│  Connectors: ADT · Signavio · LeanIX                    │  sap-automate-connectors
├─────────────────────────────────────────────────────────┤
│  Agentic: skills · memory · scheduler · channels        │  sap-automate-{skills,memory}
└─────────────────────────────────────────────────────────┘
```

## Crate map

| Crate | Purpose |
|---|---|
| `mcp-core` | JSON-RPC 2.0 + MCP 2025-06-18 types |
| `mcp-transport` | Transport trait + stdio (HTTP transports next) |
| `mcp-server` | Server builder, capability router, dispatch loop |
| `mcp-client` | Async client with request/response correlation |
| `sap-automate-kb` | Document + chunk schema, `KnowledgeStore` trait, InMemory + Qdrant backends |
| `sap-automate-rag` | Layered RAG engine (L2 hybrid now) |
| `sap-automate-ingest` | HTML crawler, chunker, `EmbeddingClient` trait, ingestion pipeline |
| `sap-automate-rfc` | **`SapClient` trait**, MockSapClient, pool, retry, circuit breaker, layered credentials |
| `sap-automate-connectors` | Connector traits (placeholder for Phase 2 finalisation) |
| `sap-automate-skills` | Skill descriptor + loader (Phase 8) |
| `sap-automate-memory` | Four-tier memory model (Phase 8) |
| `apps/sap-automate-server` | Main MCP server binary (stdio); 12 tools, 11 resources, 2 prompts |
| `apps/sap-automate-ingest` | Ingestion CLI (Phase 1A) |
| `apps/sample-server` | Minimal demo server (echo + add) |
| `apps/sample-client` | CLI client that drives any MCP server |

## License

Apache-2.0.
