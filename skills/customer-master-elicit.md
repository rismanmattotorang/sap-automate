---
name: sap.skill.customer_master_elicit
description: Two-step elicitation for customer master maintenance — pick the view, then fill scoped fields.
tags: [sd, customer-master, elicitation, workflow]
requires_tools: [sap.workflow.maintain_customer_master, sap.docs.search]
arguments:
  - name: customer_hint
    description: Customer (KUNNR) hint
    required: false
---

Maintain customer master data for **{{customer_hint}}** using the chained elicitation workflow.

The `sap.workflow.maintain_customer_master` tool issues **two elicitations**:

1. **Scope selection** — which view to maintain (general data | company code data | sales area data).
2. **Scoped fields** — the form fields depend on the chosen view:
   - *general_data*: name, city, country
   - *company_code_data*: reconciliation account, payment terms, dunning area
   - *sales_area_data*: sales org, distribution channel, division, incoterms

Steps:

1. Search the Help Portal first with `sap.docs.search` and `"customer master XD02 BAPI_CUSTOMER"` to confirm the canonical procedure.
2. Call `sap.workflow.maintain_customer_master`. Walk the user through the two elicitations.
3. Echo the confirmed changes back to the user before the (eventual, write-mode-gated) `BAPI_CUSTOMER_CHANGEFROMDATA` call.

This skill exists specifically to demonstrate **chained elicitation** — declining the first form aborts cleanly without ever showing the second.
