---
name: sap.skill.security_sod_audit
description: Segregation-of-Duties (SoD) audit workflow for SAP authorisations — read-only analytical review across users, roles, profiles, RFC connections, and critical T-Codes. Convergent pattern from the Security MCP Server family in marianfoo/sap-ai-mcp-servers.
tags: [security, sod, audit, governance]
requires_tools: [sap.table.read, sap.rfc.metadata, sap.docs.search, sap.system.info]
arguments:
  - name: user_or_role
    description: Target SAP user ID OR role name to audit (e.g. "EDWIN" or "SAP_ALL")
    required: true
  - name: scope
    description: Audit scope — "user" (single user) | "role" (single role) | "system" (whole system roll-up)
    required: true
---

# Segregation-of-Duties audit — read-only

Convergent pattern from the **Security MCP Server** family catalogued in
[`marianfoo/sap-ai-mcp-servers`](https://github.com/marianfoo/sap-ai-mcp-servers)
(~19 read-only analytical tools for SoD, RFC analysis, role review). This
skill produces a structured audit report **without writing anything** —
no role assignments, no profile changes, no transport creation.

**Target:** `{{user_or_role}}` (scope: `{{scope}}`)

## Step 1 — Identify the target

Call `sap.system.info` first. Record `sid` / `client` / `system_role`.

For `{{scope}} == "user"`:

```
sap.table.read table=USR02
  fields=BNAME,USTYP,ACCNT,GLTGV,GLTGB,LOCKED,UFLAG
  where_conditions=["BNAME = '{{user_or_role}}'"]
```

For `{{scope}} == "role"`:

```
sap.table.read table=AGR_DEFINE
  fields=AGR_NAME,PARENT_AGR,TEXT,CREATE_USER,CREATE_DAT
  where_conditions=["AGR_NAME = '{{user_or_role}}'"]
```

## Step 2 — Enumerate authorisations

**User → roles:**

```
sap.table.read table=AGR_USERS
  fields=AGR_NAME,UNAME,FROM_DAT,TO_DAT,ORG_FLAG
  where_conditions=["UNAME = '{{user_or_role}}'", "TO_DAT >= sy-datum"]
```

**Role → authorisation objects:**

```
sap.table.read table=AGR_1251
  fields=AGR_NAME,OBJECT,FIELD,LOW,HIGH,DELETED
  where_conditions=["AGR_NAME = '{{user_or_role}}'", "DELETED = ' '"]
```

**Role → T-Codes:**

```
sap.table.read table=AGR_TCODES
  fields=AGR_NAME,TCODE,COL_FLAG
  where_conditions=["AGR_NAME = '{{user_or_role}}'"]
```

## Step 3 — Apply SoD rule library

Compare the enumerated T-Code set against canonical SoD conflict pairs.
The classical FI/MM conflicts:

| Conflict pair | Risk |
|---|---|
| `FB01` (post journal) + `FBV1` (park) + `FBV0` (post parked) | One person can post unreviewed entries |
| `ME21N` (create PO) + `MIGO` (goods receipt) + `MIRO` (invoice) | Three-way-match bypass |
| `FK01` (create vendor) + `FB60` (post vendor invoice) + `F-53` (pay) | Phantom-vendor fraud |
| `XK01` (create customer) + `VA01` (create order) + `VF01` (bill) | Phantom-customer revenue inflation |
| `SU01` (create user) + `PFCG` (assign roles) + `SCC4` (open client) | Self-privilege escalation |

Cite the SAP-canonical SoD documentation via `sap.docs.search` for each
conflict found (search terms: "SoD", "segregation of duties",
"sensitive transactions").

## Step 4 — Critical authorisation objects

Flag any of these objects with `*` (full) authority — they are
universally over-privileged:

- `S_TCODE` with `*` — execute any transaction
- `S_RFC` with `RFC_NAME=*` — call any RFC
- `S_TABU_DIS` with `DICBERCLS=*` — read any table cluster
- `S_DEVELOP` with `OBJTYPE=*` and `ACTVT=02` — modify any ABAP object
- `S_BTCH_ADM` with `BTCADMIN=Y` — admin all batch jobs
- `S_USER_GRP`, `S_USER_AUT`, `S_USER_AGR` with `ACTVT=*` — full user admin

## Step 5 — RFC destination review

For `{{scope}} == "system"`:

```
sap.table.read table=RFCDES
  fields=RFCDEST,RFCTYPE,RFCHOST,RFCUSER,RFCCLIENT
  where_conditions=["RFCTYPE IN ('3', 'H', 'T', 'W')"]
```

Cross-reference `RFCUSER` against `USR02.USTYP == 'A'` (dialog user).
A dialog user used as an RFC technical account is a finding.

## Step 6 — Report shape

Produce a markdown report with these sections — in this order — and
nothing else:

```markdown
# SoD Audit — {{user_or_role}}

## Target
- System: <sid>/<client> (<system_role>)
- Scope: {{scope}}
- Audit timestamp: <UTC ISO 8601>

## Findings
| Severity | Code | Title | Evidence |
|---|---|---|---|
| HIGH | SOD-001 | <conflict pair> | <T-Codes that overlap> |
| ...

## Critical authorisations
| Object | Field | Value | Role |
|---|---|---|---|
| ...

## Citations
- sap-help://... (SAP SoD reference)
- sap-table://USR02/structure
- ...

## Recommendation
- <Action 1 — must be a change-request title, never a direct write>
- <Action 2>
```

**Never** propose to write the fix yourself. SoD remediation requires a
basis change request, GRC approval, and a transport — out of scope for
the agent.
