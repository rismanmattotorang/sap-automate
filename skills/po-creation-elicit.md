---
name: sap.skill.po_creation_elicit
description: Guided purchase-order creation with mid-execution elicitation for cost centre, company code, and delivery date.
tags: [mm, purchase-order, elicitation, workflow]
requires_tools: [sap.workflow.create_purchase_order, sap.rfc.metadata, sap.docs.search]
arguments:
  - name: vendor_hint
    description: Vendor (LIFNR) hint, e.g. "V-100100"
    required: false
  - name: material_hint
    description: Material (MATNR) hint
    required: false
---

Create a purchase order for **{{vendor_hint}}** / **{{material_hint}}** using the guided workflow.

The `sap.workflow.create_purchase_order` tool pauses mid-execution and asks the user to confirm:

- Vendor (LIFNR) and material (MATNR)
- Quantity and unit
- **Cost centre (KOSTL)** — high-stakes; never inferred silently
- Company code (BUKRS) and currency
- Requested delivery date

Steps:

1. Optionally run `sap.docs.search` with `"purchase order BAPI_PO_CREATE1"` to confirm the procedure.
2. Optionally run `sap.rfc.metadata` for `BAPI_PO_CREATE1` to confirm the parameter shape that will fire downstream.
3. Call `sap.workflow.create_purchase_order` with any vendor/material hints you have. The tool pauses and asks the user to confirm the form. The user can accept, decline (cancels without side-effects), or cancel.
4. If accepted and the server was started with `--enable-writes`, the next step calls `sap.rfc.call` for `BAPI_PO_CREATE1`. Do NOT proceed without the user's explicit confirmation in the elicitation form.

Cite the BAPI URI (`sap-rfc://BAPI_PO_CREATE1`) and the procedure page in the final summary.
