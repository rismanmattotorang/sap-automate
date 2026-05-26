# SAP-Automate Development Roadmap

> **Status: v1.3.0 released — 2026-05-25.**  v1.0 paper phases ✅ complete; v1.1 shipped three convergence passes (skills + apps + KB/RAG/crawler); v1.2 filled in optional MCP 2025-06-18 spec utilities; v1.3 ships the Live SAP backend tier — `BusinessHubClient` against the SAP Business Accelerator Hub sandbox (OData v4, `API_BUSINESS_PARTNER`), with `sap.bp.search` / `sap.bp.get` MCP tools.  See [`CHANGELOG.md`](../CHANGELOG.md) for release notes and [`docs/INTEGRATION.md`](INTEGRATION.md) for the 3-tier integration strategy.

This document translates the SAP-Automate paper (ParagonCorp TPO R&D, 2026)
into an executable Rust roadmap.

## Strategic goals

- **MCP-native**: the server speaks MCP 2025-06-18 over stdio, HTTP+SSE, and
  Streaming HTTP, with full capability negotiation.
- **Knowledge-engineered**: ABAP, Signavio BPMN, LeanIX EAM, and SAP Help
  Portal indexed as first-class retrieval units (paper §VI).
- **Sub-100 ms P95 retrieval**: layered RAG (hybrid + GraphRAG + HippoRAG +
  RAPTOR) chosen per query (paper §VII).
- **Agentic**: multi-channel gateway, skill library, four-tier memory,
  proactive scheduler, sub-agent runtime (paper §IX).

## Workspace layout

```
sap-automate/
├── crates/
│   ├── mcp-core/                 # JSON-RPC 2.0 + MCP 2025-06-18 types
│   ├── mcp-transport/            # stdio (P1), HTTP+SSE (P1 finalisation), Streaming HTTP
│   ├── mcp-server/               # capability router + dispatch loop
│   ├── mcp-client/               # async client with request/response correlation
│   ├── sap-automate-kb/          # KB: schema + InMemory (P1), Qdrant/Postgres (P1A)
│   ├── sap-automate-rag/         # RAG: L2 hybrid (P3), L3 GraphRAG / L4 HippoRAG / L5 RAPTOR (P5A)
│   ├── sap-automate-connectors/  # ADT (ABAP) + Signavio + LeanIX (P2)
│   ├── sap-automate-skills/      # agentskills.io-compatible loader (P8A)
│   ├── sap-automate-memory/      # working/episodic/semantic/procedural (P8)
│   └── sap-automate-ingest/      # crawler + chunker + embedder + pipeline (P1A)
├── apps/
│   ├── sap-automate-server/      # main MCP server binary
│   ├── sap-automate-ingest/      # ingestion CLI (P1A)
│   ├── sample-server/            # minimal demo server (echo + add)
│   └── sample-client/            # CLI client that spawns and drives a server
└── docs/
    ├── ROADMAP.md                # this file
    └── sample-help-corpus/       # 4-page HTML corpus for end-to-end demo
```

## Phase plan

