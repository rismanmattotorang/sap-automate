# SAP-Automate Development Roadmap

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
| P6  | 21–24 | MCP elicitation; SAP-typical workflow templates | live elicitation working | |
| P7  | 25–32 | Pen-test, OAuth flow, observability, chaos engineering | third-party security sign-off | |
| P8  | 33–40 | Multi-channel gateway (Teams → Slack → Telegram); 12 seed skills; 4-tier memory | Teams query → cited answer within budget + 50 ms tax | |
| P8A | 37–44 (∥) | Reflection loop, scheduler, sub-agent runtime, skill commons | ≥5 new skills + 8% episodic hit rate | |

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
