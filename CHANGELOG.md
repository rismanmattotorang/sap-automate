# Changelog

All notable changes to **SAP-Automate** are documented here.  The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.4.0] — 2026-05-29  ·  Dev-tenant live wiring, enterprise auth, gated writes, audit

Turns the "live SAP backend" tier from a public-sandbox demo into a
path that reaches a **real customer S/4HANA development tenant** over
three pure-HTTP transports — no NetWeaver RFC SDK required.  All
additive; the offline mock remains the default and CI without SAP
secrets is unaffected (the live integration tests skip cleanly).

### Added — live transports

- **ADT REST (live).** `HttpAdtClient` is now wired into the server via a
  destination TOML selected with `--destination` / `SAP_AUTOMATE_DESTINATION`
  (search path: `$SAP_AUTOMATE_DESTINATION_DIR`, `./.sap-automate/destinations/`,
  `~/.config/sap-automate/destinations/`).  `AdtDestination::load` +
  permission warnings on credential files.
- **OData (tenant).** `BusinessHubClient` generalised beyond the sandbox:
  new `OdataAuth` (ApiKey / Basic / Bearer / **OAuth2 client-credentials**
  with token cache), `tenant_business_partner()` + generic `new()`
  constructors, env-driven `from_env()` (`SAP_ODATA_*`) that prefers a
  tenant over the sandbox.
- **SOAP RFC (live).** New `SoapRfcClient` (feature `soap`) over
  `/sap/bc/soap/rfc`: real `RFC_READ_TABLE` (DELIMITER mode),
  `RFC_SYSTEM_INFO`, `DDIF_FIELDINFO_GET`, and generic `call_rfc`.
  Metadata + the read-only safety gate delegate to the curated catalogue
  (fail-closed for state-modifying / uncatalogued functions).  Configured
  via `SAP_RFC_*`, decoupled from the native-RFC credential chain.

### Added — enterprise auth

- ADT **ServiceKey (XSUAA)** auth — loads a BTP service key, runs the
  OAuth2 client-credentials grant, caches the token (refresh 60 s early).
- ADT **Certificate (mTLS)** auth — `reqwest::Identity` from cert+key PEM.
- The previous "ServiceKey / Certificate not yet wired (Phase 7)" stub is
  gone; auth resolution is async with a token cache.

### Added — gated transactional writes

- `sap_automate_rfc::transaction::execute_write_bapi` — calls a write BAPI
  then `BAPI_TRANSACTION_COMMIT` on success / `BAPI_TRANSACTION_ROLLBACK`
  on a BAPIRET2 error.  **Fail-closed**: an empty/unparseable BAPIRET2 is
  treated as *unconfirmed* and never committed; rollback success is verified.
- `sap.rfc.call` gains a `commit=true` flag routing through that helper
  (requires `--enable-writes`).

### Added — audit log (full wiring)

- `AuditLog` / `AuditSink` wired into the server.  Every state-mutating
  call (`sap.rfc.call commit=true` + the three `sap.workflow.*` tools)
  records a **redacted** `AuditEntry` (event id, timestamp, tool, SAP
  system, redacted args, outcome, duration).  Default sink emits JSON on
  the `sap_audit` `tracing` target (stderr — safe for stdio MCP);
  pluggable for Loki / S3 object-lock / Splunk HEC.

### Added — security hardening (from two review passes)

- Validate RFC function + parameter/field names against a safe ABAP
  identifier charset (prevents XML injection that could smuggle a second
  RFC into a SOAP envelope and bypass the read-only gate).
- Char-boundary-safe response-body truncation (no panic on multibyte).
- XML parser recursion-depth cap (256).
- Manual `Debug` for `OdataAuth` / `AdtAuth` so secrets can't leak via `{:?}`.
- Permission warnings on destination / service-key / mTLS-key files.

### Added — docs & ops

- `docs/RUNBOOK_DEV_TENANT.md` — end-to-end dev-tenant onboarding runbook.
- `docs/PRODUCTION_PLAN.md` — readiness assessment + sprint plan (status).
- `deploy/grafana/sap-automate-overview.json` — Grafana dashboard.
- `deploy/sap-automate-destination.example.toml` — destination template.
- `docs/INTEGRATION.md` extended for tenant OData + SOAP RFC + the runbook.

### Tests

