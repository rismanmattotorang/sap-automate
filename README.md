<div align="center">

# SAP-Automate

### The agentic OS for SAP — built in Rust, on-premise by default.

**Sub-millisecond retrieval. 145 SAP-correctness tests. Apache-2.0.**
**Made by [ParagonCorp](#about-paragoncorp).**

[![CI](https://img.shields.io/badge/CI-passing-22c55e?style=flat-square&logo=githubactions)](.github/workflows/ci.yml)
[![Tests](https://img.shields.io/badge/tests-145%20passing-22d3ee?style=flat-square)](#tests)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-2025--06--18-8b5cf6?style=flat-square)](https://modelcontextprotocol.io)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)

[**Quick start →**](#quick-start) · [Why it exists](#why-paragoncorp-built-this) · [What you can do](#what-you-can-do-with-it) · [Architecture](#architecture) · [Roadmap](docs/ROADMAP.md) · [Whitepaper](docs/SAPAutomate.pdf)

</div>

---

## Quick start

```bash
# Build everything (Rust 1.80+).
cargo build --release --bins --features sap-automate-adt/http

# Single binary, stdio MCP server — drop into Claude Code, Cursor, or any MCP client.
./target/release/sap-automate-server

# Or HTTP for browser / remote agents.
./target/release/sap-automate-server --transport http --bind 127.0.0.1:3030
curl http://127.0.0.1:3030/health      # → "ok"
curl http://127.0.0.1:3030/metrics     # → Prometheus exposition

# Ratatui operator console.
./target/release/sap-automate-tui

# Full multi-channel agentic gateway.
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

Five routes: **Operations** (live latency budget + RFC metadata cache), **Query Lab** (dense + sparse + RRF + reranked side-by-side), **Graph Lab** (HippoRAG / GraphRAG), **Tool Explorer** (schema-driven forms), **Skill Lab**, **Resources**. Screenshots in [`docs/web-screens/`](docs/web-screens/).

### Deploy to Kubernetes

```bash
docker build -t ghcr.io/your-org/sap-automate:$(git rev-parse --short HEAD) -f deploy/Dockerfile .
kubectl apply -k deploy/k8s/
```

Production-grade manifests live in [`deploy/k8s/`](deploy/k8s/README.md): 3-replica Deployment on distroless + nonroot, ClientIP-affinity Service, latency-based HPA (3–12), default-deny NetworkPolicy, PodDisruptionBudget, Kustomize overlays.

---

## Why ParagonCorp built this

SAP S/4HANA runs the financials, supply chains, and HR of a substantial slice of the Fortune 500. But the gap between *what AI agents can do generally* and *what they can do against SAP* is enormous:

- **3% of SAP customers run SAP Business AI in production.**
- **77% of AI-active enterprises rely on non-SAP alternatives.**
- (DSAG Investment Survey 2026.)

The open-source SAP MCP servers that exist today are fragmented across vendors, drift from SAP API Hub canon, ship in Python / Node with 10–100 ms latency tails, and quietly require cloud-only deployment.

**That's the gap. SAP-Automate closes it — on-prem, in Rust, with the correctness story written down in tests.**

ParagonCorp is the customer that needed this. We built it for our own SAP estate first, then open-sourced it because the cost of fragmentation is too high to bear alone.

---

## What you can do with it

| You are… | …and you can | The tool |
|---|---|---|
| an FI ops lead | ask "why didn't period 2026-M03 close?" — get a cited answer with the right `ACDOCA` excerpt and the failing `BAPI_TRANSACTION_COMMIT` | `sap.docs.search` → `sap.rfc.metadata` → `sap.rfc.call` |
| an ABAP developer | review a class, list every caller across packages, see DDIC dependencies before activating | `abap.adt.get_class` → `abap.adt.where_used` → `abap.adt.activate` |
| a basis admin | get live cache hit-ratio, transport impact summaries, ATC findings from the last hour — pushed to Teams / Slack / Telegram | `sap-automate-gw` channels + scheduler |
| an enterprise architect | ask cross-domain questions ("which Fiori apps depend on `FAGLFLEXA`?") and get a multi-hop traversal across ABAP + BPMN + LeanIX | `kb.multi_hop` (HippoRAG PPR) |
| a Clean Core auditor | navigate a 200-page SAP Help page section-by-section instead of similarity-blind | `sap.kb.navigate` (hierarchical doc tree) |
| a basis sec lead | run a read-only segregation-of-duties review against `USR02` / `AGR_*` / `RFCDES` | `sap.skill.security_sod_audit` |

---

## Where it leads on speed, correctness, and openness

> **Three claims. One repo to verify them.**

### 1. Sub-millisecond retrieval — 500–5000× under the published gates

| Layer | P95 | Acceptance gate | Margin |
|---|---:|---:|---:|
| Hybrid RAG (dense + BM25 + RRF + rerank) | **0.16 ms** | < 80 ms | **500×** |
| Multi-hop graph traversal (HippoRAG PPR, 4 hops) | **0.08 ms** | < 400 ms | **5000×** |

Measured by `cargo run --release -p sap-automate-bench --graph` on the pilot corpus. The Rust core, the typed `KnowledgeStore` trait, the BM25 implementation with SAP-identifier-preserving tokenisation, and the cross-encoder reranker stage are all in this repository.

### 2. SAP correctness — verified by 7 precision tests in CI

Every BAPI parameter signature is aligned with the SAP API Hub canon. Every DDIC fixture is verified against SE11. Every ADT REST URL is verified against the open-source ADT reference clients. The precision tests fail loudly the second any of those drift:

```rust
every_write_bapi_has_bapiret2_in_tables         // BAPI return contract
every_write_bapi_requires_commit                // No auto-commit; caller must invoke BAPI_TRANSACTION_COMMIT
every_rfc_has_at_least_one_authorization_entry  // S_RFC / S_TABU_DIS / S_CTS_ADMI
every_table_has_client_as_first_key             // MANDT / RCLNT convention
material_number_is_char_40_per_s4hana           // MATN9 conversion
acdoca_is_present_and_marked_as_universal_journal
compatibility_views_carry_s4hana_storage_note   // BSEG / FAGLFLEXA → ACDOCA
```

Full audit trail: [`docs/SAP_CORRECTNESS.md`](docs/SAP_CORRECTNESS.md).

### 3. Open and on-premise capable — no vendor lock-in

| Concern | SAP Joule | CData / commercial MCPs | **SAP-Automate** |
|---|---|---|---|
| License | RISE/GROW only | Commercial | **Apache-2.0** |
| Target systems | S/4HANA cloud only | varies | **ECC 6.0 / S/4HANA / ABAP Cloud** |
| Deployment | Vendor SaaS | Vendor SaaS | **On-prem K8s / Docker / single binary** |
| Cross-domain reasoning | SAP-supplied only | Single-system | **ABAP + RFC + DDIC + BPMN + LeanIX + Help Portal** |
| Customisable guardrails | No | No | **AGENTS.md + skills layer** |
| MCP elicitation | No | No | **Yes (2025-06-18 spec, live round-trip)** |

---

## What ships in this repo

**35 production MCP tools** across 5 domains:

| Domain | Tools |
|---|---|
| **RAG search** (6) | `abap.search`, `bpmn.find_process`, `eam.search_apps`, `sap.help.search`, `sap.docs.search`, `sap.kb.navigate` (hierarchical document-tree walker) |
| **SAP system / RFC / tables** (12) | `sap.system.info`, `sap.system.health`, `sap.system.cache_stats`, `sap.system.cache_invalidate`, `sap.rfc.search`, `sap.rfc.metadata`, `sap.rfc.bulk_metadata`, `sap.rfc.call`, `sap.table.read`, `sap.table.structure`, `sap.bapi.parse_return`, `sap.docs.search` |
| **ABAP ADT** (11) | `abap.adt.get_program`, `…get_class`, `…get_interface`, `…get_include`, `…get_function_module`, `…get_package_contents`, `…get_cds_view`, `…search`, `…where_used`, `…get_table_contents`, `…activate` (write, gated) |
| **Knowledge graph** (4) | `kb.multi_hop` (HippoRAG), `kb.global_query` (GraphRAG), `kb.summarise` (RAPTOR), `kb.graph_neighborhood` |
| **Workflows** (3, write, gated) | `sap.workflow.create_purchase_order`, `sap.workflow.maintain_customer_master`, `sap.workflow.release_transport` |

Plus **12 MCP resources** (`sap-system://info`, `sap-rfc://…`, `sap-table://…`, `adt-destination://info`, `sap-cache://stats`, `agents://guardrails`) and **16 MCP prompts** (3 built-in + 13 disk-loaded skills auto-discovered from `./skills/*.md`).

### Bundled skills

13 declarative workflow templates ship in [`./skills/`](skills/). Each is a markdown file with YAML frontmatter; the server auto-loads them and exposes each as an MCP prompt.

| Skill | What it captures |
|---|---|
| `sap.skill.period_close_investigation` | Root-cause an FI period-close failure |
| `sap.skill.transport_impact_analysis` | Enumerate impacted callers before releasing a transport |
| `sap.skill.transport_release_elicit` | Re-typed-confirmation workflow for production transport releases |
| `sap.skill.rap_service_scaffolding` | Generate a RAP service definition + behavior |
| `sap.skill.abap_code_review` | SAP-specific ABAP code review (Clean Core, no-DELETE-without-WHERE, etc.) |
| `sap.skill.clean_core_audit` | Find Z-namespace objects that modify SAP-standard tables |
| `sap.skill.po_creation_elicit` · `sap.skill.customer_master_elicit` | Two-step elicitation flows for high-stakes writes |
| `sap.skill.odata_service_design` | Design discipline for exposing a new OData service as MCP tools |
| `sap.skill.security_sod_audit` | Read-only segregation-of-duties review across `USR02` / `AGR_*` / `RFCDES` |
| `sap.skill.bw_to_datasphere_migration` | BW-to-Datasphere modernisation classification matrix + 3-wave plan |
| `sap.skill.aipnv_ai_pairing` | Anti-autopilot five-question checklist for write-side calls |

Drop a markdown file into `./skills/`, restart the server, and it becomes an invokable MCP prompt.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  Channels: Teams · Slack · Telegram · WhatsApp · Email · CLI         │  sap-automate-channels
├──────────────────────────────────────────────────────────────────────┤
│  Gateway: intent routing · 4-tier memory · proactive scheduler       │  sap-automate-gw
├──────────────────────────────────────────────────────────────────────┤
│  MCP transports: stdio · HTTP+SSE · Streaming HTTP                   │  mcp-transport
├──────────────────────────────────────────────────────────────────────┤
│  MCP server: 35 tools · 12 resources · 16 prompts · elicitation      │  mcp-server  + apps/sap-automate-server
├──────────────────────────────────────────────────────────────────────┤
│  RAG engine: dense + BM25 + RRF + cross-encoder reranker             │  sap-automate-rag
│  Graph engine: GraphRAG (Louvain) · HippoRAG (PPR) · RAPTOR          │  sap-automate-graph
├──────────────────────────────────────────────────────────────────────┤
│  Knowledge base: in-memory · Qdrant · ArangoDB · DocumentTree        │  sap-automate-kb
│  Ingestion: HTML crawler · contextual chunker · embedding pipeline   │  sap-automate-ingest
│             robots.txt · per-host rate-limit · fit-markdown filter   │
├──────────────────────────────────────────────────────────────────────┤
│  SAP backends: SapClient · AdtClient (HTTP + mock) · MetadataCache   │  sap-automate-rfc · sap-automate-adt
│  Credentials: env · keyring · service key (XSUAA-ready)              │
├──────────────────────────────────────────────────────────────────────┤
│  Observability: Prometheus · audit log · OpenTelemetry ready         │  sap-automate-observability
└──────────────────────────────────────────────────────────────────────┘
```

Every layer is a trait-based seam: `KnowledgeStore`, `EmbeddingClient`, `SapClient`, `AdtClient`, `Reranker`, `ChannelAdapter`, `AuditSink`. **Every backend in this matrix is independently replaceable** without touching the server, the client, the tool surface, or the test suite.

---

## Production posture

- ✅ **145 tests passing** across protocol conformance, SAP correctness, ADT integration, RAG, graph, agentic, observability, KB, and crawler hardening
- ✅ **Read-only by default**, `--enable-writes` to flip
- ✅ **Structured error taxonomy** mapped to MCP JSON-RPC error codes (transient / permanent / degraded)
- ✅ **AGENTS.md guardrails** loaded from disk; surfaced in `initialize.instructions` and as MCP resource
- ✅ **Prometheus `/metrics`** endpoint with paper §IV-H named series
- ✅ **Audit log** with PII / secret redaction
- ✅ **GitHub Actions CI**: fmt, clippy, test (stable + beta), SAP precision gate, P95 acceptance gate, security audit, Docker build, K8s manifest lint, Next.js web build
- ✅ **Production K8s manifests**: Deployment (3 replicas, distroless, nonroot, read-only rootfs), Service (ClientIP affinity), HPA (3–12), NetworkPolicy (default-deny), PodDisruptionBudget
- 🚧 **Live SAP backend wiring** — `HttpAdtClient` complete (17 integration tests); `NetweaverSapClient` against a real sandbox is the next milestone
- 🚧 **OAuth 2.1 / XSUAA** — service-key model in `AdtAuth`; production flow in v1.2
- 🚧 **OpenTelemetry OTLP exporter** — tracing spans already structured; OTLP wiring is a one-file change behind a feature flag

---

## Repository layout

```
sap-automate/
├── crates/                        ← 16 Rust crates
│   ├── mcp-core/                    JSON-RPC 2.0 + MCP 2025-06-18 types
│   ├── mcp-transport/               stdio + HTTP/SSE transports
│   ├── mcp-server/                  capability router + elicitation runtime
│   ├── mcp-client/                  async client + ElicitationDelegate
│   ├── sap-automate-rfc/            SapClient + RFC catalogue + BAPIRET2 parser + MetadataCache (TTL)
│   ├── sap-automate-adt/            AdtClient (HTTP + mock; CSRF cache)
│   ├── sap-automate-kb/             KB schema + InMemory + Qdrant + DocumentTree
│   ├── sap-automate-rag/            Hybrid RAG + reranker + graph layers + RetrievalDiagnostics
│   ├── sap-automate-graph/          Entities + Louvain + PPR + RAPTOR
│   ├── sap-automate-ingest/         Crawler + chunker + embedder + robots.txt + rate-limit + fit-markdown
│   ├── sap-automate-memory/         Working + episodic four-tier memory
│   ├── sap-automate-scheduler/      TOML-declared proactive jobs
│   ├── sap-automate-channels/       Teams / Slack / Telegram / CLI adapters
│   ├── sap-automate-skills/         AGENTS.md-style skill loader
│   └── sap-automate-observability/  Prometheus metrics + audit log + tracing
├── apps/                          ← 7 Rust binaries + Next.js web UI
│   ├── sap-automate-server/         the MCP server (stdio + HTTP)
│   ├── sap-automate-gw/             multi-channel agentic gateway
│   ├── sap-automate-tui/            Ratatui operator console
│   ├── sap-automate-ingest/         knowledge ingestion CLI
│   ├── sap-automate-bench/          P95 acceptance harness
│   ├── sample-server/               minimal echo+add MCP server
│   ├── sample-client/               CLI MCP client
│   └── web/                         Next.js 14 web UI
├── skills/                        ← 13 auto-loaded agentic skills
├── deploy/                        ← Dockerfile + K8s manifests + runbook
├── docs/                          ← SAPAutomate.pdf, ROADMAP, SAP_CORRECTNESS, COMPARISON
└── .github/workflows/             ← CI + release
```

---

## Documentation

| Document | What |
|---|---|
| [`docs/SAPAutomate.pdf`](docs/SAPAutomate.pdf) | The ParagonCorp whitepaper — full architectural specification |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Phased delivery plan with current status per release |
| [`docs/SAP_CORRECTNESS.md`](docs/SAP_CORRECTNESS.md) | Every fixture mapped to its SAP source-of-truth |
| [`docs/COMPARISON.md`](docs/COMPARISON.md) | Side-by-side analysis vs reference SAP MCP servers |
| [`deploy/k8s/README.md`](deploy/k8s/README.md) | Production deployment runbook |
| [`AGENTS.md`](AGENTS.md) | Default agent guardrails (per-deployment overridable) |
| [`CHANGELOG.md`](CHANGELOG.md) | Release history |

---

## Tests

```bash
cargo test --workspace --features sap-automate-adt/http
# → 145 tests passing
```

Coverage spans:

- **Protocol** — JSON-RPC framing, MCP 2025-06-18 handshake, elicitation round-trip
- **SAP correctness** — BAPI signatures, DDIC invariants, MANDT/RCLNT first-key, S/4HANA-storage notes (7 precision tests in CI)
- **ADT integration** — 17 axum-fixture tests exercising every `HttpAdtClient` path: URL patterns, headers, CSRF flow, XML parsers, error mapping
- **RAG pipeline** — BM25, RRF fusion, reranker promotion, contextual enrichment, retrieval diagnostics
- **Knowledge base** — content-hash dedup, hierarchical document tree builder
- **Crawler** — robots.txt parser (RFC 9309 subset), per-host token-bucket rate-limiter, BM25 fit-markdown filter
- **Graph** — Louvain modularity, PPR convergence, RAPTOR levels
- **Agentic** — memory tiers, scheduler cadence, channel routing, skill-aware gateway routing
- **Observability** — Prometheus rendering, audit redaction
- **Server-binary integration** — in-process via `tokio::io::duplex` for the cache and KB-navigate tool surfaces

---

## About ParagonCorp

**ParagonCorp** is an Indonesia-based enterprise that runs a large SAP S/4HANA estate across consumer goods, retail, and manufacturing operations. The TPO (Technology Product Owner) R&D team builds and operates the AI tooling that ParagonCorp's own SAP organisation depends on every day.

We built SAP-Automate because the existing options didn't fit:

- **Joule / SAP Business AI** assumes RISE / GROW — we run on-prem with sovereignty obligations.
- **Commercial MCP servers** are per-seat and closed.
- **Open-source MCP servers** are fragmented across vendors and ship in stacks that don't meet our latency budgets.

So we built our own, then released it under Apache-2.0 — because the cost of being the only customer of a tool this complex is too high. The architecture is documented in *SAP-Automate: An MCP-Native RAG Architecture for SAP S/4HANA* ([whitepaper](docs/SAPAutomate.pdf)), ParagonCorp Technical Review Vol. 1 No. 1 (2026).

**We're hiring.** If you want to work on Rust + SAP + agentic systems at production scale, reach out at `tpo-research@paracorpgroup.com`.

---

## Reference designs studied while building this

- [`VectifyAI/OpenKB`](https://github.com/VectifyAI/OpenKB) + [`VectifyAI/PageIndex`](https://github.com/VectifyAI/PageIndex) — hierarchical document-tree pattern; informs `sap_automate_kb::DocumentTree` + the `sap.kb.navigate` MCP tool
- [`unclecode/crawl4ai`](https://github.com/unclecode/crawl4ai) — robots.txt respect, per-host rate limiting, BM25-based fit-markdown boilerplate filter; informs `sap_automate_ingest::{robots, rate_limit, fit_markdown}`
- [`thupalo/sap-rfc-mcp-server`](https://github.com/thupalo/sap-rfc-mcp-server) — connection pooling + metadata cache patterns; informs `MetadataCache` TTL decorator
- [`CDataSoftware/sap-erp-mcp-server-by-cdata`](https://github.com/CDataSoftware/sap-erp-mcp-server-by-cdata) — read-only-by-default safety property
- [`SAP/mdk-mcp-server`](https://github.com/SAP/mdk-mcp-server) — AGENTS.md + constrained-enum tool parameters
- [`mario-andreschak/mcp-abap-adt`](https://github.com/mario-andreschak/mcp-abap-adt) — ADT REST URL canon
- [`fr0ster/mcp-abap-adt`](https://github.com/fr0ster/mcp-abap-adt) — handler-exposure groups + multi-transport + the AIPNV anti-autopilot stance
- [`marianfoo/sap-ai-mcp-servers`](https://github.com/marianfoo/sap-ai-mcp-servers) — 40+ server meta-registry, skills-layer convergence, generic OData proxy pattern

---

## License

[Apache-2.0](LICENSE). Use it, fork it, build a business on top of it.

---

<div align="center">

**ParagonCorp** · TPO R&D · 2026
*Reference design: PC-TR-2026-SAP-AUTOMATE-01*

</div>
