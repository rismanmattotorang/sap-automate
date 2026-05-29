# Runbook — connecting SAP-Automate to a development S/4HANA tenant

This is the end-to-end operator guide for pointing SAP-Automate at a **real
ParagonCorp development S/4HANA system** and verifying each live data path.
It consolidates the three transports built in Sprints 1–3 and the write
enablement from Sprint 4.

> All live paths are **secret-gated and skip cleanly when unconfigured**, so
> the offline test suite and CI are unaffected. Read-only is the default;
> writes require an explicit flag.

---

## 0. Prerequisites — the Basis hand-off

Before anything below works, obtain from your Basis team and confirm:

| Item | Needed for | Notes |
|---|---|---|
| HTTPS host + port (e.g. `https://s4dev:44300`) | ADT, OData, SOAP RFC | The ICM HTTPS port (`smicm`). |
| SAP client (e.g. `100`) | all | |
| Technical user + auth method | all | Basic, X.509/mTLS, or BTP/OAuth. |
| `/sap/bc/adt` active (SICF) | ADT | Eclipse ADT uses the same node. |
| `/sap/bc/soap/rfc` active (SICF) | SOAP RFC | Often disabled by default — ask explicitly. |
| OData service published | OData | e.g. `API_BUSINESS_PARTNER` in `/IWFND/MAINT_SERVICE`. |
| Network reachability from the runtime | all | Firewall / SAProuter / proxy. |

Network reachability check from the runtime host:

```bash
curl -ksS -o /dev/null -w "%{http_code}\n" https://s4dev:44300/sap/bc/adt/discovery
```

---

## 1. ADT (ABAP read) — `abap.adt.*`

Create a destination file (credentials live here — the directory is
gitignored and the server never logs the password/token):

```bash
mkdir -p ./.sap-automate/destinations
cp deploy/sap-automate-destination.example.toml \
   ./.sap-automate/destinations/dev-s4.toml
$EDITOR ./.sap-automate/destinations/dev-s4.toml   # set base_url / client / [auth]
chmod 600 ./.sap-automate/destinations/dev-s4.toml  # the server warns if group/other-readable
```

Auth options in the `[auth]` block: `basic`, `bearer`, `service_key` (BTP
XSUAA — `path` to the service-key JSON), or `certificate` (mTLS — `cert_path`
+ `key_path`). Run and smoke-test:

```bash
SAP_AUTOMATE_DESTINATION=dev-s4 ./target/release/sap-automate-server
# logs: "ADT client: live HttpAdtClient against real SAP system"

SAP_AUTOMATE_DESTINATION=dev-s4 \
  cargo test -p sap-automate-server --test live_adt -- --nocapture
```

---

## 2. OData (read) — `sap.bp.*`

```bash
export SAP_ODATA_BASE_URL="https://s4dev:44300"   # bare host → BP path appended
export SAP_ODATA_AUTH=basic                        # basic | bearer | oauth
export SAP_ODATA_USER="TECHUSER"
export SAP_ODATA_PASSWORD="…"
# OAuth2 (BTP/XSUAA) instead:
#   SAP_ODATA_AUTH=oauth SAP_ODATA_TOKEN_URL=… SAP_ODATA_CLIENT_ID=… SAP_ODATA_CLIENT_SECRET=… [SAP_ODATA_SCOPE=…]

./target/release/sap-automate-server   # logs: "OData v4 backend active … target=tenant"

SAP_ODATA_BASE_URL=… SAP_ODATA_AUTH=basic SAP_ODATA_USER=… SAP_ODATA_PASSWORD=… \
  cargo test -p sap-automate-rfc --features odata live_business_partner_search -- --nocapture
```

`SAP_ODATA_BASE_URL` takes precedence over `SAP_BUSINESS_HUB_KEY` (the public
sandbox). A full OData service-root URL is used verbatim; a bare host gets the
`API_BUSINESS_PARTNER` v4 path appended.

---

## 3. SOAP RFC (read) — `sap.table.read`, `sap.system.info`, `sap.rfc.call`

```bash
export SAP_RFC_HTTP_URL="https://s4dev:44300"
export SAP_RFC_CLIENT="100"
export SAP_RFC_USER="TECHUSER"
export SAP_RFC_PASSWORD="…"
# SAP_RFC_LANG defaults to EN

./target/release/sap-automate-server
# logs: "live SOAP RFC backend active (data ops); metadata via curated catalogue"

SAP_RFC_HTTP_URL=… SAP_RFC_USER=… SAP_RFC_PASSWORD=… \
  cargo test -p sap-automate-rfc --features soap live_read_table_t000 -- --nocapture
```