| Phase | Weeks | Deliverable | Gate | Status |
|---|---|---|---|---|
| **P1**  | 1–4  | Rust MCP server: JSON-RPC, transports, capability router, basic auth | MCP conformance against stub backend | **✅ done** |
| **P1A** | 1–3 (∥)  | Postgres + Qdrant schema; SAP Help crawler; 10k-page pilot | end-to-end vector search returns Help pages | **✅ done (pilot scale)** |
| **P2**  | 5–8  | ADT + Signavio + LeanIX clients wired as MCP tools | typed tools live | **✅ extended**: 10 ADT tools (mario-andreschak coverage + fr0ster's where-used/CDS), `AdtClient` trait with `MockAdtClient` (offline) and `HttpAdtClient` (live, CSRF cache). Signavio + LeanIX clients land in Phase 5 |
| **P3**  | 9–12 | Hybrid RAG (dense + sparse) + RRF fusion + cross-encoder reranker | P95 hybrid retrieval < 80 ms | **✅ done — P95 = 0.159 ms (500× under gate)** |
| **P3A** | 12–15 (∥) | Contextual retrieval enrichment + SPLADE + parent-child expansion | P95 < 100 ms with reranking | **✅ contextual enrichment shipped**; SPLADE deferred to Phase 5A |
| **P4**  | 13–16 | Ratatui TUI: Sessions / Tools / KB / RAG / Logs tabs | operators hold the latency budget visible | **✅ done** — five-tab TUI with P50/P95/P99 per tool, live LatencyBreakdown gauge against the 80 ms budget, KB staleness, structured log tail. Plus skills layer (5 starter skills auto-loaded as MCP prompts) — convergent pattern from `SAP/mdk-mcp-server` + `marianfoo/sap-ai-mcp-servers` + `fr0ster/mcp-abap-adt`. |
| **P5**  | 17–20 | Next.js 14 web UI: chat, citation rendering, BPMN/graph preview | hands-on usability review | **✅ done** — `apps/web` Next.js 14 App Router with 5 routes (Operations, Query Lab, Tool Explorer, Skill Lab, Resources). Speaks MCP 2025-06-18 over HTTP+JSON-RPC through a same-origin Next.js API proxy. Server gained `--transport http` flag. Query Lab is the killer feature — citation chips colour-coded by URI scheme. Screenshots in `docs/web-screens/`. |
| **P5A** | 20–24 (∥) | ArangoDB graph + Leiden communities + Personalised PageRank | multi-hop traversal < 400 ms P95 (≤4 hops) | **✅ done — P95 = 0.082 ms (~5000× under gate)**. `sap-automate-graph` crate: 25-node demo graph, single-pass Louvain, PPR (α=0.15), 3-level RAPTOR. 4 new MCP tools (`kb.multi_hop`, `kb.global_query`, `kb.summarise`, `kb.graph_neighborhood`). Web Graph Lab route. |
| **P6**  | 21–24 | MCP elicitation; SAP-typical workflow templates | live elicitation working | **✅ done — round-trip live over stdio**. `ElicitationHandle` + `tokio::task_local!` `TOOL_CONTEXT` + reader/writer split. 3 workflow tools (`sap.workflow.create_purchase_order`, `…maintain_customer_master` with chained elicitation, `…release_transport` with re-typed confirmation phrase). 3 matching skill templates. Client `ElicitationDelegate` trait with 4 built-in modes (decline / accept / stdin / seed). |
| **P7**  | 25–32 | SAP correctness audit; security hardening; observability | third-party security sign-off | **✅ correctness pass done**. Every BAPI signature aligned with SAP API Hub canon (incl. full ACC_DOCUMENT_POST tables, PO_CREATE1, CUSTOMER_CHANGEFROMDATA1, TMS_MGR_FORWARD_TR_REQUEST). Every table carries MANDT/RCLNT, authorization group, S/4HANA storage notes. ACDOCA modelled as the Universal Journal. ADT URL patterns + headers verified against open-source ADT clients (POST nodestructure, X-SAP-Client capitalization, usageReferences endpoint). 7 precision tests enforce SAP invariants in CI. Security hardening (OAuth/OTEL/chaos) deferred to P7-second-pass. |
| **P8**  | 33–40 | Multi-channel gateway; 12 seed skills; 4-tier memory; proactive scheduler | channel query → cited answer within budget + 50 ms tax | **✅ done (architecture + CLI proof)**. Gateway tax **0 ms** on the demo. New crates: `sap-automate-memory` (working ring-buffer + episodic tag/tenant index + MemoryManager), `sap-automate-scheduler` (TOML-declared jobs + 5 cadence kinds), `sap-automate-channels` (`ChannelAdapter` trait + CliChannel + Teams/Slack/Telegram skeletons + ChannelRegistry). New binary `apps/sap-automate-gw` proves end-to-end channel→agent→MCP routing with per-turn memory recording. `scheduler.toml` declares 4 canonical jobs (ATC weekly / LeanIX EOL quarterly / period-close daily / transport QA hourly) — all 4 fire successfully against the live MCP server. |
| **P9**  | post-MVP | Production wiring — real backends, security hardening, K8s | live SAP demo + ops sign-off | **✅ in progress**. HttpAdtClient integration-test suite (17/17 against an axum mock ADT server — every URL pattern, header capitalisation, CSRF flow, POST-vs-GET, XML parser path verified; uncovered + fixed a real bug in the `nodestructure` XML parser). Production K8s manifests + multi-stage distroless Dockerfile shipped under `deploy/`: Deployment (3 replicas, nonroot, read-only rootfs, hardened probes), Service (ClientIP affinity), HPA (3–12), NetworkPolicy (default-deny + allow-list), PodDisruptionBudget, ConfigMap (with AGENTS.md), Secret template, Kustomize entry point, operator runbook. Next: live SAP sandbox connection + OAuth/OTel/chaos. |

## Phase 1 acceptance — what we shipped today

The foundation is in place; downstream phases extend rather than rewrite it.

- **Workspace**: 12 crates compile cleanly on stable Rust 1.80+.
- **JSON-RPC 2.0**: `Message::from_json` correctly disambiguates request /
  response / notification; round-trip test passes (`mcp-core`).
- **MCP protocol types**: initialise handshake, tools, resources, prompts —
  all serialise to the wire format from paper §II.
- **Transport trait**: send / recv / close contract; line-delimited JSON
  stdio implementation passes round-trip test.
- **Server framework**: builder API for tools/resources/prompts; method
  dispatcher covers initialise, ping, tools/list, tools/call, resources/list,
  resources/read, prompts/list, prompts/get; structured error taxonomy
  matches paper §IV-I.
- **Client framework**: async request/response correlation via oneshot
  channels; typed wrappers for the Phase 1 methods.
- **`sap-automate-server` binary**: exposes `abap.search`,
  `bpmn.find_process`, `eam.search_apps`, `sap.help.search` over stdio,
  backed by an in-memory KB seeded with realistic ABAP/BPMN/EAM/Help docs.
- **`sample-client` binary**: spawns any MCP server, performs initialise,
  lists tools, invokes tools with `--call name=key=val,key=val`.
- **`sample-server` binary**: smoke-test target (echo + add).
- **Integration test**: in-process client ↔ server over `tokio::io::duplex`
  passes initialise → list_tools → call_tool, and validates the UnknownTool
  protocol error.

## Phase 1A acceptance — what we shipped

The acceptance gate (paper §X-B: *"end-to-end vector search returning Help
pages by intent"*) is satisfied at pilot scale.

- **`KnowledgeStore` trait** with two implementations: `InMemoryKb` (default,
  offline) and `QdrantStore` (REST, behind the `qdrant` feature) — same async
  contract, hot-swappable.
- **Document + Chunk schema** (paper §VI-D): stable URIs, breadcrumbs,
  SHA-256 content hash, ETag, per-domain Qdrant collection mapping.
- **HTML parser** (`parse_help_portal_html`) with snapshot test suite
  guarding against Help Portal selector drift (paper §X-N risk-1).
- **`HelpPortalCrawler`** with directory mode (CI / offline) and HTTP mode
  (`If-None-Match` / 304 short-circuit, ETag capture).
- **Chunker** with breadcrumb-prepended contextual prefix and configurable
  target/overlap; preserves sentence boundaries.
- **`EmbeddingClient` trait** with a deterministic `MockEmbedder` (no
  network, used by tests/CI) and an `OpenAiEmbedder` for
  `text-embedding-3-large` (or any OpenAI-compatible endpoint).
- **`IngestionPipeline`** orchestrator: crawl → chunk → batch-embed → upsert.
- **`sap-automate-ingest` CLI** runs the full flow end-to-end with a
  `--verify-query` smoke test that prints top-K hits.
- **MCP server** updated to embed the user's query and route through the
  same `KnowledgeStore` trait; ABAP / BPMN / EAM / SAP Help tools all work
  in either backend mode.
- **Demo corpus**: 4 HTML pages under `docs/sample-help-corpus/` (FI period
  close, MM goods movement, SD billing, HCM payroll); each test query
  returns the right top-1 hit:

  | Query | Top-1 hit | Score |
  |---|---|---|
  | "period close foreign currency revaluation" | FI period-close | 0.329 |
  | "movement type 101 goods receipt" | MM goods movement | 0.329 |
  | "billing document VF01 invoice" | SD billing | 0.372 |
  | "payroll wage type" | HCM payroll | 0.486 |

- **Tests**: 16 passing — JSON-RPC round-trip, transport, KB
  (text + vector + hashing), chunker (breadcrumb + multi-chunk), embedder
  (cosine, dim, normalisation), pipeline (ingest → vector search),
  crawler (parse + reject), MCP integration (handshake + tool call + error).

## Next up: Phase 2 — SAP Connectors

1. Wire real ADT REST client into `sap-automate-connectors` (paper §IV
   data-and-I/O band).
2. Signavio GraphQL client.
3. LeanIX GraphQL client.
4. Add the corresponding MCP tools: `abap.read_object`, `abap.callers`,
   `bpmn.read_xml`, `eam.impact_map`, `eam.lifecycle`.
5. Transport-resilient retry + circuit-breaker policies.

Phase 1A trait surfaces (`KnowledgeStore`, `EmbeddingClient`,
`IngestionPipeline`) are stable; Phase 2 lands without touching them.

---

# Post-v1.1 Forward Plan

> **v1.1.0 is the convergence-pass release, not the finish line.**  All paper phases (P1–P9) plus three Karpathy-style convergence passes are shipped; the codebase has 145 tests passing, sub-millisecond P95 retrieval, production K8s manifests, a polished agentic surface, and a hierarchical doc-tree navigation surface (OpenKB + PageIndex pattern).  Everything below is what we build *next*, anchored to the four strategic themes that turn this project from "best-in-class open source" into "the category-defining SAP-agent platform".

## v1.1.0 — state of the union (released 2026-05-25)

What v1.1 added on top of v1.0:

| Surface | What shipped | Notes |
|---|---|---|
| Skills library | **8 → 13** auto-discovered | + Karpathy guidelines, AIPNV anti-autopilot, OData design, SoD audit, BW-to-Datasphere |
| MCP tools | **32 → 35** | + `sap.system.cache_stats`, `sap.system.cache_invalidate`, `sap.kb.navigate` |
| MCP resources | **11 → 12** | + `sap-cache://stats` |
| KB layer | **`DocumentTree`** + content-hash dedup at upsert | OpenKB + PageIndex pattern; `KnowledgeStore::get_document_tree` default impl |
| RAG layer | **`RetrievalDiagnostics`** on `SearchResponse` | dense / sparse counts, RRF overlap, tokenised query terms, reranker-ran flag |
| Crawler | robots.txt + per-host rate-limiter + BM25 "fit markdown" filter | Crawl4AI convergent patterns; pure Rust, no new deps |
| RFC layer | **`MetadataCache`** TTL decorator wired in the server | thupalo's `get_metadata_cache_stats` pattern; live in TUI KB tab + web Operations panel |
| Gateway | Skill-aware routing (8 intents) | `match_skill()` invokes `prompts/get` before falling back to raw tools |
| Tests | **104 → 145** | +6 metadata-cache +6 doc-tree +3 cache-tools integration +4 kb-navigate integration +7 robots +5 rate-limit +4 fit-markdown +3 store dedup/tree +2 RAG diagnostics +5 misc |

## v1.0.0 — state of the union (released 2026-05-25)

| Surface | Status | Notes |
|---|---|---|
| MCP protocol (2025-06-18) | ✅ complete | tools, resources, prompts, structured elicitation, all wire-format compliant |
| MCP server (stdio + HTTP/SSE) | ✅ production | split reader/writer, cancellation-safe, exposure-policy gated |
| MCP client | ✅ production | typed wrappers, elicitation delegate, split-half spawn |
| SAP backend traits | ✅ stable | `SapClient`, `AdtClient`, `KnowledgeStore`, `EmbeddingClient`, `Reranker`, `ChannelAdapter`, `AuditSink` |
| Real SAP wiring | 🟡 partial | `HttpAdtClient` complete (17 integration tests); `NetweaverSapClient` is the next mock to replace |
| RAG: hybrid + GraphRAG + HippoRAG + RAPTOR | ✅ production | P95 0.16 ms / 0.08 ms — orders under the paper's gates |
| Agentic layer (memory + scheduler + channels) | ✅ architecture | CLI channel live; Teams / Slack / Telegram trait-skeletons |
| Observability | 🟡 partial | Prometheus `/metrics` shipped; OTLP exporter behind feature flag; Grafana dashboard JSONs queued |
| Deployment | ✅ ready | Multi-stage distroless Dockerfile, 9 hardened K8s manifests, Kustomize, runbook |
| CI/CD | ✅ production | GitHub Actions: fmt, clippy, stable+beta test matrix, SAP-precision gate, P95 gate, cargo-audit, Docker build, K8s lint, Next.js web build, release pipeline (multi-arch GHCR) |
| Security | 🟡 partial | Read-only by default, audit log + redaction, structured error taxonomy.  OAuth 2.1 / mTLS / pen-test remain for v1.2 |

## Strategic themes for the next 12 months

The four product themes for v1.1 through v2.0:

1. **Real-world wiring** — replace every mock backend with a live integration test against a real SAP / Qdrant / ArangoDB / Postgres.
2. **Enterprise security** — OAuth 2.1, XSUAA service-key, mTLS, SBOM, cosigned containers, third-party pen-test.
3. **Agent intelligence** — LLM router, reflection loop, sub-agent runtime, real ONNX cross-encoder, contextual enrichment via prompt-cached LLM call.
4. **Ecosystem reach** — Microsoft Teams / Slack / Telegram / WhatsApp real adapters, public docs site, sample agent libraries, ParagonCorp Skill Commons.

## Versioned milestones

### v1.1 — Live SAP wiring  (target Q3 2026, ~6 weeks)

Replace mocks with real backends so the *demo* runs against the *real thing*.

- `NetweaverSapClient` against a published SAP sandbox (e.g. ES5 / BTP trial), behind a `netweaver` feature flag.  17+ integration tests modelled on the existing ADT suite.
- `HttpAdtClient` end-to-end test against an actual ABAP Cloud trial; CSRF token rotation under load.
- `QdrantStore` integration tests against a containerised Qdrant in CI.
- `OpenAiEmbedder` + `voyage` embedder backends in CI behind secrets-gated jobs.
- Postgres `DocumentStore` (currently in-memory) with `sqlx` migrations.
- ArangoDB `GraphStore` (currently `InMemoryGraph`) — full schema, AQL queries.
- **Acceptance gate:** end-to-end agent query *"investigate why FI period 2026-M03 didn't close"* runs against the sandbox, returns a cited answer with a real ACDOCA snippet and a real ATC reading.

### v1.2 — Enterprise security hardening  (Q4 2026, ~8 weeks)

Paper §X-J completed.

- **OAuth 2.1 with PKCE** for the HTTP transport (RFC 6749 / 7636).
- **XSUAA service-key** auth for SAP BTP (the `AdtAuth::ServiceKey` variant fully wired).
- **mTLS** support for both transports (server-side cert + optional client cert verification).
- **External Secrets Operator** integration documented end-to-end (Vault / AWS SM / Azure KV examples).
- **Audit log sinks**: Loki, S3 with object lock, Splunk HEC, Azure Monitor.
- **SBOM** generation (CycloneDX) embedded in every release.
- **Cosign-signed** container images + GitHub provenance attestations (SLSA Level 3).
- **Third-party pen-test** by an external SAP-focused firm; remediation tracked in `docs/SECURITY_REVIEW.md`.
- **Compliance posture document** (`docs/COMPLIANCE.md`): GDPR Article 30 records-of-processing, SOX evidence trail for FI postings, retention policy.

### v1.3 — Observability finalisation  (Q4 2026 in parallel, ~3 weeks)

Make the operator experience production-grade.

- **OpenTelemetry OTLP exporter** wired into `apps/sap-automate-server` behind the `otlp` feature flag.  Spans propagate through every layer: MCP session → tool → SAP backend → KB / graph.
- **Grafana dashboard JSONs** shipped under `deploy/grafana/`: latency budget, error rate, pool saturation, scheduler health, channel throughput.
- **SLI / SLO YAML** declarations (the Pyrra format).
- **Alertmanager rules** for the paper §X-D / §X-H gates: alert when P95 retrieval exceeds 80 ms or multi-hop exceeds 400 ms.
- **Continuous benchmarks** in CI (`cargo-criterion`) with regression detection against `main`.

### v1.4 — Real channel adapters  (Q1 2027, ~8 weeks)

The trait surface is stable; this lights up the actual user-facing channels.

- **Microsoft Teams** (Bot Framework SDK + Adaptive Cards).
- **Slack** (Bolt SDK + Block Kit).
- **Telegram** (webhook + Bot API).
- **WhatsApp Business Cloud API**.
- **Email** (IMAP receive + SMTP send via `lettre`).
- **Hermes-Agent pairing flow** — channel user ↔ MCP session binding with mTLS or signed-cookie session tokens.
- **Per-channel idempotency** (the gateway de-duplicates retried webhooks).

### v1.5 — Agent intelligence  (Q1 2027, ~10 weeks)

Replace the current keyword-based intent router with LLM-driven reasoning.

- **`LlmRouter` trait** + reference implementations against Claude, GPT, Gemini, local Llama via `llama.cpp`.
- **Reflection loop** (paper §IX-B P3-self-improvement): trajectory capture → LLM critique → memory consolidation into episodic store.
- **Sub-agent runtime** (paper §IX-B): contained child agents share the parent's MCP surface but with restricted exposure policies.
- **Skill Commons** (paper §X-L): private registry at `commons.paragoncorp.example` for signed, code-reviewed skills.  Skills carry an Ed25519 signature; unsigned skills refuse to load in production deployments.
- **Streaming responses** through the HTTP/SSE transport (`notifications/progress` per MCP 2025-06-18) so the web UI renders tokens as they arrive.

### v1.6 — Multi-tenancy + multi-system  (Q2 2027, ~6 weeks)

- **Per-tenant KBs** (`KnowledgeStore` already takes a tenant scope; wire end-to-end).
- **Per-tenant skill libraries** (`SkillRegistry` accepts a tenant filter).
- **Quota enforcement** at the tool-call boundary (req/s, tokens/min).
- **BTP Destination service** lookup as a runtime backend for `AdtDestination`.
- **Multi-SID** support: one server instance serves multiple SAP systems via a `--system <SID>` request header.

### v1.7 — Advanced retrieval  (Q2 2027 in parallel, ~6 weeks)

- **ColBERT late-interaction** retrieval (paper §VII-C) — measurable precision lift on ABAP-identifier queries.
- **SPLADE sparse encoder** (paper §X-E) for code corpora — replaces BM25 on the ABAP collection.
- **Real ONNX cross-encoder reranker** (`bge-reranker-large`, `flashrank/MS-MARCO-MiniLM`) wired through `ort` crate; the `MockReranker` becomes the offline fallback only.
- **Contextual chunk enrichment via LLM** with prompt caching (paper §VII-D + §X-N risk-2) — replaces the current extractive heuristic.

### v2.0 — Adjacent SAP surfaces  (H2 2027, ~3 months)

A major release that broadens beyond S/4HANA core.

- **SAP Datasphere** connector (analytics + governance APIs).
- **SAP Cloud Integration (CPI)** iFlow management adapter.
- **SAP Cloud ALM** bridge for incident + change correlation.
- **SAP Build / Joule Studio** export — generate Joule-compatible skill packages from SAP-Automate skills, opening a migration path *into* SAP-Automate from Joule deployments.
- **SAP BTP destination-service** native integration.
- **OData v2/v4 generic proxy** — point at any OData metadata URL, get an MCP tool surface for free (the `marianfoo/sap-ai-mcp-servers` convergent pattern).

## Ongoing tracks

Three tracks run continuously alongside the versioned milestones:

- **Operational maturity**
  - Helm chart with dev/staging/prod overlays (currently Kustomize only).
  - ArgoCD / Flux GitOps templates.
  - Terraform modules for AWS / Azure / GCP.
  - Backup + restore runbook for the Postgres / Qdrant / ArangoDB tier.
  - Cost-optimisation tooling — local `bge-m3` embedder backend as the default when no OpenAI key is configured, per paper §X-N risk-2.

- **Community + ecosystem**
  - Public docs site (mdBook) with API reference + tutorials + recipe cookbook.
  - Sample agent libraries for Claude Code, ChatGPT Custom GPTs, LangChain, AutoGen, CrewAI.
  - Discord + GitHub Discussions.
  - `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`.
  - Public Skill Commons (curated, code-reviewed skills with Ed25519 signatures).
  - Quarterly "what's new" video.
  - Conference circuit (SAP TechEd, SAP Sapphire, KubeCon, RustConf).

- **Research feedback loop**
  - Benchmark against named market alternatives (Joule, CData, the 6 reference projects) using paper §XII methodology — publish results quarterly.
  - Track which retrieval layer (L2/L3/L4/L5) the agent actually picks in production — feeds the next iteration of the router.
  - Episodic-memory hit-rate measurement (paper §X-L gate of 8%) — informs skill-commons promotion.
  - Continued whitepaper updates as the system evolves; ParagonCorp Technical Review Vol. 1 No. 2 planned for end of 2026.

## What's explicitly NOT on the roadmap

To keep the project focused:

- ❌ A custom LLM.  We integrate with existing model providers; we do not train.
- ❌ Vendor-locked deployment.  SAP-Automate stays infrastructure-agnostic; no AWS-only or Azure-only paths.
- ❌ A graphical agent designer.  Skills authored in markdown remain the primary surface; UI tooling is *editing* skills, never *replacing* them.
- ❌ Closed-source plugins.  Apache-2.0 across the board.

## How to influence the roadmap

- Open a GitHub issue tagged `roadmap` with a use case.
- Vote on existing `roadmap` issues.
- For enterprise customers: ParagonCorp accepts paid prioritisation under standard commercial terms — contact `tpo-research@paracorpgroup.com`.

---

*Reference design: PC-TR-2026-SAP-AUTOMATE-01.  Whitepaper: [`docs/SAPAutomate.pdf`](SAPAutomate.pdf).*
