# Agent Guardrails — SAP-Automate

These rules apply to any AI agent driving this MCP server.

## Read-only by default

- Production / QA systems: use `sap.docs.search`, `sap.system.info`, `sap.rfc.search`,
  `sap.rfc.metadata`, `sap.rfc.bulk_metadata`, `sap.table.read`, `sap.table.structure`.
- Do NOT call write-side RFCs (anything where `read_only=false` in its metadata)
  unless the server was started with `--enable-writes` AND the user has explicitly
  authorised the change in the current session.

## Cite every claim

Every answer that references SAP behaviour must cite either:
- a `sap-help://` URI from `sap.docs.search`, OR
- a `sap-rfc://` URI from `sap.rfc.metadata`, OR
- a `sap-table://` URI from `sap.table.structure`.

## Before any `sap.rfc.call`

1. Invoke `sap.rfc.metadata` first to confirm the parameter signature.
2. Use the `sap.review-rfc-call` prompt to summarise the intended call.
3. Only then call `sap.rfc.call`.

## Table reads

- Always set `fields` (column projection) — do not fetch all columns by default.
- Always set a `where_conditions` clause for tables larger than ~1k rows.
- Never raise `max_rows` above the default 100 unless the user requests it.
