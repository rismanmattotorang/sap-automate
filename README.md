<div align="center">

# SAP-Automate

**The Rust-native, MCP-native agentic interface for SAP S/4HANA.**

*Sub-millisecond retrieval · 104 SAP-correctness tests · On-premise capable · Apache-2.0*

Built by the **ParagonCorp TPO R&D Team**.

[![CI](https://img.shields.io/badge/CI-passing-22c55e?style=flat-square&logo=githubactions)](.github/workflows/ci.yml)
[![Tests](https://img.shields.io/badge/tests-104%20passing-22d3ee?style=flat-square)](#tests)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-2025--06--18-8b5cf6?style=flat-square)](https://modelcontextprotocol.io)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)

[Why SAP-Automate](#why-sap-automate) · [Quick start](#quick-start) · [Architecture](#architecture) · [Roadmap](docs/ROADMAP.md) · [Whitepaper](docs/SAPAutomate.pdf)

</div>

---

## The problem

SAP S/4HANA powers the financials, supply chains, and HR of a substantial fraction of the global Fortune 500. Yet the gap between *what AI agents can do* and *what they can do against SAP* is enormous: only **3% of SAP customers run SAP Business AI in production**, and **77% of AI-active enterprises rely on non-SAP alternatives** (DSAG Investment Survey 2026). The few open-source MCP servers that bridge AI agents to SAP today are fragmented across vendors, drift from SAP API Hub canon, ship in Python/Node (10–100 ms latency tails), and lack on-premise support.

**SAP-Automate closes that gap.**

## Why SAP-Automate

> *Three things make this a category-of-one tool. Speed. Correctness. Open.*

### 1. **Sub-millisecond retrieval** — 500–5000× under the published gates

| Layer | P95 | Paper acceptance gate | Margin |
|---|---:|---:|---:|
| Hybrid RAG (dense + BM25 + RRF + rerank) | **0.16 ms** | < 80 ms | 500× |
| Multi-hop graph traversal (HippoRAG PPR, 4 hops) | **0.08 ms** | < 400 ms | 5000× |

Measured by `cargo run --release -p sap-automate-bench --graph` on the pilot corpus. The Rust core, the typed `KnowledgeStore` trait, the BM25 implementation with SAP-identifier-preserving tokenisation, and the cross-encoder reranker stage are all in this repository.

### 2. **SAP-correctness verified** — 104 tests including 7 that catch DDIC/BAPI drift in CI

Every BAPI parameter signature is aligned with SAP API Hub canon. Every DDIC table fixture is verified against SE11. Every ADT REST URL pattern is verified against the open-source `mario-andreschak/mcp-abap-adt` source. The precision tests fail loudly if any of those drift:

```rust
every_write_bapi_has_bapiret2_in_tables         // SAP BAPI return contract
every_write_bapi_requires_commit                // No auto-commit; caller must invoke BAPI_TRANSACTION_COMMIT
every_rfc_has_at_least_one_authorization_entry  // S_RFC / S_TABU_DIS / S_CTS_ADMI metadata
every_table_has_client_as_first_key             // MANDT / RCLNT convention
material_number_is_char_40_per_s4hana           // S/4HANA MATN9 conversion
acdoca_is_present_and_marked_as_universal_journal
compatibility_views_carry_s4hana_storage_note   // BSEG / FAGLFLEXA → ACDOCA
```

See [`docs/SAP_CORRECTNESS.md`](docs/SAP_CORRECTNESS.md) for the audit trail.

### 3. **Open and on-premise capable** — no vendor lock-in

| Concern | SAP Joule | CData / commercial MCPs | **SAP-Automate** |
|---|---|---|---|
| License | RISE/GROW only | Commercial | **Apache-2.0** |
| Target systems | S/4HANA cloud only | varies | **ECC 6.0 / S/4HANA / ABAP Cloud** |
| Deployment | Vendor SaaS | Vendor SaaS | **On-prem K8s / Docker / single binary** |
| Cross-domain reasoning | SAP-supplied only | Single-system | **ABAP + RFC + DDIC + BPMN + LeanIX + Help Portal** |
| Customisable agent guardrails | No | No | **AGENTS.md + skills layer** |
| MCP elicitation | No | No | **Yes (2025-06-18 spec, live round-trip)** |

## Quick start

```bash
# Build everything (Rust 1.80+).
cargo build --release --bins --features sap-automate-adt/http

# Run the MCP server over stdio (default).
./target/release/sap-automate-server

# Or over HTTP for remote / web access:
./target/release/sap-automate-server --transport http --bind 127.0.0.1:3030

# Health + metrics:
curl http://127.0.0.1:3030/health     # → "ok"
curl http://127.0.0.1:3030/metrics    # → Prometheus exposition

# Run the operator TUI (Phase 4):
./target/release/sap-automate-tui

# Run the full agentic gateway (Phase 8):
./target/release/sap-automate-gw \
    --server ./target/release/sap-automate-server \
    --scheduler-config ./scheduler.toml \
    --simulate-query "Investigate ATC findings from this week"
```

### Try the web UI

```bash
./target/release/sap-automate-server --transport http --bind 127.0.0.1:3030 &
cd apps/web && npm install && npx next dev
# → http://localhost:3000
```

Five routes: **Operations**, **Query Lab** (live dense + sparse + RRF + reranked side-by-side), **Graph Lab** (HippoRAG / GraphRAG), **Tool Explorer** (schema-driven forms), **Skill Lab**, **Resources**. Screenshots in [`docs/web-screens/`](docs/web-screens/).

### Deploy to Kubernetes

```bash
docker build -t ghcr.io/your-org/sap-automate:$(git rev-parse --short HEAD) -f deploy/Dockerfile .
kubectl apply -k deploy/k8s/
```

See [`deploy/k8s/README.md`](deploy/k8s/README.md) for the full deployment runbook (multi-stage distroless build, External-Secrets / Vault integration, NetworkPolicy hardening, latency-based HPA, PodDisruptionBudget, multi-env overlays).

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  Channels: Teams · Slack · Telegram · WhatsApp · Email · CLI         │  sap-automate-channels
├──────────────────────────────────────────────────────────────────────┤
│  Gateway: intent routing · 4-tier memory · proactive scheduler       │  sap-automate-gw
├──────────────────────────────────────────────────────────────────────┤
│  MCP transports: stdio · HTTP+SSE · Streaming HTTP                   │  mcp-transport
├──────────────────────────────────────────────────────────────────────┤
│  MCP server: 32 tools · 11 resources · 11 prompts · elicitation      │  mcp-server  + apps/sap-automate-server
├──────────────────────────────────────────────────────────────────────┤
│  RAG engine: dense + BM25 + RRF + cross-encoder reranker             │  sap-automate-rag
│  Graph engine: GraphRAG (Louvain) · HippoRAG (PPR) · RAPTOR          │  sap-automate-graph
├──────────────────────────────────────────────────────────────────────┤
│  Knowledge base: in-memory · Qdrant · ArangoDB                       │  sap-automate-kb
│  Ingestion: HTML crawler · contextual chunker · embedding pipeline   │  sap-automate-ingest
├──────────────────────────────────────────────────────────────────────┤
│  SAP backends: SapClient · AdtClient (HTTP + mock)                   │  sap-automate-rfc · sap-automate-adt
│  Credentials: env · keyring · service key (XSUAA-ready)              │
├──────────────────────────────────────────────────────────────────────┤
│  Observability: Prometheus · audit log · OpenTelemetry ready         │  sap-automate-observability
└──────────────────────────────────────────────────────────────────────┘
```

Every layer is a trait-based seam: `KnowledgeStore`, `EmbeddingClient`, `SapClient`, `AdtClient`, `Reranker`, `ChannelAdapter`, `AuditSink`. **Every backend in this matrix is independently replaceable** without touching the server, the client, the tool surface, or the test suite.

## What's inside

**32 production MCP tools** across 5 domains:

| Domain | Tools |
|---|---|
| **RAG search** (5) | `abap.search`, `bpmn.find_process`, `eam.search_apps`, `sap.help.search`, `sap.docs.search` |
| **SAP system / RFC / tables** (10) | `sap.system.info`, `sap.system.health`, `sap.rfc.search`, `sap.rfc.metadata`, `sap.rfc.bulk_metadata`, `sap.rfc.call`, `sap.table.read`, `sap.table.structure`, `sap.bapi.parse_return`, `sap.docs.search` |
| **ABAP ADT** (11) | `abap.adt.get_program`, `…get_class`, `…get_interface`, `…get_include`, `…get_function_module`, `…get_package_contents`, `…get_cds_view`, `…search`, `…where_used`, `…get_table_contents`, `…activate` (write, gated) |
| **Knowledge graph** (4) | `kb.multi_hop` (HippoRAG), `kb.global_query` (GraphRAG), `kb.summarise` (RAPTOR), `kb.graph_neighborhood` |
| **Workflows** (3, write, gated) | `sap.workflow.create_purchase_order`, `sap.workflow.maintain_customer_master`, `sap.workflow.release_transport` |

Plus **11 MCP resources**, **11 MCP prompts** (3 built-in + 8 disk-loaded skills auto-discovered from `./skills/*.md`).

## Production posture

- ✅ **104 tests passing** (unit + 17 ADT integration tests against a mock SAP server + 7 SAP-precision tests + 4 elicitation round-trips + 4 channel/scheduler/memory tests)
- ✅ **Read-only by default**, `--enable-writes` to flip
- ✅ **Structured error taxonomy** mapped to MCP JSON-RPC error codes (transient / permanent / degraded)
- ✅ **AGENTS.md guardrails** loaded from disk; surfaced in `initialize.instructions` and as MCP resource
- ✅ **Prometheus `/metrics`** endpoint with paper §IV-H named series
- ✅ **Audit log** with PII / secret redaction
- ✅ **GitHub Actions CI**: fmt, clippy, test (stable + beta), SAP precision gate, P95 acceptance gate, security audit, Docker build, K8s manifest lint, Next.js web build
- ✅ **Production K8s manifests**: Deployment (3 replicas, distroless, nonroot, read-only rootfs), Service (ClientIP affinity), HPA (3–12), NetworkPolicy (default-deny), PodDisruptionBudget
- 🚧 **Live SAP backend wiring** — `HttpAdtClient` complete (17 integration tests); `NetweaverSapClient` against a real sandbox is the next milestone
- 🚧 **OAuth 2.1 / XSUAA** — service-key model in `AdtAuth`; production flow in P10
- 🚧 **OpenTelemetry OTLP exporter** — tracing spans already structured; OTLP wiring is a one-file change behind a feature flag

## Repository layout

```
sap-automate/
├── crates/                        ← 13 Rust crates
│   ├── mcp-core/                    JSON-RPC 2.0 + MCP 2025-06-18 types
│   ├── mcp-transport/               stdio + HTTP/SSE transports
│   ├── mcp-server/                  capability router + elicitation runtime
│   ├── mcp-client/                  async client + ElicitationDelegate
│   ├── sap-automate-rfc/            SapClient + RFC catalogue + BAPIRET2 parser
│   ├── sap-automate-adt/            AdtClient (HTTP + mock; CSRF cache)
│   ├── sap-automate-kb/             KB schema + InMemory + Qdrant
│   ├── sap-automate-rag/            Hybrid RAG + reranker + graph layers
│   ├── sap-automate-graph/          Entities + Louvain + PPR + RAPTOR
│   ├── sap-automate-ingest/         Crawler + chunker + embedder
│   ├── sap-automate-memory/         Working + episodic four-tier memory
│   ├── sap-automate-scheduler/      TOML-declared proactive jobs
│   ├── sap-automate-channels/       Teams / Slack / Telegram / CLI adapters
│   ├── sap-automate-skills/         AGENTS.md-style skill loader
│   └── sap-automate-observability/  Prometheus metrics + audit log + tracing
├── apps/                          ← 7 binaries
│   ├── sap-automate-server/         the MCP server (stdio + HTTP)
│   ├── sap-automate-gw/             multi-channel agentic gateway
│   ├── sap-automate-tui/            Ratatui operator console
│   ├── sap-automate-ingest/         knowledge ingestion CLI
│   ├── sap-automate-bench/          P95 acceptance harness
│   ├── sample-server/               minimal echo+add MCP server
│   ├── sample-client/               CLI MCP client
│   └── web/                         Next.js 14 web UI
├── skills/                        ← 8 auto-loaded agentic skills
├── deploy/                        ← Dockerfile + K8s manifests + runbook
├── docs/                          ← SAPAutomate.pdf, ROADMAP, SAP_CORRECTNESS, COMPARISON
└── .github/workflows/             ← CI + release
```

## Documentation

| Document | What |
|---|---|
| [`docs/SAPAutomate.pdf`](docs/SAPAutomate.pdf) | The ParagonCorp TPO R&D whitepaper — full architectural specification |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Phased delivery plan with current status per phase |
| [`docs/SAP_CORRECTNESS.md`](docs/SAP_CORRECTNESS.md) | Every fixture mapped to its SAP source-of-truth |
| [`docs/COMPARISON.md`](docs/COMPARISON.md) | Side-by-side analysis vs 6 reference SAP MCP servers |
| [`deploy/k8s/README.md`](deploy/k8s/README.md) | Production deployment runbook |
| [`AGENTS.md`](AGENTS.md) | Default agent guardrails (per-deployment overridable) |

## Tests

```bash
cargo test --workspace --features sap-automate-adt/http
# → 104 tests passing
```

Test coverage spans **104 unit + integration + acceptance tests** across:

- **Protocol** (JSON-RPC framing, MCP 2025-06-18 handshake, elicitation round-trip)
- **SAP correctness** (BAPI signatures, DDIC invariants, MANDT/RCLNT first-key, S/4HANA-storage notes)
- **ADT integration** (17 axum-fixture tests exercising every HttpAdtClient path: URL patterns, headers, CSRF flow, XML parsers, error mapping)
- **RAG pipeline** (BM25, RRF fusion, reranker promotion, contextual enrichment)
- **Graph** (Louvain modularity, PPR convergence, RAPTOR levels)
- **Agentic** (memory tiers, scheduler cadence, channel routing)
- **Observability** (Prometheus rendering, audit redaction)

## Credits

SAP-Automate is built and maintained by the **ParagonCorp Technology Product Owner R&D team**. The architecture is documented in *SAP-Automate: An MCP-Native RAG Architecture for SAP S/4HANA* ([whitepaper](docs/SAPAutomate.pdf)), ParagonCorp Technical Review Vol. 1 No. 1 (2026).

Reference designs studied while building this:

- [`thupalo/sap-rfc-mcp-server`](https://github.com/thupalo/sap-rfc-mcp-server) — connection pooling + metadata cache patterns
- [`CDataSoftware/sap-erp-mcp-server-by-cdata`](https://github.com/CDataSoftware/sap-erp-mcp-server-by-cdata) — read-only-by-default safety property
- [`SAP/mdk-mcp-server`](https://github.com/SAP/mdk-mcp-server) — AGENTS.md + constrained-enum tool params
- [`mario-andreschak/mcp-abap-adt`](https://github.com/mario-andreschak/mcp-abap-adt) — ADT REST URL canon
- [`fr0ster/mcp-abap-adt`](https://github.com/fr0ster/mcp-abap-adt) — handler-exposure groups + multi-transport
- [`marianfoo/sap-ai-mcp-servers`](https://github.com/marianfoo/sap-ai-mcp-servers) — 40+ server meta-registry, skills-layer convergence

## License

[Apache-2.0](LICENSE). Use it, fork it, build a business on top of it.

---

<div align="center">

**ParagonCorp** · TPO R&D · 2026
*Reference design: PC-TR-2026-SAP-AUTOMATE-01*

</div>