Metadata (`sap.rfc.metadata` / `sap.rfc.search`) and the **read-only safety
gate** stay served by SAP-Automate's curated catalogue: a state-modifying or
*uncatalogued* function is refused in read-only mode (fail-closed).

---

## 4. Writes (gated) — `sap.rfc.call` with `commit=true`

> ⚠️ Only against a **development** tenant. Writes are off by default.

```bash
SAP_RFC_HTTP_URL=… SAP_RFC_USER=… SAP_RFC_PASSWORD=… \
  ./target/release/sap-automate-server --enable-writes
```

Call a write BAPI transactionally — the server runs the BAPI then
`BAPI_TRANSACTION_COMMIT` on success or `BAPI_TRANSACTION_ROLLBACK` on a
BAPIRET2 error:

```jsonc
// tools/call
{
  "name": "sap.rfc.call",
  "arguments": {
    "function": "BAPI_PO_CREATE1",
    "parameters": { "POHEADER": { /* … */ }, "POHEADERX": { /* … */ }, "POITEM": [ /* … */ ] },
    "commit": true
  }
}
```

Safety properties (verified in Sprint-4 security review):

- **Fail-closed read-only gate** at three layers (server default, the write
  helper, and `call_rfc`'s catalogue check). `commit=true` in read-only mode
  is rejected before any RFC fires.
- **Fail-closed on ambiguity**: a BAPI that returns *no* parseable BAPIRET2
  is treated as *unconfirmed* and **not** committed (rolled back).
- **Verified rollback**: `rolled_back` reflects the actual rollback result,
  not an assumption.
- **Audit log**: every state-mutating call (`sap.rfc.call commit=true` and the
  `sap.workflow.*` tools) records a redacted `AuditEntry` — event id,
  timestamp, tool, SAP system, **redacted** arguments (secrets/PII stripped),
  outcome, duration. By default these are emitted as JSON on the `sap_audit`
  `tracing` target (stderr); point your log pipeline at it, or wire a
  tamper-evident `AuditSink` (Loki / S3 object-lock / Splunk HEC) for SOX/GDPR
  evidence.

---

## 5. Security checklist before enabling writes

- [ ] Destination / service-key / key files are `0600` (server warns otherwise).
- [ ] Technical user has only the authorizations the use-case needs (`S_RFC`,
      `S_TABU_DIS`, the relevant BAPI auth groups) — least privilege.
- [ ] TLS verification is on (default; never disable certificate validation).
- [ ] `--enable-writes` is set **only** for the dev tenant, never prod.
- [ ] Audit log (`sap_audit` target) is captured by your log pipeline.

---

## 6. Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| `401`/`403` on ADT/OData/RFC | bad creds, locked user, missing auth | check user in `SU01`; confirm auth method matches `[auth]`/env |
| ADT `CsrfRefresh` error | CSRF node disabled / token rotation | confirm `/sap/bc/adt` active; retry (token re-fetched) |
| SOAP RFC connection refused | `/sap/bc/soap/rfc` SICF node inactive | ask Basis to activate the node |
| `function … not in the curated read-only catalogue` | uncatalogued RFC in read-only mode | expected fail-closed; use `--enable-writes` for a real write, or add the function to the catalogue |
| Write returns `committed=false … unconfirmed` | BAPI returned no BAPIRET2 | inspect the raw `result`; the change was rolled back, not persisted |
| OData hits the sandbox not the tenant | `SAP_ODATA_BASE_URL` unset | set it; it overrides `SAP_BUSINESS_HUB_KEY` |

---

## 7. Observability

The HTTP transport exposes Prometheus metrics for live tuning:

```bash
./target/release/sap-automate-server --transport http --bind 127.0.0.1:3030 &
curl -s http://127.0.0.1:3030/metrics
```

Key series: `mcp_tool_latency_seconds`, `mcp_tool_calls_total`,
`mcp_tool_errors_total`, `sap_rfc_calls_total`, `sap_authz_denied_total`,
`sap_pool_in_use`. Import `deploy/grafana/sap-automate-overview.json` into
Grafana for a ready-made overview (latency budget, error rate, RFC volume,
authz denials). Tune `timeout` / retry / circuit-breaker thresholds against
the real-call latencies you observe here.
```
