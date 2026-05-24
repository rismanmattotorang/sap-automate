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
│   └── sap-automate-memory/      # working/episodic/semantic/procedural (P8)
├── apps/
│   ├── sap-automate-server/      # main MCP server binary
│   ├── sample-server/            # minimal demo server (echo + add)
│   └── sample-client/            # CLI client that spawns and drives a server
└── docs/
    └── ROADMAP.md                # this file
```

## Phase plan

| Phase | Weeks | Deliverable | Gate | Status |
|---|---|---|---|---|
| **P1**  | 1–4  | Rust MCP server: JSON-RPC, transports, capability router, basic auth | MCP conformance against stub backend | **Phase 1 foundation: ✅ done** |
| P1A | 1–3 (∥)  | Postgres + Qdrant schema; SAP Help crawler; 10k-page pilot | end-to-end vector search returns Help pages | next |
| P2  | 5–8  | ADT + Signavio + LeanIX clients wired as MCP tools | typed tools live | |
| P3  | 9–12 | Hybrid RAG (dense + sparse) + RRF fusion + cross-encoder reranker | P95 hybrid retrieval < 80 ms | |
| P3A | 12–15 (∥) | Contextual retrieval enrichment + SPLADE + parent-child expansion | P95 < 100 ms with reranking | |
| P4  | 13–16 | Ratatui TUI: Sessions / Tools / KB / RAG / Logs tabs | operators hold the latency budget visible | |
| P5  | 17–20 | Next.js 14 web UI: chat, citation rendering, BPMN/graph preview | hands-on usability review | |
| P5A | 20–24 (∥) | ArangoDB graph + Leiden communities + Personalised PageRank | multi-hop traversal < 400 ms P95 (≤4 hops) | |
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

## Phase 1A — next up

1. `sap-automate-kb`: add Qdrant client (HTTP) and Postgres pool (`sqlx`).
2. Build the document schema migration; index seed corpus into Postgres.
3. Implement the SAP Help Portal crawler (HTML extraction, ETag handling,
   breadcrumb capture) — paper §VI-C.
4. Wire `text-embedding-3-large` calls behind a `EmbeddingClient` trait;
   stub with a hash-based mock for offline tests.
5. Replace `InMemoryKb::search` with a `QdrantKb` backend behind the same
   trait; switch `RagEngine` over once parity is reached.

The Phase 1 trait surfaces (`Transport`, `ToolHandler`, KB schema) are stable;
Phase 1A and beyond can land without touching server or client code.