- **172 → 206** workspace tests.  New: destination loader, OData auth modes,
  SOAP envelope/codec/parsers/gate, transactional commit/rollback decision,
  ADT ServiceKey/mTLS, and write-path + audit integration tests.  Live
  integration tests (`live_adt`, `live_business_partner_search`,
  `live_read_table_t000`) are secret-gated and skip without a tenant.

## [1.3.0] — 2026-05-25  ·  Live SAP backend tier (Business Hub sandbox)

Adds the second integration testing tier: live OData v4 against the
**SAP Business Accelerator Hub sandbox**.  The first piloted endpoint
is the `API_BUSINESS_PARTNER` v4 service (richest schema, read-stable
across releases).  Additive — no breaking changes.

### Added — OData client

- **`sap_automate_rfc::odata`** module behind feature `odata`.
  - `BusinessHubConfig` — service-specific config; ships with
    `business_partner_sandbox(api_key)`.
  - `BusinessHubClient` — async `reqwest` client with `APIKey` header
    auth, 15 s timeout, OData v4 `$filter` / `$select` / `$top` query
    building, `$filter`-quote escaping per OData §5.1.1.6.1.
  - `BusinessPartner` typed projection of the V4 `A_BusinessPartner`
    entity (id, full name, category, organization name, first/last
    name, grouping, creation date).
  - `BusinessHubClient::from_env()` builds a sandbox client from
    `SAP_BUSINESS_HUB_KEY`; returns `None` when unset so CI without
    secrets skips silently.

### Added — MCP tools

- **`sap.bp.search`** — substring search over `BusinessPartnerFullName`
  using OData v4 `contains()`.  Returns up to 100 rows.
- **`sap.bp.get`** — single-entity fetch by Business Partner id.
- Both tools return a clean "feature disabled" error pointing the
  operator at `SAP_BUSINESS_HUB_KEY` when the env var is unset.  The
  tools are always registered so capability discovery is consistent.

### Added — tests

- **`crates/sap-automate-rfc/src/odata.rs`** — 8 unit tests + 1 live
  integration test (`live_business_partner_search`) that auto-skips
  when `SAP_BUSINESS_HUB_KEY` is unset.
- **`apps/sap-automate-server/tests/business_partner.rs`** — 5
  in-process integration tests covering tool registration, friendly
  "disabled" fallback, and argument validation.

### Added — docs

- **`docs/INTEGRATION.md`** — three-tier integration strategy: CI
  (in-process mocks), Demo (Business Hub sandbox — this release),
  Power-user (ABAP Platform Trial Docker).  Step-by-step on getting
  a free SAP Community API key, wiring `SAP_BUSINESS_HUB_KEY`,
  running the live integration test, rate-limit guidance, and the
  extension pattern for adding `API_MATERIAL` / other services.
- **README** — new "Live SAP backend" row in the MCP spec coverage
  matrix; new "SAP Business Hub" tools row; bumped test count
  159 → 172.

### Changed

- Workspace version: `1.2.0` → `1.3.0` (SemVer minor — additive).
- MCP tool count: **35 → 37**.
- Test count: **159 → 172** passing (+8 odata module +5 BP integration).

### Reference designs studied

