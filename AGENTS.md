# Agent Guardrails — SAP-Automate

These rules apply to any AI agent driving this MCP server.

## Behavioural guidelines (apply before any tool call)

SAP-Automate adopts the four Karpathy guidelines, ported with attribution
from [`multica-ai/andrej-karpathy-skills`](https://github.com/multica-ai/andrej-karpathy-skills)
(MIT). Run them as a mental pre-flight:

1. **Think before coding** — state your SAP assumptions explicitly; if a
   simpler approach exists, say so; if a precondition is unclear, stop.
2. **Simplicity first** — minimum tool calls that solve the problem; no
   retrieval-layer escalation beyond what's needed; no unbounded table
   reads; no fabricated parameter defaults.
3. **Surgical changes** — touch only what the user asked you to touch;
   clean up only your own mess; match existing style; mention unrelated
   dead code, never delete it.
4. **Goal-driven execution** — define success criteria up front; loop
   until verified; one bullet per step with an explicit `verify:` check.

The full text — adapted with SAP-specific examples — lives in
`skills/karpathy-guidelines.md` and is auto-loaded as the
`sap.skill.karpathy_guidelines` MCP prompt.

The anti-autopilot stance from [`fr0ster/mcp-abap-adt`](https://github.com/fr0ster/mcp-abap-adt)
("AI Pairing, Not Vibing") is captured as `sap.skill.aipnv_ai_pairing` —
a five-question pre-flight checklist that every write-side call must
pass.

## Read-only by default

- Production / QA systems: use `sap.docs.search`, `sap.system.info`, `sap.rfc.search`,
  `sap.rfc.metadata`, `sap.rfc.bulk_metadata`, `sap.table.read`, `sap.table.structure`,
  `abap.adt.get_program`, `abap.adt.get_class`, `abap.adt.get_interface`,
  `abap.adt.get_include`, `abap.adt.get_function_module`, `abap.adt.get_cds_view`,
  `abap.adt.get_package_contents`, `abap.adt.where_used`, `abap.adt.search`,
  `abap.adt.get_table_contents`.
- Do NOT call write-side RFCs (anything where `read_only=false` in its metadata)
  or `abap.adt.activate` unless the server was started with `--enable-writes` AND
  the user has explicitly authorised the change in the current session.
- The server hides write tools from `tools/list` entirely when in read-only mode
  (fr0ster exposure-policy pattern). If you can see a write tool, the operator
  has opted in.

## Cite every claim

Every answer that references SAP behaviour must cite either:
- a `sap-help://` URI from `sap.docs.search`, OR
- a `sap-rfc://` URI from `sap.rfc.metadata`, OR
- a `sap-table://` URI from `sap.table.structure`.

## Before any `sap.rfc.call`

1. Invoke `sap.rfc.metadata` first to confirm the parameter signature.
2. Use the `sap.review-rfc-call` prompt to summarise the intended call.
3. Only then call `sap.rfc.call`.

## Before any `abap.adt.activate` (or any future write-side ADT tool)

1. Always call `abap.adt.where_used` first to enumerate impacted callers.
2. Use the `abap.review-where-used` prompt to structure the impact summary.
3. Only then activate.

## When `abap.adt.get_table_contents` returns DataPreviewBlocked

Some SAP BTP backends block the ADT Data Preview API for certain tables.
The server surfaces this as a structured `[DataPreviewBlocked]` error.
Fall back to `sap.table.read` (RFC path) — it has its own buffer-overflow
safety (max 1000 rows).

## Workflow tools use elicitation — never fabricate confirmations

Three high-stakes workflows pause mid-execution and ask the user to
confirm cost centres, customer numbers, or transport IDs via a
structured form rendered by the client:

- `sap.workflow.create_purchase_order`
- `sap.workflow.maintain_customer_master` (chained two-step elicitation)
- `sap.workflow.release_transport` (re-typed confirmation phrase)

The agent's role is to *kick off the workflow* with the best hints it
has — never to hard-code cost centres, customer keys, or transport IDs.
If the user declines or the client lacks the elicitation capability,
the tool aborts safely with no write side-effect.

## Choose the right retrieval layer

The server exposes four retrieval surfaces; pick deliberately:

| Layer | Tool | When |
|---|---|---|
| **L2 Hybrid** | `sap.docs.search` | Default. Lexical + semantic + RRF + rerank over the document corpus. |
| **L3 GraphRAG** | `kb.global_query` | Global / analytical questions ("which apps touch period close?"). Returns community summaries spanning multiple domains. |
| **L4 HippoRAG** | `kb.multi_hop` | Multi-hop / impact / where-used queries ("what depends on BAPI_X?"). PPR-ranked, hop-distance-bounded. |
| **L5 RAPTOR** | `kb.summarise` | Granularity-aware orientation. Level 0 = leaves, 1 = communities, 2 = SAP module roll-ups. |

When in doubt, start with `sap.docs.search`. Promote to `kb.multi_hop` only
when the user explicitly asks about dependencies, impact, or callers.

## Table reads

- Always set `fields` (column projection) — do not fetch all columns by default.
- Always set a `where_conditions` clause for tables larger than ~1k rows.
- Never raise `max_rows` above the default 100 unless the user requests it.
