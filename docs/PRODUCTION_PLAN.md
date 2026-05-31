# SAP-Automate — Production Readiness Assessment & Sprint Plan

> **Goal of this document:** get the codebase to a state where it can be
> *tested against a real Gaussian Technologies development S/4HANA system* — not the
> public Business Hub sandbox, not in-memory fixtures, but a live dev tenant.
>
> **Status as assessed:** 2026-05-29. Build healthy (16 crates + 8 apps
> compile on stable Rust; 172 tests pass). **However, every test today runs
> offline against in-memory fixtures. No working data path to a real
> customer S/4HANA system exists yet.** This plan closes that gap.

---

## 1. Honest assessment — claim vs. reality

The architecture, MCP protocol surface, RAG/graph engines, and packaging are
genuinely strong and well-tested. The gap is concentrated entirely in the
**live SAP connectivity layer**, which is where "test on dev S/4HANA" lives.

| Capability | README/ROADMAP framing | Actual state in code | Real against dev S/4HANA? |
|---|---|---|---|
| RFC / BAPI calls (`sap.rfc.call`) | "12 SAP system / RFC tools" | **`MockSapClient` only.** No real RFC transport exists in `sap-automate-rfc`. | ❌ No |
| Table reads (`sap.table.read`, `RFC_READ_TABLE`) | listed as tools | Seeded fixtures in `client.rs` | ❌ No |
| System info / health / cache | tools listed | Fixtures | ❌ No |
| ADT read (`abap.adt.get_*`, `where_used`) | "`HttpAdtClient` complete, 17 tests" | `HttpAdtClient` **is real reqwest code**, but **the server binary only ever instantiates `MockAdtClient`** (`main.rs:171`). No CLI/config path switches to HTTP. Tested only vs. axum mock. | ⚠️ Client exists, **not wired**, never hit a real ABAP stack |
| ADT write (`abap.adt.activate`) | "write, gated" | Mock returns `"activated (mock)"` | ❌ No |
| OData (`sap.bp.search/get`) | "Live SAP backend" | `BusinessHubClient` is real reqwest, but **hardcoded to `sandbox.api.sap.com`** with `APIKey` header — a public demo host, not a tenant. | ⚠️ Real HTTP, **wrong target**; can't reach a customer system |
| Workflows (PO / customer / transport) | "write, gated" | All return `"(mock execution)"` strings | ❌ No |
| ADT auth: ServiceKey / Certificate | "XSUAA-ready" | Returns `"not yet wired (Phase 7)"` | ❌ No |
| Signavio / LeanIX connectors | "ABAP + BPMN + LeanIX" | `sap-automate-connectors` is a 40-line stub | ❌ No |

**Bottom line:** the only two pieces of real network code (`HttpAdtClient`,
`BusinessHubClient`) cannot today reach a Gaussian Technologies dev tenant — one is
unwired, the other points at SAP's public sandbox. Closing those two gaps,
plus choosing an RFC transport, is the whole job.

---

## 2. Production-readiness scorecard

| Dimension | Score | Note |
|---|---|---|
| Protocol / MCP conformance | 🟢 strong | 2025-06-18 surface is real and tested |
| Build / CI hygiene | 🟢 strong | fmt, clippy, test matrix, K8s lint all wired |
| RAG / graph engines | 🟢 strong | real algorithms, benchmarked |
| Packaging / deploy | 🟢 strong | distroless, K8s manifests, runbook |
| **Live SAP read path** | 🔴 **blocking** | no wired real client to a dev tenant |
| **Live SAP write path** | 🔴 **blocking** | 100% mock; no `BAPI_TRANSACTION_COMMIT` ever fires |
| **SAP auth (OAuth/XSUAA/basic-against-tenant)** | 🟠 partial | basic+bearer coded for ADT; nothing wired end-to-end |
| Secrets / destination config | 🟠 partial | env+static creds; destination TOML loader behind feature |
| Real-system integration tests | 🔴 missing | zero tests touch a real SAP host |
| Observability against live calls | 🟠 partial | metrics exist; never exercised on real latency |

