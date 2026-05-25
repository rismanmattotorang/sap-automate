# Comparative analysis: SAP-Automate vs. reference MCP servers

Phase 2 of SAP-Automate is informed by source-reading three production
MCP servers in the SAP ecosystem and two SAP Community blog posts.
This document records the insights drawn and the specific design moves
that follow from them.

## Reference projects studied

| Project | Source | Stack |
|---|---|---|
| **`thupalo/sap-rfc-mcp-server`** | github.com/thupalo/sap-rfc-mcp-server | Python 3.9+, pyrfc |
| **`CDataSoftware/sap-erp-mcp-server`** | github.com/CDataSoftware/sap-erp-mcp-server-by-cdata | Java, JDBC, CData driver |
| **`SAP/mdk-mcp-server`** | github.com/SAP/mdk-mcp-server | Node.js 22+, official SAP project |
| **`mario-andreschak/mcp-abap-adt`** | github.com/mario-andreschak/mcp-abap-adt | TypeScript, 13 read-only ADT tools |
| **`fr0ster/mcp-abap-adt`** | github.com/fr0ster/mcp-abap-adt | TypeScript, 60+ tools, full CRUD, RAP-first, multi-transport, "AI Pairing, Not Vibing" |
| **`marianfoo/sap-ai-mcp-servers`** | github.com/marianfoo/sap-ai-mcp-servers | Meta-registry of 40+ SAP MCP servers, skills, and Claude plugins |
| **`multica-ai/andrej-karpathy-skills`** | github.com/multica-ai/andrej-karpathy-skills | Skill repository — behavioural guidelines (MIT) distilled from Andrej Karpathy's observations on LLM coding pitfalls |
| SAP Community blog #1 (ABAP add-on for ECC/S4) | community.sap.com | ABAP add-on |
| SAP Community blog #2 (Mobile MCP) | community.sap.com | SAP announcement, MDK |

## Best ideas drawn per project

### `thupalo/sap-rfc-mcp-server`

- **13-tool surface**: `rfc_system_info`, `get_rfc_functions`,
  `call_rfc_function`, `get_function_metadata`, `search_rfc_functions`,
  `get_metadata_cache_stats`, `bulk_load_metadata`,
  `export_metadata_for_rag`, `read_table`, `read_table_complete`,
  `get_table_structure`, `test_table_access`.
- **Connection pooling** via `SAP_POOL_SIZE` (default 10).
- **Persistent metadata cache** (200–500 ms cold → 1–5 ms warm).
- **Bulk metadata** (`bulk_load_metadata`) avoids N round-trips.
- **Multi-source credentials** with priority chain: env → keyring → encrypted file → .env.
- **Version-aware** behaviour from R/3 4.5B to S/4HANA (handles single-letter
  language codes on legacy systems).
- **Dual stdio + HTTP transport**.

### `CDataSoftware/sap-erp-mcp-server`

- **Universal SQL discovery triad**: `get_tables` → `get_columns` →
  `run_query`. Agents need to know what's available before querying.
- **Read-only by default**. Write capabilities are gated behind a separate
  commercial product. A strong safety property.
- **OAuth 2.0** for the JDBC driver.

### `SAP/mdk-mcp-server`

- **AGENTS.md per-project guardrails**. A markdown file that constrains AI
  behaviour without code changes.
- **Constrained-enum tool parameters** (e.g. `controlType` ∈
  {`ObjectTable`, `FormCell`, …}). Agents pick from a finite set rather
  than free-form.
- **Project-aware tools**: every tool takes `folderRootPath` so it can
  introspect existing state.
- **Semantic doc search** (`mdk-docs`) via
  `@huggingface/transformers.js` WordPiece tokenisation.
- **Schema versioning**: `--schema-version 26.3` keeps tool catalogues
  stable across client releases.
- **Multi-artefact tools**: `mdk-gen` handles page / action / i18n / rule
  in one tool, reducing tool churn.

### "Unlimited ABAP add-on" blog (inferred from title + SAP literature)

- **MCP server embedded inside SAP** as an ABAP add-on. Native auth +
  native business logic — no RFC translation gap.
- **Operates on standard SAP objects**: Sales Order, Purchase Order,
  Material Master, etc., via BAPIs / standard dialogs.
