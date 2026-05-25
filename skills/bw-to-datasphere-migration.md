---
name: sap.skill.bw_to_datasphere_migration
description: BW-to-Datasphere modernisation workflow — inventory BW objects, classify migration patterns (lift-and-shift vs redesign), produce a phased migration plan. Convergent pattern from the BW Modernization MCP family in marianfoo/sap-ai-mcp-servers.
tags: [bw, datasphere, modernization, planning]
requires_tools: [sap.docs.search, sap.table.read, abap.adt.where_used, kb.global_query]
arguments:
  - name: bw_object
    description: BW object name (InfoCube, ADSO, Composite Provider, Query, Process Chain) OR "*" for system-wide inventory
    required: true
  - name: target_release
    description: Target Datasphere release / persona (e.g. "SAP Datasphere 2026-Q2", "BW Bridge", "Cloud Embedded Analytics")
    required: false
---

# BW → Datasphere migration planning

Convergent pattern from the BW-modernisation MCP servers catalogued in
[`marianfoo/sap-ai-mcp-servers`](https://github.com/marianfoo/sap-ai-mcp-servers).
The skill produces a **migration design document**, not a migration
execution. All operations are read-only.

**Target object:** `{{bw_object}}`
**Target platform:** `{{target_release}}`

## Step 1 — Object inventory

For `{{bw_object}} == "*"` (system-wide), enumerate the major BW
artefact tables:

| Artefact | Table | Key fields |
|---|---|---|
| InfoObjects | `RSDIOBJ` | IOBJNM |
| InfoProviders (DSO/ADSO/Cube) | `RSDODSO`, `RSADSO`, `RSDCUBE` | DSO / CUBE |
| Composite Providers (HCPR) | `RSOHCPR` | HCPR |
| BEx Queries | `RSZCOMPDIR` | COMPID, COMPTYPE |
| Process Chains | `RSPCCHAIN` | CHAIN_ID |
| Transformations | `RSTRAN` | TRANID |

For a single object, fetch its definition plus where-used:

```
sap.table.read table=<artefact_table>
  where_conditions=["<key_field> = '{{bw_object}}'"]

abap.adt.where_used object_name={{bw_object}}
```

## Step 2 — Classification matrix

Classify each artefact along two axes:

**Axis A — Migration path:**

| Class | Datasphere counterpart | Effort |
|---|---|---|
| ADSO (Standard) | Local Table (persisted) | Low |
| Composite Provider | Analytical Model | Medium |
| InfoCube (classic) | Redesign as ADSO → Analytical Model | **High** |
| Process Chain | Replication Flow + Task Chain | Medium |
| BEx Query (released) | Analytical Model + Story | Medium |
| BEx Query (custom code) | **Redesign** (no direct equivalent) | **High** |
| MultiProvider | Composite Provider intermediate, then Analytical Model | Medium |
| Open Hub Destination | Replication Flow (outbound) | Low |

**Axis B — Business criticality:**

| Class | Source | Action |
|---|---|---|
| Active in operational reports | `RSRREPDIR` last-run < 30 days | Migrate first |
| Reference only | last-run 30–365 days | Migrate in wave 2 |
| Stale | last-run > 365 days OR never | **Archive — do not migrate** |
| Custom code (ABAP routines in transformations) | non-empty `RSTRANROUT` | Flag for redesign in `kb.multi_hop` |

## Step 3 — Custom-code surfacing

Every BW transformation routine + start/end routine + expert routine is
ABAP code that *will not run as-is* in Datasphere. Enumerate them:

```
sap.table.read table=RSTRANROUT
  fields=TRANID,RULEID,RULETP,ROUTID
  where_conditions=["TRANID = '<transformation_id>'"]
```

For each `ROUTID`, fetch the ABAP source:

```
abap.adt.get_include name=GP_TRRT_<ROUTID>
```

Classify each routine:

| Routine pattern | Datasphere equivalent |
|---|---|
| Simple lookup against master data | View + JOIN in Analytical Model |
| Currency / unit conversion | Built-in Datasphere function |
| Filter / aggregation | Filter / Aggregation transform |
| Hardcoded business rule | **Manual rewrite as SQL view or Data Flow Python** |
| External call (RFC / HTTP) | **Replication Flow with secondary source** or external API task |
| BW-internal macro / variable | **Manual redesign** |

## Step 4 — Citation pass

Call `sap.docs.search` for the Datasphere migration handbook and the
official BW Bridge documentation. Cite both URIs in the report.

If the project is large (>50 BW objects), also call:

```
kb.global_query query="BW to Datasphere migration patterns"
```

to retrieve the community-summary roll-up across the knowledge graph.

## Step 5 — Wave plan

Produce a phased plan (3-wave default):

```markdown
## Wave 1 — Foundation (weeks 0-4)
- Inventory + classification (done — this skill output)
- Connectivity: Datasphere ↔ source SAP via Replication Flow
- 3 reference ADSOs migrated as proof of concept

## Wave 2 — Active workloads (weeks 4-12)
- All HIGH-criticality (active in operational reports) artefacts
- All Composite Providers with no custom code
- BEx queries linked to Wave 2 InfoProviders

## Wave 3 — Custom code redesign (weeks 12-24)
- Transformations with custom ABAP routines
- BEx queries with virtual key figures
- Process chains touching custom function modules

## Wave 4 — Decommission (weeks 24-26)
- Disable replication on source BW
- Archive stale artefacts (>365 day last-run)
- Final BW system shutdown sign-off
```

## Step 6 — Risk register

Always end with a risk register. Minimum entries:

| ID | Risk | Mitigation |
|---|---|---|
| R1 | BEx variable replacement breaks in Datasphere | Variable inventory pass; manual rewrite list |
| R2 | Reporting performance regression on first cut | Index-equivalent Analytical-Model design; baseline P95 capture |
| R3 | Authorisation model differs (BW analysis authorisations vs Datasphere spaces) | SoD audit (`sap.skill.security_sod_audit`) before cut-over |
| R4 | Custom ABAP routines have hidden side-effects (writes to Z-tables) | `abap.adt.where_used` on every routine before migration |

**No write operations** are performed by this skill. The deliverable is
a markdown design document the basis team can review and turn into
transports / change requests.
