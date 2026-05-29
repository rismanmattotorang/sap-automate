# SAP-Automate — SAP S/4HANA integration testing

SAP-Automate supports three tiers of SAP integration, picked by what
you actually need:

| Tier | What you get | Cost | When to use |
|---|---|---|---|
| **1. CI** | In-process axum mocks for ADT + RFC fixtures | free | unit / regression tests; no network |
| **2. Demo** | **SAP Business Accelerator Hub sandbox** — live OData v4 against real SAP-published endpoints | free SAP Community login; rate-limited | smoke-testing real SAP semantics; demoing the tool surface; this doc covers it |
| **3. Power-user** | `sapse/abap-platform-trial` Docker — full ABAP Platform on HANA 2.0, real RFC + ADT + DDIC | ~50 GB disk, 32 GB RAM | end-to-end on-prem testing; the only path that exercises real RFC |

---

## Tier 2: SAP Business Accelerator Hub sandbox

The sandbox at `sandbox.api.sap.com` exposes OData v4 endpoints for
**hundreds of S/4HANA Cloud Public Edition APIs** — `API_BUSINESS_PARTNER`,
`API_MATERIAL`, `API_PURCHASEORDER_PROCESS_SRV`, `API_SALES_ORDER_SRV`,
`API_OPLACCTGDOCITEMCUBE_SRV` (ACDOCA), etc.  SAP-Automate v1.3 ships a
client for the V4 `API_BUSINESS_PARTNER` service; other services slot in
behind the same `BusinessHubClient` shape.

### 1. Get an API key

1. Sign in (or sign up — free) at [SAP Business Accelerator Hub](https://api.sap.com/).
2. Open the [`API_BUSINESS_PARTNER` v4 package](https://api.sap.com/api/API_BUSINESS_PARTNER).
3. Click **Show API Key** (top-right).  The key is unique to your SAP account.

The key is sent as `APIKey: <value>` on every request — never as a URL
parameter, never logged by SAP-Automate.

### 2. Wire it into the server

```bash
export SAP_BUSINESS_HUB_KEY="<paste-your-key-here>"
./target/release/sap-automate-server
```

On startup the server logs:

```
SAP Business Accelerator Hub sandbox active  base_url=https://sandbox.api.sap.com/s4hanacloud/...
```

If the env var is unset you'll see the friendly fallback:

```
SAP_BUSINESS_HUB_KEY not set — Business Hub tools disabled
```

The `sap.bp.*` MCP tools are still registered in this case — they just
return a clean error pointing the caller back here.

### 3. Drive it from any MCP client

```jsonc
// tools/call
{
  "name": "sap.bp.search",
  "arguments": { "query": "Smith", "limit": 5 }
}
```

Sample response (one row):

```json
{
  "query": "Smith",
  "count": 5,
  "results": [
    {
      "id": "1003764",
      "full_name": "Anna Smith",
      "category": "1",
      "organization_name": null,
      "first_name": "Anna",
      "last_name": "Smith",
      "grouping": "BP02",
      "creation_date": "2021-08-15T00:00:00Z"
    }
    // ...
  ]
}
```

And `sap.bp.get`:

```jsonc
{
  "name": "sap.bp.get",
  "arguments": { "id": "1003764" }
}
```

### 4. Run the live integration test locally

The integration test lives in
`crates/sap-automate-rfc/src/odata.rs` (`live_business_partner_search`).
It auto-skips when no key is set, so CI without secrets is unaffected.

```bash
# With key — runs the live round-trip:
SAP_BUSINESS_HUB_KEY="<your-key>" \
  cargo test -p sap-automate-rfc --features odata live_business_partner_search -- --nocapture

# Without key — skips with a printed message:
cargo test -p sap-automate-rfc --features odata live_business_partner_search
```

### 5. Rate limits + best practices

- The sandbox is **shared infrastructure** — be polite.  Treat it like a
  free public API.
- SAP-Automate's `reqwest::Client` is configured with a 15-second
  timeout per request.
- The `BusinessHubClient` issues `$select` queries to keep row sizes
  predictable.
- Cache hits across re-runs do *not* count against the rate limit if
  you reuse responses — write your own caching layer for production
  workloads.

### 6. Extending to other services

Adding `API_MATERIAL`, `API_SALES_ORDER_SRV`, etc. follows the same
pattern as `business_partner_sandbox`:

```rust
impl BusinessHubConfig {
    pub fn material_sandbox(api_key: impl Into<String>) -> Self {
        Self {
            base_url: "https://sandbox.api.sap.com/s4hanacloud/sap/opu/odata4/sap/api_material/srvd_a2x/sap/material/0001".into(),
            api_key: api_key.into(),
            timeout: Duration::from_secs(15),
        }
    }
}
```

Plus a matching projection struct + tool registration.  Pull requests
welcome.

---

## Tier 1: in-process CI mocks

Already shipped.  See:

- `crates/sap-automate-adt/tests/http_integration.rs` — 17 tests
  exercising every `HttpAdtClient` URL pattern, header, CSRF flow,
  XML parser path against an axum-fixture server.
- `apps/sap-automate-server/tests/cache_tools.rs`,
  `kb_navigate.rs`,
  `spec_utilities.rs`,
  `business_partner.rs` — in-process tool-surface tests via
  `tokio::io::duplex`.

These tests run in **< 0.1 s** total and require no external state.

---

## Tier 3: ABAP Platform Trial Docker

For full RFC + ADT against a real ABAP system on your own hardware:

1. Pull `sapse/abap-platform-trial` from the SAP Docker registry
   (see [SAP-docs/abap-platform-trial-image](https://github.com/SAP-docs/abap-platform-trial-image)
   for current image tags and license refresh procedure).
2. Allocate **32 GB RAM** to the container.  SAP officially recommends
   this; less may run but degrades severely.
3. Initial start-up takes 10–20 minutes on first run.
4. Point `HttpAdtClient` at the system by creating a **destination file**
   and selecting it with `--destination` (or `SAP_AUTOMATE_DESTINATION`):

   ```bash
   mkdir -p ./.sap-automate/destinations
   cp deploy/sap-automate-destination.example.toml \
      ./.sap-automate/destinations/dev-s4.toml
   # edit base_url / client / [auth] (e.g. DEVELOPER + container password)

   SAP_AUTOMATE_DESTINATION=dev-s4 ./target/release/sap-automate-server
   # logs: "ADT client: live HttpAdtClient against real SAP system"
   ```

   Search order: `$SAP_AUTOMATE_DESTINATION_DIR`,
   `./.sap-automate/destinations/`, `~/.config/sap-automate/destinations/`.
   The destination file holds credentials — `.sap-automate/destinations/`
   is gitignored and the server never logs the password/token.

   Smoke-test the live path (auto-skips without a destination):

   ```bash
   SAP_AUTOMATE_DESTINATION=dev-s4 \
     cargo test -p sap-automate-server --test live_adt -- --nocapture
   ```

   The 17 ADT integration tests still exercise the axum mock; the
   `live_adt` test exercises the real ABAP stack.
5. RFC integration requires the SAP NetWeaver RFC SDK (free download,
   not redistributable — sign-in required at the SAP Software Centre).

This tier is **not run in CI** and not exercised by the default
`cargo test` workspace sweep.

---

## Decision tree

```
Do you need real SAP semantics?
├── No   → Tier 1 (default; runs everywhere)
└── Yes
    ├── Read-only OData is enough?
    │   └── Yes → Tier 2 (SAP_BUSINESS_HUB_KEY; this doc)
    └── Need real RFC + ADT + writes?
        └── Yes → Tier 3 (ABAP Platform Trial Docker)
```