---

## 3. The critical path — three real transports, in priority order

For a dev-tenant test we do **not** need the non-redistributable NetWeaver
RFC SDK. Three pure-HTTP paths reach S/4HANA and are achievable in Rust:

1. **ADT REST (`/sap/bc/adt/...`)** — *lowest effort, highest value first.*
   The `HttpAdtClient` already exists with CSRF handling and Basic/Bearer
   auth. The only work is **wiring it into the server** via a destination
   config and proving it against the dev tenant. This unlocks every
   `abap.adt.*` read tool immediately.

2. **OData v2/v4 (`/sap/opu/odata|odata4/...`)** — *generalize what exists.*
   Refactor `BusinessHubClient` into a host-configurable `OdataClient` that
   targets the tenant's own services with Basic or OAuth client-credentials.
   This makes `sap.bp.*` real against the dev system and gives a generic
   read path for Business Partner, Material, Sales Order, ACDOCA cubes, etc.

3. **SOAP RFC (`/sap/bc/soap/rfc`)** — *the real RFC path without the C SDK.*
   Implement a `SoapRfcClient: SapClient` that posts SOAP envelopes to the
   SAP SOAP runtime. This is what turns `sap.rfc.call`, `RFC_READ_TABLE`,
   and the write BAPIs (with real `BAPI_TRANSACTION_COMMIT`) from fixtures
   into live calls. Larger effort; gated and read-only-first.

> The NetWeaver RFC SDK (FFI) remains a *later* option for performance-
> critical native RFC, but it is **not on the critical path** for dev-tenant
> testing and carries redistribution/licensing friction.

---

## 4. Recommended Claude Code skills (and how to use each)

The user asked which Claude Code skills best fit this work. Mapping the
available skills to the sprint tasks:

| Skill | Where it's used in this plan | Why |
|---|---|---|
| **`init`** | Sprint 0 | Refresh/create `CLAUDE.md` so every future session has the backend-wiring map and test commands. |
| **`session-start-hook`** | Sprint 0 | Add a SessionStart hook so web/CI sessions auto-build + run the offline test suite and can reach the (secret-gated) dev tenant. |
| **`deep-research`** | Sprints 1–3 (spikes) | Pin down exact ADT CSRF flow, OData OAuth client-credentials against S/4HANA, and the SOAP RFC envelope format + `RFC_READ_TABLE` quirks before coding. |
| **`Plan` agent** | Start of each sprint | Turn each sprint's goal into a concrete file-level implementation plan. |
| **`autopilot`** | Sprints 1–3 implementation | Self-contained "wire HttpAdtClient into server + dev-tenant integration test" style tasks — plan→critique→implement→bughunt→PR. |
| **`bugfix`** | Throughout | When a live call fails against the dev tenant, reproduce-first against a recorded fixture, then fix. |
| **`verify`** / **`run`** | End of each sprint | Actually launch `sap-automate-server` pointed at the dev tenant and observe a real tool call returning real data. |
| **`investigate`** | When live calls misbehave | Root-cause auth/CSRF/timeout failures against the real system without guessing. |
| **`code-review`** | Every PR | Correctness pass on the new network + auth code. |
| **`security-review`** | Sprint 4 (gate) | Mandatory before any write-enabled run against a real system — credential handling, secret redaction, CSRF, TLS. |
| **`docs`** | Sprint 5 | Update `INTEGRATION.md` with the real dev-tenant onboarding runbook. |
| **`update-config`** / **`fewer-permission-prompts`** | Sprint 0 | Allowlist the cargo/curl commands this work repeats so sessions run with fewer prompts. |
| **`dashboard`** | Sprint 5 (optional) | Grafana panels for live-call latency/error rate once real traffic exists. |

**Recommended default driver:** use **`autopilot`** for each connectivity
sprint (it plans, critiques, implements, bug-hunts, and opens a PR), bracketed
by a **`deep-research`** spike before and **`verify`** + **`security-review`**
after.

---

## 5. Sprint plan