- **Display AND create** workflows from one surface.
- **Insight for us**: design the `SapClient` trait so a future
  `AbapBridgeSapClient` can be a drop-in replacement, routing tool calls
  through an in-SAP HTTP endpoint backed by ABAP.

### `mario-andreschak/mcp-abap-adt`

- **13 read-only ADT tools** with crisp object-type-specific signatures:
  `GetProgram`, `GetClass`, `GetFunctionGroup`, `GetFunction`,
  `GetStructure`, `GetTable`, `GetTableContents`, `GetPackage`,
  `GetTypeInfo`, `GetInclude`, `SearchObject`, `GetInterface`,
  `GetTransaction`.
- **ADT REST** (`/sap/bc/adt/...`) over HTTPS — the modern ABAP
  integration surface (no RFC SDK on the build host).
- **Basic auth + `.env` config**, deliberately simple and easy to deploy.

### `fr0ster/mcp-abap-adt`

- **Near-rewrite with 60+ tools and full CRUD** for Class / Interface /
  CDS View / Program / Function Group / Function Module / Domain /
  Data Element / Table / Structure.
- **RAP-first**: BDEF / DDLX / Service Definition / Metadata Extension
  as first-class CRUD targets.
- **Where-used analysis**, **AST + semantic introspection**, **transport
  management** (`CreateTransport`, `ActivateObject`), **runtime
  diagnostics** (profiler traces, dumps, gateway error log).
- **3 transports** (stdio, HTTP, SSE), **5 auth schemes** (Basic, JWT,
  Service Key, mTLS, Kerberos), **destination model** with named
  service-key files.
- **Embeddable server pattern** for SAP CAP / Express integration.
- **Handler exposure groups** (`exposition: ['readonly', 'high']`) —
  role-based tool filtering.
- **"AI Pairing, Not Vibing"** explicit anti-autopilot stance.

### `marianfoo/sap-ai-mcp-servers` (meta-registry, 40+ servers)

- **Skills layer convergence**: across CAP Agentic Engineered Skills,
  ARC-1 SAP Skills, RAP Skills, SAP Skills for Claude Code, the
  pattern is the same — **markdown files with YAML frontmatter** wrap
  tool composition + prompt engineering for a specific workflow.
  Agents invoke skills, not raw tools.
- **Config-driven OData proxy**: foundation layer; single JSON +
  BTP destinations = MCP tool surface for any SAP OData service.
  Zero code per new system.
- **Multi-server orchestration**: ADT + GUI + Docs + OData servers
  used together (read metadata, then act, then verify).
- **Knowledge-driven decision-making**: documentation MCP servers
  (SAP Docs MCP, SAP Notes MCP) inform agent choices before execution.

### `multica-ai/andrej-karpathy-skills`

