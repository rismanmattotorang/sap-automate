---
name: sap.skill.clean_core_audit
description: Audit an ABAP package against SAP Clean Core principles (no modifications, released APIs, extension framework).
tags: [clean-core, abap, audit, compliance]
requires_tools: [abap.adt.get_package_contents, abap.adt.get_class, abap.adt.where_used, sap.docs.search]
arguments:
  - name: package
    description: ABAP package to audit, e.g. "ZFIN"
    required: true
---

Audit the **{{package}}** package against SAP Clean Core principles.

1. **Inventory** — call `abap.adt.get_package_contents` on `{{package}}`. For each member, note its kind (program, class, interface, CDS view, function module, include).
2. **Sample three objects** — pick the largest program, the largest class, and one CDS view (if present). For each:
   a. Fetch its source via `abap.adt.get_program` / `abap.adt.get_class` / `abap.adt.get_cds_view`.
   b. Scan for use of non-released APIs (rule of thumb: any `CL_*` or `cl_*` not in the SAP Released APIs list).
   c. Scan for direct DDIC table reads of standard tables (e.g. `SELECT ... FROM bseg` instead of using released CDS views).
3. **Where-used cross-check** — for each non-released-API touchpoint, call `abap.adt.where_used` to see whether the dependency is limited to the audited package or leaks into others.
4. **Clean Core procedure** — call `sap.docs.search` with `"Clean Core extension framework released API"` to retrieve the canonical procedure.

Produce a 4-section report:
- **Released-API compliance**: percentage of touchpoints using released APIs.
- **Direct table reads**: count + worst offenders.
- **Extension framework usage**: any explicit BAdI / extension point use.
- **Recommended remediation**: ranked by effort vs benefit.

Do NOT propose code changes; produce only the audit report. Code changes are a separate skill (sap.skill.released_api_migration, planned).
