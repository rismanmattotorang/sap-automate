---
name: sap.skill.transport_impact_analysis
description: Cross-domain impact analysis for an SAP transport before release.
tags: [basis, transport, impact-analysis]
requires_tools: [abap.adt.where_used, abap.adt.get_package_contents, sap.table.read, sap.docs.search]
arguments:
  - name: transport
    description: Transport request ID, e.g. "ZTRA01K900123"
    required: true
  - name: target_system
    description: Target system (PRODUCTION / QA / DEV)
    required: false
---

Analyse the impact of transport **{{transport}}** on **{{target_system}}** before release.

1. **Enumerate transport contents** — call `sap.table.read` on `E070` and `E071` filtered by `TRKORR = '{{transport}}'` to list every modified object (ABAP class, program, table, function module, CDS view, etc.).
2. **Direct impact** — for each object in the transport, call `abap.adt.where_used` to enumerate every caller, implementer, and include site.
3. **Package context** — for each affected package, call `abap.adt.get_package_contents` to identify sibling objects that may share state.
4. **Business-process impact** — call `sap.docs.search` and `bpmn.find_process` with the package and module names to find which business processes the transport touches.
5. **Pre-import checks** — for any ABAP class / interface change, call `sap.docs.search` with `"ABAP unit test ATC pre-transport"` to retrieve the standard pre-release procedure.

Produce a 3-section report: *Direct impact*, *Indirect dependents*, *Recommended pre-import checks*. Cite every claim. If the impact crosses three or more packages, recommend splitting the transport.
