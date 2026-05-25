---
name: sap.skill.aipnv_ai_pairing
description: AI-Pairing-Not-Vibing (AIPNV) pre-flight checklist — anti-autopilot guardrails for SAP write operations. Forces an explicit human-in-the-loop confirmation before transports, BAPI writes, or ADT activations.
tags: [behaviour, guardrails, aipnv, safety]
requires_tools: [sap.system.info, abap.adt.where_used, sap.review-rfc-call]
arguments:
  - name: intended_action
    description: One-line description of the write you are about to perform (e.g. "activate ZCL_FIN_POSTER", "release transport ER1K900042", "post BKPF document")
    required: true
---

# AI-Pairing-Not-Vibing (AIPNV)

Convergent pattern from
[`fr0ster/mcp-abap-adt`](https://github.com/fr0ster/mcp-abap-adt) —
*"built for AI-assisted pair programming, not autopilot vibe coding"*.

SAP-Automate enforces AIPNV at three layers in the runtime:
exposure policy (write tools hidden in read-only mode), per-call
`read_only=false` flag, AGENTS.md guardrails surfaced in
`initialize.instructions`. This skill is the **fourth** layer — the
agent's own pre-flight checklist, run before the write.

**Intended action:** {{intended_action}}

## The five-question checklist

Answer every question explicitly in your reply to the user. If you
cannot answer one of them, **stop** and ask the user before invoking the
write tool.

### Q1. What system am I targeting?

Call `sap.system.info`. State the `sid` / `client` / `system_role`
(`DEV` / `QAS` / `PRD`) verbatim in your reply.

**Stop conditions:**
- `system_role == "PRD"` and the user has not explicitly authorised a
  production write in *this* session (not a prior session, not
  inferred from context).
- `sid` does not match the system the user named in their request.

### Q2. What is the blast radius?

For ABAP activations, call `abap.adt.where_used` on the target object.
Quote the impacted-caller count.

For BAPI writes, name every dependent document type (e.g. posting
`BKPF` ⇒ touches `BSEG` + `ACDOCA` + `MLHD/MLIT` if material ledger
active).

For transport releases, call `sap.table.read` on `E070` / `E071` to
enumerate impacted objects.

**Stop conditions:**
- More than 50 callers and the user has not acknowledged the scope.
- The impacted object set crosses a Clean Core boundary (i.e. touches
  SAP-standard objects).

### Q3. What is the rollback path?

Name it explicitly:

- ADT activation: previous inactive version still exists in the version
  database (`SE10` → revert).
- BAPI posting: reversal BAPI (e.g. `BAPI_ACC_DOCUMENT_REV_POST`) and
  the document key needed.
- Transport release: transport import-back via STMS, or a compensating
  transport.

**Stop conditions:** no rollback path. Do not proceed.

### Q4. Have I cited the SAP canon for this operation?

Call `sap.docs.search` for the BAPI / transaction / procedure name.
Cite the returned `sap-help://` URI in your reply. If the docs contradict
your intended call signature, fix the call — don't override the canon.

### Q5. Has the user explicitly authorised this write in this session?

Re-read the most recent user turn. The authorisation must be:

- **Explicit** ("yes, post the document" — not "ok" or "go ahead" in
  response to a different question).
- **Scoped** (matches the action you're about to perform — not a blanket
  "do whatever you need").
- **Current** (this session, not inferred from agent memory).

If any of these are false, **invoke the elicitation flow** via the
matching workflow tool (`sap.workflow.create_purchase_order`,
`sap.workflow.maintain_customer_master`,
`sap.workflow.release_transport`) which renders a structured
confirmation form on the client. Never fabricate the confirmation.

## Final gate

Only after Q1–Q5 are answered may you invoke the write tool. Include
the answers in your final report so the audit log (`auditlog://recent`)
captures them alongside the call.

---

*Reference: `fr0ster/mcp-abap-adt` README — "AI Pairing, Not Vibing".
SAP-Automate's runtime layers documented in `AGENTS.md` and
`crates/mcp-server/src/lib.rs` (`ExposurePolicy`).*
