---
name: sap.skill.transport_release_elicit
description: Transport release with re-typed confirmation phrase and explicit opt-in for dangerous flags.
tags: [basis, transport, elicitation, workflow]
requires_tools: [sap.workflow.release_transport, abap.adt.where_used, sap.docs.search]
arguments:
  - name: transport
    description: Transport request ID (TRKORR), e.g. "ZTRA01K900123"
    required: true
  - name: target_system
    description: Target system (DEV | QA | PRODUCTION)
    required: false
---

Release transport **{{transport}}** to **{{target_system}}**.

The `sap.workflow.release_transport` tool elicits:

- **Transport ID** (pre-filled from the argument hint)
- **Target system** (enum: DEV / QA / PRODUCTION)
- **Release dependent transports?** (boolean; default false)
- **Skip ATC checks?** (boolean; default false — `true` here is dangerous and the agent should warn the user)
- **Confirmation phrase** (the user must re-type the transport ID to proceed)

The tool refuses to execute if the confirmation phrase doesn't match the transport ID, and refuses outright on clients that don't advertise the `elicitation` capability — there is no way to silently release a transport.

Pre-flight checklist before invoking the tool:

1. Call `abap.adt.where_used` on the most critical objects in the transport to surface unexpected impact.
2. Call `sap.docs.search` with `"TMS_MGR_FORWARD_TR_REQUEST transport release"` to confirm the canonical procedure.
3. Call `sap.workflow.release_transport` with the transport hint.

Production releases SHOULD NOT skip ATC. If the user requests `skip_atc=true`, push back and ask the user to confirm in plain text before submitting the elicitation form.