Sprints are ~1 week. Each has a single demoable acceptance gate. The ordering
front-loads the lowest-effort real connectivity (ADT) so there is a live
dev-tenant result by end of Sprint 1.

### Sprint 0 — Foundations & dev-tenant access (prep) — ✅ DONE
**Shipped:** destination TOML loader (`AdtDestination::load` / `load_from_path`
/ `config_search_paths`, behind the `http` feature), `--destination` CLI flag
+ `SAP_AUTOMATE_DESTINATION` env, secret-free `AdtAuth::label()`, credential
files gitignored, `deploy/sap-automate-destination.example.toml` template, and
4 offline loader unit tests (incl. a password-non-leak assertion). The one
outstanding Sprint-0 item — obtaining real dev-tenant connection details from
Basis — is an organisational dependency, not code.

**Goal:** anyone can configure a real destination and run the suite.
- Obtain dev S/4HANA connection details (host, client, technical user, auth
  method) from Basis. Confirm network reachability from the runtime.
- Implement/finish the **destination TOML loader** (`~/.config/sap-automate/
  destinations/<name>.toml`) behind the `http` feature; document the schema.
- Add `SAP_AUTOMATE_DESTINATION` env + `--destination <name>` CLI plumbing
  (no behavior change yet — still defaults to mock).
- Secrets: never log credentials; confirm redaction covers the new fields.
- **Skills:** `init`, `session-start-hook`, `update-config`.
- **Gate:** `--destination` flag parses and loads a TOML; offline tests stay green.

### Sprint 1 — Live ADT read against the dev tenant ⭐ first real result — ✅ DONE (code), pending dev-tenant run
**Shipped:** `HttpAdtClient` is now wired into the server (`build_adt_client`
in `main.rs`) — a non-mock destination builds the live client instead of the
mock; the "Phase 7" stub path is gone. `manual_div_ceil` clippy fix in the
ADT base64 helper. Secret-gated `tests/live_adt.rs` (skips without
`SAP_AUTOMATE_DESTINATION`, so CI stays green). Verified end-to-end against the
binary: missing destination errors cleanly, a basic-auth destination logs the
live HttpAdtClient path and serves `/health`, an `auth=mock` destination falls
back safely. **Remaining:** point it at the real dev tenant and confirm
`get_class` returns live source (needs the Sprint-0 Basis credentials).

**Goal:** `abap.adt.get_class` returns real source from dev S/4HANA.
- Spike (`deep-research`): confirm dev-tenant ADT base path, CSRF fetch, and
  Basic-auth handshake.
- Wire `HttpAdtClient` selection into `main.rs`/`context.rs` when the chosen
  destination's auth ≠ `Mock` (remove the "Phase 7" stub path).
- Add a **secret-gated live integration test** (mirrors the gated Business
  Hub test pattern) that auto-skips without `SAP_AUTOMATE_DESTINATION`.
- **Skills:** `deep-research` → `autopilot` → `verify`, then `code-review`.
- **Gate:** with a real destination set, `abap.adt.get_class` + `where_used`
  return live data; without it, suite still green.

### Sprint 2 — Live OData read (generalize BusinessHubClient) — ✅ DONE (code), pending dev-tenant run
**Shipped:** `OdataAuth` enum (ApiKey / Basic / Bearer / OAuth2
client-credentials) with secret-safe `Debug` + `label()`; `BusinessHubConfig`
now carries `auth` and gained `tenant_business_partner(host, auth)` + generic
`new(base_url, auth)` constructors. `BusinessHubClient::from_env()` prefers a
tenant (`SAP_ODATA_BASE_URL` + `SAP_ODATA_AUTH`/creds) over the sandbox, with
on-demand OAuth2 token fetch + cache (refresh 60 s early). Sandbox profile
unchanged (no regression). 5 new offline tests incl. secret-non-leak; the
gated `live_business_partner_search` now covers tenant or sandbox. Verified
end-to-end: tenant/sandbox/bad-oauth startup paths log correctly. **Remaining:**
run against the real tenant once Basis provides OData credentials.

