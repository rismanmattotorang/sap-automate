# Comparative analysis: SAP-Automate vs. reference MCP servers

Phase 2 of SAP-Automate is informed by source-reading three production
MCP servers in the SAP ecosystem and two SAP Community blog posts.
This document records the insights drawn and the specific design moves
that follow from them.

## Reference projects studied

| Project | Source | Stack |
|---|---|---|
| **`thupalo/sap-rfc-mcp-server`** | github.com/thupalo/sap-rfc-mcp-server | Python 3.9+, pyrfc, Anthropic SDK |
| **`CDataSoftware/sap-erp-mcp-server`** | github.com/CDataSoftware/sap-erp-mcp-server-by-cdata | Java, JDBC, CData driver |
| **`SAP/mdk-mcp-server`** | github.com/SAP/mdk-mcp-server | Node.js 22+, official SAP project |
| SAP Community: "MCP Server for SAP ECC & S/4HANA – Unlimited ABAP add-on" | community.sap.com | ABAP add-on |
| SAP Community: "Developing Mobile Apps with AI Agents – Introducing the MCP Server for Mobile" | community.sap.com | SAP announcement, MDK |

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

### "Developing Mobile Apps with AI Agents" blog (SAP, official)

- **AI-agent-first design**: tools designed for agent decision-making,
  not human use.
- **Integration with SAP Build / Joule**.
- **Templated scaffolding** reduces choices to a manageable few.

## Where SAP-Automate now improves on the references

| Concern | Reference behaviour | SAP-Automate (Phase 2) |
|---|---|---|
| **Language** | Python / Java / Node | **Rust** — 5–20× faster cold-start, native binary, no runtime dependency |
| **Tool surface** | 3 (CData), 4 (MDK), 13 (RFC) | **12** spanning RFC + tables + RAG + cross-domain docs |
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
| **In-process testability** | low | High — `MockSapClient` ships in the same crate, used by 14 unit tests and 2 integration tests |

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
