# Agent Guardrails — SAP-Automate

These rules apply to any AI agent driving this MCP server.

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