**Goal:** `sap.bp.search` hits the tenant's own `API_BUSINESS_PARTNER`.
- Refactor `BusinessHubClient` → host-configurable `OdataClient` (base URL,
  auth = Basic | Bearer | OAuth client-credentials; APIKey becomes one mode).
- Add OAuth2 client-credentials token fetch + refresh for BTP-fronted tenants.
- Keep the public sandbox working as one configured profile (no regression).
- **Skills:** `deep-research` (OAuth flow) → `autopilot` → `verify`.
- **Gate:** `sap.bp.search` returns real BPs from the dev tenant via Basic
  and via OAuth; sandbox profile still passes the existing gated test.

### Sprint 3 — Live RFC read via SOAP (`SoapRfcClient`) — ✅ DONE (code), pending dev-tenant run
**Shipped:** `SoapRfcClient` (rfc crate, `soap` feature) posts SOAP envelopes
to `/sap/bc/soap/rfc`. Live data ops — `read_table` (RFC_READ_TABLE, DELIMITER
mode), `system_info` (RFC_SYSTEM_INFO), `table_structure` (DDIF_FIELDINFO_GET),
generic `call_rfc`. Metadata + the read-only gate delegate to the curated
catalogue (fail-closed: uncatalogued/state-modifying functions refused in
read-only mode). Generic JSON⇄SOAP-XML codec (namespace-stripping, repeated
tags→arrays, SOAP-fault surfacing, XML escaping). Wired into the server via
`SAP_RFC_HTTP_URL` + `SAP_RFC_*` (decoupled from the native-RFC credential
chain). 12 offline tests (envelope/codec/parsers/gate/caps) + gated live test.
Verified: backend selection logs correctly; offline mock preserved.
**Remaining:** run against the real tenant (needs `/sap/bc/soap/rfc` active +
credentials); structure-aware client (MANDT) scoping deferred to hardening.

**Goal:** `sap.table.read` (`RFC_READ_TABLE`) returns real rows.
- Spike (`deep-research`): SOAP RFC envelope shape, `RFC_READ_TABLE`
  field/option/data semantics, 512-byte row cap, error/`BAPIRET2` mapping.
- Implement `SoapRfcClient: SapClient` (read methods first: `system_info`,
  `rfc_metadata`, `read_table`, `table_structure`). Reuse `parse_bapiret2`.
- Select it via destination config; **read-only enforced**.
- **Skills:** `deep-research` → `autopilot` → `bugfix` (for parser edge
  cases) → `verify`.
- **Gate:** `sap.table.read` on a small standard table (e.g. `T001`) returns
  real rows from the dev tenant.

### Sprint 4 — Security hardening & write-path enablement (gated) — ✅ DONE (code), pending dev-tenant run
**Shipped:** ADT **ServiceKey (XSUAA)** auth — loads a BTP service key, fetches
an OAuth2 client-credentials token, caches it (refresh 60 s early); ADT
**Certificate (mTLS)** auth via `reqwest::Identity` from cert+key PEM (the
"Phase 7 stub" is gone). Transactional write helper `execute_write_bapi`
(rfc crate) — calls a BAPI then `BAPI_TRANSACTION_COMMIT`/`ROLLBACK` based on
BAPIRET2; wired to `sap.rfc.call` via `commit=true`. Per-write `sap_audit` log
line (function + outcome only).

**Mandatory security review run** (2 agents) — fixes applied:
- **Fail-closed commit decision**: a BAPI returning no parseable BAPIRET2 is
  treated as *unconfirmed* and rolled back, never committed on faith.
- **Verified rollback**: `rolled_back` reflects the real rollback result.
- File-permission warnings on the new service-key JSON + mTLS key files.
- Manual `Debug` on `AdtAuth` so passwords/tokens can't leak via `{:?}`.
Review confirmed the read-only gate is fail-closed at three layers (no
`commit=true` bypass) and credential error paths leak no secrets.

