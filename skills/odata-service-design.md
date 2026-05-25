---
name: sap.skill.odata_service_design
description: Design discipline for exposing a new SAP OData v2/v4 service as an MCP tool surface — the convergent OData-proxy pattern from marianfoo/sap-ai-mcp-servers. Captures the metadata-first / entity-mapping / auth-binding decisions in one place.
tags: [odata, design, proxy, integration]
requires_tools: [sap.docs.search, sap.system.info]
arguments:
  - name: service_name
    description: OData service technical name (e.g. "ZUI_PURCHASE_ORDER_O2")
    required: true
  - name: btp_destination
    description: BTP destination name OR direct base URL (e.g. "ZD_S4_API" or "https://my-s4.cfapps.eu10.hana.ondemand.com")
    required: false
---

# OData service design — generic proxy pattern

Convergent pattern from
[`marianfoo/sap-ai-mcp-servers`](https://github.com/marianfoo/sap-ai-mcp-servers)
and its anchor implementation
[`lemaiwo/odata-mcp-proxy`](https://github.com/lemaiwo/odata-mcp-proxy):
*one config-driven foundation can expose any SAP OData service as MCP
tools without per-service code*. This skill is the design checklist
that turns a service name into a stable, agent-friendly tool surface.

**Target service:** `{{service_name}}`
**Destination:** `{{btp_destination}}`

## Step 1 — Metadata first

Always fetch `$metadata` before tool design:

```
GET <base>/sap/opu/odata/sap/{{service_name}}/$metadata
Header: Accept: application/xml
```

Parse the `EntityType`, `EntitySet`, `Association`, `FunctionImport`,
and `Action` declarations. **Do not infer.** SAP services frequently
expose entities that look related but are not navigable (no
`NavigationProperty`).

Cite the resulting URI as a `sap-help://` reference in your output (use
`sap.docs.search` to find the service's official documentation).

## Step 2 — Tool surface design

Map OData primitives to MCP tool names following the
SAP-Automate convention `<domain>.<verb>.<entity>`:

| OData operation | MCP tool name | Read-only? |
|---|---|---|
| `GET /<EntitySet>` (with `$filter`) | `<domain>.search.<entity_plural>` | yes |
| `GET /<EntitySet>(<key>)` | `<domain>.get.<entity_singular>` | yes |
| `GET /<EntitySet>(<key>)/<NavProp>` | `<domain>.list.<navprop>` | yes |
| `POST /<EntitySet>` | `<domain>.create.<entity_singular>` | **no — gate behind `--enable-writes`** |
| `PATCH /<EntitySet>(<key>)` | `<domain>.update.<entity_singular>` | **no** |
| `DELETE /<EntitySet>(<key>)` | `<domain>.delete.<entity_singular>` | **no** |
| `<FunctionImport>` | `<domain>.<function_lower>` | depends on annotation |
| `<Action>` | `<domain>.<action_lower>` | **no — Actions always write** |

## Step 3 — Schema generation

For every tool, generate a JSON Schema from the OData EDM types:

| EDM type | JSON Schema |
|---|---|
| `Edm.String` (`MaxLength=N`) | `{"type":"string","maxLength":N}` |
| `Edm.Decimal` (`Precision`,`Scale`) | `{"type":"string","pattern":"^-?\\d+\\.\\d{0,<scale>}$"}` — strings, never floats, for SAP amounts |
| `Edm.DateTime` / `Edm.DateTimeOffset` | `{"type":"string","format":"date-time"}` |
| `Edm.Boolean` | `{"type":"boolean"}` |
| `Edm.Int32` | `{"type":"integer"}` |
| Navigation property | `$ref` to the nested entity schema |

Set `"additionalProperties": false` on every tool's schema — agents
must not invent fields.

## Step 4 — Auth binding

| Destination kind | Auth scheme | Where credentials live |
|---|---|---|
| On-prem via Cloud Connector | `BasicAuthentication` or `PrincipalPropagation` | BTP destination |
| BTP-internal | `OAuth2ClientCredentials` (service-key) | `AdtAuth::ServiceKey` |
| Public test (e.g. ES5) | `BasicAuthentication` | env / keyring via `LayeredCredentialProvider` |
| Production | OAuth 2.1 + PKCE | XSUAA (v1.2 roadmap) |

Never store credentials in the tool schema. Never log them. Audit log
must use `redact_secret()` from `sap-automate-observability`.

## Step 5 — Read-only safety posture

Default the new tool set to read-only. Mark write operations explicitly
via the server's `ExposurePolicy`:

```rust
ToolDescriptor::new("zui.create.purchase_order", schema)
    .with_writes()   // hidden from tools/list unless --enable-writes
    .with_description("...");
```

Mirror the AGENTS.md rule: write tools require the elicitation flow on
high-stakes entities (anything touching FI postings, customer master,
transports).

## Step 6 — Verify

Verifiable success criteria (per Karpathy goal-driven execution):

```
1. $metadata fetch returns 200 with valid EDM XML
   → verify: assertion in integration test
2. tools/list emits one MCP tool per OData entity + function import
   → verify: cardinality check in unit test
3. tools/call <domain>.search.<entity> returns a non-empty result on
   the live destination
   → verify: integration test against the destination
4. Every write tool is hidden from tools/list when --enable-writes is
   absent
   → verify: 7th SAP-precision test catches regression
5. Every write tool fires elicitation before the actual POST
   → verify: round-trip test against an elicitation-capable client
```

Produce a markdown design doc with one section per step. Do not start
coding until the design doc is reviewed.
