---
name: sap.skill.abap_code_review
description: Structured ABAP code review with explicit checks for SAP-specific anti-patterns.
tags: [abap, review, quality]
requires_tools: [abap.adt.get_class, abap.adt.get_program, abap.adt.where_used, sap.docs.search]
arguments:
  - name: object_name
    description: ABAP object name to review, e.g. "ZCL_FIN_POSTER"
    required: true
  - name: kind
    description: Object kind (class | program | interface | function_module)
    required: true
---

Review the **{{kind}}** **{{object_name}}** for SAP-specific code quality issues.

1. **Fetch source** — `abap.adt.get_{{kind}}` with name={{object_name}}.
2. **Static checks** (silent unless violations found):
   - **DELETE FROM dbtab WHERE clause-less** — full-table deletes are almost always a bug.
   - **Unbounded SELECT** — `SELECT * FROM <table>` with no WHERE clause or `INTO TABLE` capacity check.
   - **Hard-coded T001 / T100 / T030 entries** — should come from customising tables, not literals.
   - **`MODIFY <std_table>`** directly — violates Clean Core; should go through a released BAPI.
   - **Mixed-case literals where SAP uses upper** — `'eur'` vs `'EUR'` in WAERS comparisons.
   - **Hard-coded company codes / cost centres** — should be parameters.
3. **Architectural checks**:
   - Where-used (`abap.adt.where_used`) — is this object called only from inside its package? If yes, it should be `PRIVATE PROTECTED` not `PUBLIC`.
   - Are there any `CALL FUNCTION 'BAPI_*' IN BACKGROUND TASK` without explicit commit handling?
4. **SAP standard procedures** — for any non-trivial pattern, call `sap.docs.search` with the relevant procedure name to confirm the SAP-canonical approach.

Produce a markdown review with severity tags (`error`, `warning`, `info`), each citing the file location (line number) and the SAP procedure URI. Do NOT modify the code.