**Audit (now complete):** the full `AuditLog` / `AuditSink` is wired into the
server. Every state-mutating call — `sap.rfc.call` with `commit=true` and the
three `sap.workflow.*` tools — records a redacted `AuditEntry` (event id,
timestamp, tool, SAP system, redacted args, outcome, duration). Default sink
routes JSON lines through the `sap_audit` `tracing` target (stderr — safe for
the stdio MCP channel); production can swap in a tamper-evident sink
(Loki / S3 object-lock / Splunk HEC). 2 integration tests cover the write path.
**Remaining:** a real PO-create against the dev tenant (needs writes + creds).

**Goal:** safe to run with `--enable-writes` against a *dev* tenant.
- Implement ServiceKey (XSUAA) + Certificate (mTLS) auth paths now stubbed.
- Real write workflows: wire `BAPI_*` create + mandatory
  `BAPI_TRANSACTION_COMMIT` through `SoapRfcClient`; keep elicitation +
  re-typed-confirmation guardrails.
- Mandatory **`security-review`**: credential lifecycle, TLS verification,
  CSRF on writes, audit-log completeness, no secret leakage.
- **Skills:** `security-review` (blocking gate), `code-review`, `verify`.
- **Gate:** a PO create in the dev tenant produces a real document number,
  fully audit-logged; security review signed off before merge.

### Sprint 5 — Live observability, docs & runbook — ✅ DONE (docs/dashboard); live tuning pending real traffic
**Shipped:** `docs/RUNBOOK_DEV_TENANT.md` — a single end-to-end operator guide
(Basis hand-off checklist, ADT/OData/SOAP setup, gated writes, security
checklist, troubleshooting table, observability). Grafana dashboard
`deploy/grafana/sap-automate-overview.json` (tool-latency P95/P99 vs the 80 ms
gate, error rate, call volume, RFC volume, authz denials, pool usage) over the
existing Prometheus series. **Remaining:** tune timeout/retry/circuit-breaker
thresholds against real dev-tenant latencies once live traffic exists (needs
the tenant connection).

**Goal:** an operator can onboard a new SAP system unaided.
- Exercise Prometheus metrics + audit log against real-call latency; tune
  timeouts/retries/circuit-breaker thresholds with real numbers.
- Rewrite `docs/INTEGRATION.md` Tier-3 into a true dev-tenant runbook
  (destination TOML, auth setup, smoke-test sequence, troubleshooting).
- Optional Grafana dashboard for live-call latency/error rate.
- **Skills:** `docs`, `dashboard` (optional), `verify`.
- **Gate:** a fresh operator follows the runbook and gets a live cited answer
  end-to-end against the dev tenant.

---

## 6. Definition of done — "testable on dev S/4HANA"

The goal is met when, against the Gaussian Technologies dev tenant:

1. A configured destination drives a **real** ADT read, OData read, and SOAP
   RFC `RFC_READ_TABLE` — all returning live data.
2. At least one **write** workflow commits a real document under the
   read-only/elicitation/confirmation guardrails, fully audit-logged.
3. Live integration tests exist for each path, **secret-gated** so CI without
   tenant access stays green (the existing 172 offline tests must not regress).
4. A security review of the credential/TLS/CSRF/write surface is signed off.
5. `docs/INTEGRATION.md` documents the dev-tenant onboarding from zero.

---

## 7. Top risks

- **Dev-tenant access & auth method unknown** — Sprint 0 must resolve host,
  client, technical user, and whether auth is Basic, X.509, or BTP/OAuth.
  Everything downstream depends on this. *Mitigation: front-loaded in Sprint 0.*
- **SOAP RFC may be disabled** on the tenant (`SICF` node `/sap/bc/soap`).
  *Mitigation: confirm with Basis early; OData read path (Sprint 2) is a
  fallback for many read use-cases if SOAP RFC is closed.*
- **CSRF / session handling under load** — real systems rotate tokens.
  *Mitigation: the existing CSRF cache must be validated against the real
  stack in Sprint 1, not assumed correct from the mock.*
- **Doc/claim drift** — README/ROADMAP currently describe live wiring that
  isn't wired. *Mitigation: update those surfaces as each sprint lands so the
  public claims track reality.*