- **Single-file behavioural skill** (`skills/karpathy-guidelines/SKILL.md`,
  MIT-licensed) distilling four principles from
  [Karpathy's observations](https://x.com/karpathy/status/2015883857489522876)
  on LLM coding pitfalls:
  1. **Think before coding** — surface assumptions; don't silently pick
     one of multiple interpretations.
  2. **Simplicity first** — minimum code; no speculative abstractions;
     no error handling for impossible cases.
  3. **Surgical changes** — touch only what the request demands; match
     existing style; orphan only what your change orphaned.
  4. **Goal-driven execution** — transform tasks into verifiable
     success criteria; loop independently.
- **Insight for us**: the *form* of a behavioural skill — frontmatter +
  numbered principles + acceptance checklist — slots cleanly into
  SAP-Automate's existing `SkillRegistry` without any change to the
  loader. The principles themselves are a force-multiplier on top of
  AGENTS.md guardrails (which constrain *what* the agent may call) by
  constraining *how* it should reason before any call.
- **Design move**: port the skill verbatim in spirit, with SAP-specific
  examples in each principle (period close, BAPI selection,
  read-only-by-default retrieval-layer escalation). Land it as
  `skills/karpathy-guidelines.md` and surface it in `AGENTS.md` as the
  default pre-flight.

### "Developing Mobile Apps with AI Agents" blog (SAP, official)

- **AI-agent-first design**: tools designed for agent decision-making,
  not human use.
- **Integration with SAP Build / Joule**.
- **Templated scaffolding** reduces choices to a manageable few.

## Where SAP-Automate now improves on the references

| Concern | Reference behaviour | SAP-Automate (Phase 2 + ADT) |
|---|---|---|
| **Language** | Python / Java / Node | **Rust** — 5–20× faster cold-start, native binary, no runtime dependency |
| **Tool surface** | 3 (CData), 4 (MDK), 13 (RFC), 13 (mario-andreschak), 60+ (fr0ster) | **22 tools + 3 prompts**: RAG (5) + RFC/tables (7) + ADT (10) — covers the union of the two strongest reference surfaces with read-only-by-default safety |
| **Error model** | text strings (RFC server) | **Structured error taxonomy** (RFC_TIMEOUT, RFC_AUTH_FAILED, RFC_NOT_FOUND, TABLE_BUFFER_OVERFLOW, …) mapped to MCP JSON-RPC codes |
| **Transient vs permanent classification** | none | Encoded in `RfcError::is_transient()`; `retry_with_backoff` only retries transients |
| **Circuit breaker** | none | `CircuitBreaker` with configurable threshold and open-duration |
| **Read-only safety** | CData yes, RFC server no | **Yes by default**, `--enable-writes` flag, per-RFC `read_only` metadata flag, server refuses non-read-only RFCs in read-only mode |
| **Credentials** | env → keyring → file (RFC) | **`LayeredCredentialProvider`** trait — composable chain, easy to add OAuth / Vault / Conjur backends |
| **Connection pool** | one knob (`SAP_POOL_SIZE`) | **`ConnectionPool`** primitive with try-acquire / available-slots semantics |
| **Bulk metadata** | yes (RFC server) | **Yes** — same shape, with explicit `missing` list for misses |
| **MCP resources** | sparse | **11 resources** seeded: `sap-system://info`, `sap-rfc://{name}` ×6, `sap-table://{name}/structure` ×3, `agents://guardrails` |
| **MCP prompts** | rare | **2 prompts**: `sap.review-rfc-call`, `sap.transport-impact-analysis` |
| **AGENTS.md** | mdk-mcp pattern | **Loaded from disk** at `./AGENTS.md` or `./.sap-automate/AGENTS.md`; surfaced in `initialize.instructions` and as `agents://guardrails` resource |
| **Constrained-enum params** | mdk-mcp pattern | Used in `sap.docs.search` (`domain` ∈ {all, sap_help, abap, bpmn, leanix}) and prepared for tool catalogue expansion |
| **Schema validation** | declarative JSON schema | **Same** — every tool emits a complete JSON Schema with `additionalProperties: false` |
| **Knowledge integration** | none (RFC server: opaque) | **First-class RAG layer**: `sap.docs.search` returns cited snippets across Help Portal, ABAP, BPMN, LeanIX, all from `sap-automate-rag` (Phase 1A) |
| **In-process testability** | low | High — `MockSapClient` + `MockAdtClient` ship alongside the live backends, used by 22 unit tests and 4 integration tests |
| **ADT REST integration** | mario-andreschak yes (read-only), fr0ster yes (CRUD) | **`AdtClient` trait** with `MockAdtClient` (offline) and `HttpAdtClient` (CSRF cache + cookie jar + base64 inliner — no extra deps). Read-only-by-default; write tools gated by exposure policy |
| **Where-used / impact analysis** | fr0ster yes | **`abap.adt.where_used` tool** + `abap.review-where-used` prompt. Mock client ships realistic dependency graphs (interface → class → program → include) so the impact-analysis story is demonstrable offline |
| **CDS / RAP** | fr0ster yes | **`abap.adt.get_cds_view` tool** with structured annotation extraction. RAP BDEF / DDLX / Service Def slots in via the trait when production wiring lands |
| **Transports** | stdio / HTTP / SSE (fr0ster) | **stdio (now)** + **HTTP/SSE transport** (`HttpServerTransport`) under the `http` feature with bearer-token auth and SSE event bus |
| **Destination model** | fr0ster | **`AdtDestination` + `AdtAuth` enum** (Basic / Bearer / ServiceKey / Certificate / Mock). Redacted view surfaced as `adt-destination://info` resource |
| **Handler exposure groups** | fr0ster `exposition: ['readonly', 'high']` | **`ExposurePolicy` enum** + `ToolDescriptor::with_writes()`. Read-only-by-default; `--enable-writes` flips it. **Hides write tools from `tools/list` entirely** so agents don't see what they can't call |
| **AI-pairing-not-vibing safety stance** | fr0ster | **Multiple lines of defence**: exposure policy + per-call `read_only` flag + per-RFC `read_only` metadata + AGENTS.md guardrails surfaced in handshake + structured error codes |
| **Operator TUI** | mostly absent in references | **5-tab Ratatui TUI** (Sessions / Tools / KB / RAG / Logs) with live LatencyBreakdown gauge against the 80 ms budget, per-tool P50/P95/P99, KB staleness, structured log tail. Connects via admin endpoint or local synthetic feed for offline ops drills |
| **Web UI** | mostly absent (CData ships a minimal management UI; SAP Joule is proprietary; the marianfoo registry catalogues 40+ servers but only one mentions a "web management interface") | **Next.js 14 App Router** with 5 routes that speak MCP 2025-06-18 directly via HTTP+JSON-RPC through a same-origin proxy. **Query Lab** shows ranked results with citation chips colour-coded by URI scheme — no other open-source SAP MCP has this. **Tool Explorer** auto-generates forms from each tool's JSON Schema and surfaces the read-only/write flag. **Skill Lab** live-renders any prompt with argument substitution as you type. **Resources** browser groups by URI scheme. **Operations** dashboard mirrors the TUI with a live latency-budget gauge against the 80 ms gate |
| **Skills layer** | mdk + fr0ster + marianfoo | **`SkillRegistry` auto-discovers markdown skills** from `./skills/`, `./.sap-automate/skills/`, `~/.config/sap-automate/skills/`. Each skill = one MCP prompt. Frontmatter declares required tools (validated at registry time), arguments, tags. **13 skills shipped**: 8 SAP-domain (period_close_investigation, transport_impact_analysis, rap_service_scaffolding, clean_core_audit, abap_code_review, po_creation_elicit, customer_master_elicit, transport_release_elicit) + 5 behavioural / design (karpathy_guidelines, aipnv_ai_pairing, odata_service_design, security_sod_audit, bw_to_datasphere_migration). |
| **Behavioural pre-flight** | absent across all references | **`sap.skill.karpathy_guidelines`** ported from [`multica-ai/andrej-karpathy-skills`](https://github.com/multica-ai/andrej-karpathy-skills) (MIT) with SAP-specific examples. Surfaced in `AGENTS.md`. Slots into the existing `SkillRegistry` with zero loader changes. |
| **RFC metadata cache** | thupalo: file-based, persistent, compressed; ~1–5 ms cached vs ~200–500 ms direct | **`MetadataCache<C: SapClient>` decorator** in `sap-automate-rfc`: TTL-keyed `(function, language)` cache over any `SapClient`. Bulk reads split into hits + misses so only misses hit the SAP system. `CacheStats` surfaced for Prometheus. `invalidate_all()` for system-role flips. No extra deps (`tokio::sync::RwLock`). Persistence + compression deferred behind the trait — easy follow-up if real load demands it. |

## Architectural moves

1. **`SapClient` trait** isolates the SAP backend so we can ship offline
   (`MockSapClient`), production (future `NetweaverSapClient`), and the
   ABAP-bridge pattern (future `AbapBridgeSapClient`) interchangeably.
2. **`KnowledgeStore` trait** (Phase 1A) already does the same for the KB.
3. **`EmbeddingClient` trait** (Phase 1A) already does the same for
   embeddings.
4. Together these three traits define a *backend matrix* — every cell is
   independently replaceable without touching the server, the client, the
   tool surface, or the test suite.

This stratification is the single biggest structural improvement over the
reference projects, all of which couple their SAP-access code to their MCP
server code one way or another (pyrfc imports leak into the MCP
registration in `thupalo/sap-rfc-mcp-server`; the JDBC connection lives at
file-load time in the CData project).

## Open follow-ups (Phase 2 finalisation)

- `NetweaverSapClient` — a real RFC binding behind the `SapClient` trait.
  Requires the SAP NetWeaver RFC SDK on the build host.
- `AbapBridgeSapClient` — HTTP client speaking to an in-SAP ABAP add-on
  endpoint, modelled on the "Unlimited ABAP add-on" blog.
- Cross-domain `sap.transport-impact-analysis` prompt → concrete walk-through
  by combining `sap.docs.search`, `sap.rfc.search`, `sap.table.read`.
- Streaming `notifications/progress` for long-running ATC scans and
  multi-page table reads.