- [SAP Business Accelerator Hub](https://api.sap.com/) — sandbox host
  pattern + `APIKey` header convention.
- [SAP S/4HANA OData v4 APIs catalogue](https://api.sap.com/package/SAPS4HANACloud/odata)
  — endpoint URL conventions, `srvd_a2x` package naming.

---

## [1.2.0] — 2026-05-25  ·  MCP spec utilities

Fills in the optional MCP 2025-06-18 utilities required for a
best-in-class protocol implementation — informed by the official
[`modelcontextprotocol`](https://github.com/modelcontextprotocol) spec
and the [`nisalgunawardhana/introduction-to-mcp`](https://github.com/nisalgunawardhana/introduction-to-mcp)
tutorial.  Additive — no breaking changes.

### Added — protocol surface

- **`logging/setLevel`** — clients can crank server log verbosity at
  runtime (RFC 5424 levels: debug → emergency).  Atomic per-server
  level; threadsafe; spec-compliant `{}` response.
- **`notifications/message`** — type model for server-emitted log
  messages keyed by logger name.
- **`completion/complete`** — pluggable per-prompt argument completer
  registry on `ServerBuilder`.  Returns matching candidates,
  spec-capped at 100 entries, with `total` / `hasMore` metadata.
  Three SAP-Automate prompts ship with completers:
  `sap.skill.security_sod_audit` (scope ∈ user/role/system),
  `sap.skill.abap_code_review` (kind ∈ class/program/interface/function_module),
  `sap.skill.bw_to_datasphere_migration` (target_release dropdown).
- **`notifications/progress`** + **`notifications/cancelled`** — type
  model with `ProgressToken` (string or number) and `ProgressParams`
  (monotonic-increase invariant documented).  Tool-side emission +
  cooperative cancellation land in a follow-up; the wire shape is in
  place so clients can rely on it.
- **`ServerCapabilities`** now advertises `logging` and `completions`
  when those utilities are wired — clients negotiate against the real
  feature set.

### Added — transport security (MCP 2025-06-18 §4.6)

- **HTTP `Origin` validation** — new `allowed_origins` field on
  `HttpServerConfig`, exposed as `--allowed-origin <url>` (repeatable)
  on the server binary.  When set, requests whose `Origin` header is
  absent or not in the allowlist return HTTP 403.  DNS-rebinding
  mitigation per spec.  Applies to both `POST /mcp` and `GET /mcp/events`
  (SSE).

### Added — client surface

- **`Client::set_log_level(level)`** — typed helper for `logging/setLevel`.
- **`Client::complete_prompt_argument(prompt, arg, typed)`** — typed
  helper for `completion/complete`.
- **`Client::raw_request<R>(method, params)`** — forwards-compat
  escape hatch for spec methods not yet wrapped by a typed helper.

### Tests

- **+14** passing — 159 total (was 145).
- **`spec_utilities.rs`** integration tests: `logging/setLevel`
  acceptance + enum validation, `completion/complete` returns
  registered values, filters by prefix, returns `[]` for unknown
  refs, and `initialize` advertises `logging` + `completions`
  capabilities.
- **HTTP transport unit tests** for `check_origin` (5) and
  `check_auth` (3).

### Reference designs studied

- [`modelcontextprotocol`](https://github.com/modelcontextprotocol) — the
  official spec org; SDKs across 10+ languages.
- [`nisalgunawardhana/introduction-to-mcp`](https://github.com/nisalgunawardhana/introduction-to-mcp)
  — 13-module tutorial covering server / client / best-practices / debugging.

---

## [1.1.0] — 2026-05-25  ·  Convergence pass

Three Karpathy-style passes layered on top of v1.0 — each additive, none
breaking — after surveying
[`multica-ai/andrej-karpathy-skills`](https://github.com/multica-ai/andrej-karpathy-skills),
[`VectifyAI/OpenKB`](https://github.com/VectifyAI/OpenKB) +
[`VectifyAI/PageIndex`](https://github.com/VectifyAI/PageIndex),
[`unclecode/crawl4ai`](https://github.com/unclecode/crawl4ai), and
re-reading the six reference SAP MCP servers tracked in
`docs/COMPARISON.md`.  Discipline: "Simplicity First / Surgical Changes"
— no rewrites of existing surfaces.

### Headlines

- **Skills**: **8 → 13** auto-discovered (Karpathy guidelines, AIPNV
  anti-autopilot, OData design, SoD audit, BW-to-Datasphere).
- **MCP tools**: **32 → 35**
  (`sap.system.cache_stats`, `sap.system.cache_invalidate`, `sap.kb.navigate`).
- **MCP resources**: **11 → 12** (`sap-cache://stats`).
- **Tests**: **104 → 145** passing (no flake-prone, all ≤ 0.1 s except the
  ADT HTTP integration suite).
- **No breaking API changes.**  Every addition is a new field on an
  existing type, a new module, or a new trait default-impl.

### KB + RAG pass (2026-05-25 — same release window)

Third pass: extends the knowledge / retrieval layer with the convergent
patterns from [`VectifyAI/OpenKB`](https://github.com/VectifyAI/OpenKB) +
[`VectifyAI/PageIndex`](https://github.com/VectifyAI/PageIndex) (hierarchical
document tree) and [`unclecode/crawl4ai`](https://github.com/unclecode/crawl4ai)
(robots.txt, rate-limit, "fit markdown" boilerplate filter), plus
retrieval transparency that operators have been asking for.

#### Knowledge base (`crates/sap-automate-kb`)

- **`doc_tree::DocumentTree`** — deterministic hierarchical tree built
  from a document's headings (Markdown ATX `#`/`##`/`###`, numbered
  sections like `1.2.3.`, or `SECTION:` keyword markers). Each node
  carries title, extractive 2-sentence summary, byte range, approx token
  count, and children. The OpenKB + PageIndex *data structure* without
  the LLM-at-build-time dependency.
- **`KnowledgeStore::get_document_tree(id)`** — default-impl trait
  method using the new builder. Production backends can override to
  cache the tree alongside the document.
- **Content-hash dedup** at chunk upsert: writing the same `(chunk_id, text)`
  twice is a no-op, surfaced via `UpsertStats::chunks_dedup_skipped`.
  Pre-empts a real foot-gun where a re-crawl with unchanged content was
  rewriting the same rows.

#### RAG (`crates/sap-automate-rag`)

- **`RetrievalDiagnostics`** field on `SearchResponse`: dense / sparse
  candidate counts, RRF overlap (consensus signal), tokenised query
  terms (so the operator sees *what* BM25 actually searched for),
  reranker-ran flag, truncated-by-top-k flag. Pure additive; ordering
  unchanged.
- `RagEngine::store()` accessor so tools can reach the underlying
  `KnowledgeStore` without re-plumbing.

#### Server (`apps/sap-automate-server`)

- **`sap.kb.navigate`** MCP tool — walks the document tree by dotted
  path (`"1.2.1"`) with a bounded `depth`. Convergent OpenKB +
  PageIndex pattern: for long SAP Help pages and ABAP source files,
  section-by-section navigation beats similarity-blind retrieval.
- 4 in-process binary integration tests under
  `tests/kb_navigate.rs` covering registration, root walk, dotted-path
  navigation, and missing-doc error path.

#### Crawler (`crates/sap-automate-ingest`)

- **`robots::RobotsTxt`** — RFC 9309-subset parser with
  most-specific-agent matching, longest-prefix Allow/Disallow,
  `Crawl-delay:` extraction. 7 unit tests.
- **`rate_limit::RateLimiter`** — per-host token-bucket spacing,
  default plus per-host overrides from `Crawl-delay:`. 5 unit tests.
- **`fit_markdown::fit_markdown_filter`** — Crawl4AI's BM25-based
  block-level content filter. Scores paragraphs against a topic
  (typically the page title), drops nav/footer/cookie-banner
  boilerplate while always keeping long blocks. Returns `FitStats`
  (retention ratios). 4 unit tests.

### Apps-layer pass (2026-05-25 — same release window)

Closes the loop on the metadata-cache work above by wiring it through
every app surface, verifying it end-to-end with binary integration
tests, and exposing it to operators (TUI + web).

#### Server (`apps/sap-automate-server`)

- **Wires `MetadataCache`** as a decorator over `MockSapClient` (also
  ready for any future `NetweaverSapClient`). New CLI flag
  `--metadata-cache-ttl-secs` (default `300`; `0` makes the cache a
  pass-through counter so operators still get hit/miss visibility).
- **`sap.system.cache_stats`** MCP tool — read-only, returns
  `{ enabled, hits, misses, entries, evictions, hit_ratio }`.
  Convergent with `thupalo/sap-rfc-mcp-server`'s
  `get_metadata_cache_stats`.
- **`sap.system.cache_invalidate`** MCP tool — operator escape hatch
  for the case where an upstream transport import changed an RFC
  signature and cached metadata is stale. Mutates only local state,
  never SAP.
- **`sap-cache://stats`** MCP resource — same JSON, surfaced through
  `resources/read`.
- **3 binary integration tests** (`apps/sap-automate-server/tests/cache_tools.rs`)
  spawn the compiled server, list tools/resources, call
  `sap.rfc.metadata` twice, and verify the hit counter moves —
  Karpathy goal-driven verify loop.

#### TUI (`apps/sap-automate-tui`)

- New `TrafficEvent::CacheStat` variant + `CacheSnapshot` in the
  state machine.
- **Cache row** at the bottom of the KB tab (hits / misses /
  entries / hit_ratio) with the same green/yellow/red threshold
  styling as the other gauges.
- Synthetic feed emits a cache snapshot every 23 ticks so the row is
  exercised offline.

#### Gateway (`apps/sap-automate-gw`)

- **Skill-aware routing** — `match_skill()` maps user-intent keywords
  to `sap.skill.*` prompts and invokes them via `prompts/get` before
  falling back to raw tool calls. Honours the convergent
  `marianfoo/sap-ai-mcp-servers` insight that *agents should invoke
  skills, not raw tools*. Eight intents routed: SoD audit, BW
  migration, period close, ABAP code review, OData design, transport
  impact, Clean Core audit, Karpathy guidelines pre-flight.

#### Web (`apps/web`)

- **Cache panel on the Operations page** — polls
  `sap.system.cache_stats` every 2 s, renders hits / misses /
  entries / evictions in stat tiles + a hit-ratio progress bar
  (green ≥80%, yellow ≥50%, red <50%).
- **Skill Lab "Why this matters"** updated to credit the Karpathy
  convergence alongside `mdk-mcp-server` / `fr0ster/mcp-abap-adt` /
  `marianfoo/sap-ai-mcp-servers`.

### Added

- **`skills/karpathy-guidelines.md`** — port of Multica's
  `karpathy-guidelines` SKILL (MIT, attributed) adapted with SAP-specific
  examples. Loaded by `SkillRegistry` as the
  `sap.skill.karpathy_guidelines` MCP prompt.
- **`skills/aipnv-ai-pairing.md`** — AIPNV anti-autopilot five-question
  checklist that surfaces the `fr0ster/mcp-abap-adt` stance as an
  invokable pre-flight skill.
- **`skills/odata-service-design.md`** — generic OData-proxy design
  discipline (metadata-first → tool-surface mapping → EDM-to-JSON-Schema
  conversion → auth binding → exposure policy → verification gates).
  Convergent pattern from `marianfoo/sap-ai-mcp-servers`.
- **`skills/security-sod-audit.md`** — read-only Segregation-of-Duties
  audit walking `USR02` / `AGR_USERS` / `AGR_1251` / `AGR_TCODES` /
  `RFCDES`; bundled SoD rule library for FI/MM/SD/basis conflict pairs.
- **`skills/bw-to-datasphere-migration.md`** — BW modernisation
  classification matrix + custom-code surfacing + 3-wave plan + risk
  register.
- **`sap-automate-rfc::MetadataCache`** — TTL-keyed decorator over any
  `SapClient`. Implements the `thupalo/sap-rfc-mcp-server` pattern:
  caches `RfcFunctionMeta` by `(function, language)`, splits bulk reads
  into hits + misses, exposes `CacheStats` for Prometheus, supports
  `invalidate_all()` for system-role flips.  `tokio::sync::RwLock`-based,
  no extra dependencies.  6 unit tests cover hit/miss, TTL=0 disable,
  TTL expiry, bulk-split, invalidation, and `(function, language)`
  keying.
- **Behavioural-guidelines section in `AGENTS.md`** — restates the four
  Karpathy principles as pre-flight rules; cross-links the new skills.

### Changed

- Skill count: **8 → 13** auto-discovered skills.
- MCP tool count: **32 → 35** (cache_stats, cache_invalidate, kb.navigate).
- MCP resource count: **11 → 12** (`sap-cache://stats`).
- MCP prompts surfaced via `prompts/list`: **11 → 16**.
- Test count: **104 → 145** passing tests (+6 metadata_cache +3 cache-tools +6 doc_tree +3 store-dedup/tree +2 RAG-diagnostics +7 robots +5 rate-limit +4 fit-markdown +4 kb_navigate +1 misc).
- `README.md` — refreshed credits, added skill table, repository-layout
  blurb; added `MetadataCache (TTL)` mention in `sap-automate-rfc`
  description; bumped tool / resource counts; credited OpenKB+PageIndex
  and Crawl4AI as the references for the KB+RAG+crawler pass.

### Notes

- Nothing in this release is breaking. Public API of `sap-automate-rfc`
  gains a `metadata_cache` module and re-exports `MetadataCache` +
  `CacheStats`; the trait signature of `SapClient` is unchanged.
- No new external dependencies.  The cache uses `tokio::sync::RwLock`,
  `std::time::Instant`, and the existing `async-trait` already in
  workspace.
- The 5 new skills carry valid YAML-style frontmatter and round-trip
  through `parse_skill_file()`; tests in `sap-automate-skills` validate
  the loader unchanged.

---

## [1.0.0] — 2026-05-25  ·  First public release

The first general-availability release of **SAP-Automate** — a
Rust-native, MCP-native agentic interface for SAP S/4HANA built by
the **Gaussian Technologies TPO R&D team**.

### Highlights

- **32 MCP tools** across 5 SAP domains (RAG search, RFC + tables, ABAP
  ADT, knowledge graph, guided workflows) with full schema-driven
  forms, structured-enum parameters, and read-only-by-default safety.
- **104 tests passing** — including 7 SAP-precision tests that enforce
  DDIC / BAPI invariants in CI, 17 ADT integration tests against an
  axum mock SAP server, and a P95 acceptance benchmark.
- **Sub-millisecond retrieval**: hybrid RAG P95 = **0.16 ms** (500×
  under paper §X-D's 80 ms gate); HippoRAG multi-hop P95 = **0.08 ms**
  (5000× under §X-H's 400 ms gate).
- **MCP 2025-06-18** wire-format compliance, including live
  **structured elicitation** for guided workflows.
- **Production deployment artefacts**: multi-stage distroless
  Dockerfile, hardened K8s manifests (Deployment, Service, HPA,
  NetworkPolicy, PodDisruptionBudget), Kustomize entry point,
  operator runbook.
- **Observability**: Prometheus `/metrics` endpoint, audit log with
  PII / secret redaction, OpenTelemetry-ready tracing.

### Added

#### Protocol & framework

- `mcp-core`: JSON-RPC 2.0 codec + full MCP 2025-06-18 protocol types
  (initialize, tools, resources, prompts, elicitation).
- `mcp-transport`: `Transport` trait + stdio + HTTP/SSE transport
  (under `http` feature).  Stdio supports independent read/write
  splits for cancellation-safe elicitation under load.
- `mcp-server`: builder API, capability router, `ExposurePolicy` for
  read-only / write-enabled tool filtering, `ElicitationHandle` +
  `tokio::task_local!` `TOOL_CONTEXT` for mid-tool elicitation.
- `mcp-client`: async client with request/response correlation,
  `ElicitationDelegate` trait (decline / accept / stdin / seed
  delegates ship in `sample-client`).

#### SAP integration

- `sap-automate-rfc`: `SapClient` async trait + `MockSapClient` with
  realistic FI / MM / SD fixtures.  Connection pool, circuit breaker,
  retry-with-backoff, layered credential provider.  Structured
  `RfcError` taxonomy mapped to MCP JSON-RPC codes.  `BAPIRET2`
  parser for SAP-standard return contracts.
- `sap-automate-adt`: `AdtClient` trait + `MockAdtClient` (offline) +
  `HttpAdtClient` (under `http` feature) with CSRF cache, X-SAP-Client
  capitalisation, real ADT URL canon, full data-preview XML parser.
  Destination model + 5 auth schemes.

#### Knowledge base + retrieval

- `sap-automate-kb`: `KnowledgeStore` trait, in-memory + Qdrant
  backends, document / chunk schema per paper §VI.
- `sap-automate-rag`: hybrid retrieval (dense + BM25 + RRF + cross-
  encoder reranker), contextual chunk enrichment, latency breakdown
  per layer.
- `sap-automate-graph`: typed cross-domain knowledge graph, Louvain
  community detection, Personalised PageRank (HippoRAG), 3-level
  RAPTOR hierarchical clusters.
- `sap-automate-ingest`: HTML crawler, sentence-boundary chunker,
  `EmbeddingClient` trait (`MockEmbedder` + `OpenAiEmbedder`),
  ingestion pipeline.

#### Agentic layer

- `sap-automate-skills`: AGENTS.md-style skill loader.  8 starter
  skills auto-loaded as MCP prompts.
- `sap-automate-memory`: 4-tier memory (working ring buffer,
  episodic tag/tenant index, semantic via RAG, procedural via skills).
- `sap-automate-scheduler`: TOML-declared proactive jobs with 5
  cadence kinds (every-N / hourly / daily / weekly / quarterly).
- `sap-automate-channels`: `ChannelAdapter` trait, working `CliChannel`,
  Teams / Slack / Telegram skeletons, `ChannelRegistry`.

#### Production

- `sap-automate-observability`: Prometheus metrics registry, audit
  log with secret redaction, tracing init scaffolding.
- Multi-stage Dockerfile (distroless runtime, nonroot UID, ≈ 20 MB).
- 9 K8s manifests: Deployment, Service, HPA, NetworkPolicy,
  PodDisruptionBudget, ConfigMap, Secret template, Kustomize,
  Namespace.
- GitHub Actions: CI (fmt, clippy, stable+beta test matrix, SAP
  precision gate, P95 bench gate, cargo-audit, Docker build, K8s
  manifest lint, Next.js web build), release pipeline (Linux x86_64
  + aarch64 binaries via `cross`, multi-arch container push to GHCR).

#### Applications

- `apps/sap-automate-server`: the main MCP server (stdio + HTTP).
- `apps/sap-automate-gw`: multi-channel agentic gateway with intent
  routing + 4-tier memory + scheduler integration.
- `apps/sap-automate-tui`: 5-tab Ratatui operator console.
- `apps/sap-automate-ingest`: knowledge ingestion CLI.
- `apps/sap-automate-bench`: P95 acceptance harness.
- `apps/web`: Next.js 14 web UI — Operations, Query Lab, Graph Lab,
  Tool Explorer, Skill Lab, Resources.
- `apps/sample-server`, `apps/sample-client`: minimal pair for smoke
  testing and framework demos.

### Documentation

- `docs/SAPAutomate.pdf` — full architectural whitepaper.
- `docs/ROADMAP.md` — phased delivery plan, all phases ✅.
- `docs/SAP_CORRECTNESS.md` — every fixture mapped to its SAP source.
- `docs/COMPARISON.md` — analysis vs 6 reference SAP MCP servers.
- `deploy/k8s/README.md` — production deployment runbook.
- `AGENTS.md` — default agent guardrails.

### Fixed (during v1.0 review pass)

- `RfcError::Internal` and `AdtError::Internal` were misclassified as
  transient — they now map to dedicated `Internal` codes (`-32299` /
  `-32298`) so retry logic does not spin on programming bugs.
- `sap.table.read` now auto-applies a MANDT / RCLNT client filter
  when the caller doesn't specify one — matches SE16 / SM30 and the
  standard `RFC_READ_TABLE` convention, eliminates cross-client
  leakage by construction.
- `parse_nodestructure` rewritten to handle the child-element XML
  shape that real SAP `repository/nodestructure` responses use (the
  old attribute-form-only parser would have returned empty results
  against any production SAP system).
- `parse_data_preview` rewritten — was always returning `Vec::new()`.
  Now extracts `<dataPreview:row>/<dataPreview:cell>` data, supporting
  both `adtcore:value` attribute and inline-text cell variants.
- ADT URL pattern for package contents corrected from
  `GET /sap/bc/adt/repository/nodestructure?...` to
  `POST /sap/bc/adt/repository/nodestructure` with form body.
- `X-SAP-Client` HTTP header capitalisation aligned with the SAP ADT
  spec (some older NW gateways are case-sensitive).
- Single-actor `select!` dispatch loop replaced with split reader /
  writer tasks on both server and client — cancellation-safe under
  any concurrent load (proven by load testing in P6).

### Migration notes (for adopters tracking pre-1.0 commits)

- Public error enums (`RfcError`, `AdtError`, `RfcErrorCode`,
  `AdtErrorCode`) are now `#[non_exhaustive]`.  Update any exhaustive
  matches to add a wildcard arm.
- `Server::run` over a generic `Transport` no longer supports
  elicitation; stdio callers must use `Server::run_stdio(reader,
  writer)` (the existing `into_parts()` split).
- `Client::spawn_with_delegate` is retained but `Client::spawn_stdio`
  is recommended — the split-half client is the only one safe for
  workflows that involve server-initiated requests.

---

## Reference

- Architecture whitepaper: *SAP-Automate: An MCP-Native RAG Architecture for SAP S/4HANA*, Gaussian Technologies Technical Review Vol. 1 No. 1 (2026).  Reference design code `GT-TR-2026-SAP-AUTOMATE-01`.
- MCP specification: <https://modelcontextprotocol.io/specification/2025-06-18>.
