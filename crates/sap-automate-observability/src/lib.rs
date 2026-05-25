//! Observability primitives for SAP-Automate (P10).
//!
//! Three concerns kept in one crate so wiring stays trivial:
//!
//!   - **`metrics`**: Prometheus-compatible counter + histogram registry.
//!     Named per paper §IV-H: `mcp_tool_latency_seconds`,
//!     `rag_retrieval_latency_seconds`, `kb_index_freshness_seconds`,
//!     `sap_rfc_calls_total`, `sap_authz_denied_total`.
//!
//!   - **`audit`**: Append-only audit log for state-mutating tool
//!     invocations.  Required for SOX evidence trails over FI postings,
//!     transport releases, customer-master changes.  Pluggable sink so
//!     production wires to a tamper-evident store (Loki / S3 with object
//!     lock / Splunk HEC).
//!
//!   - **`tracing_init`**: idempotent OpenTelemetry / `tracing`
//!     subscriber setup driven by env vars (RUST_LOG, OTEL_EXPORTER_*,
//!     OTEL_SERVICE_NAME).  When the `OTEL_EXPORTER_OTLP_ENDPOINT` env
//!     var is set, an OTLP exporter is configured; otherwise it falls
//!     back to a JSON stderr writer.  The crate keeps the actual OTLP
//!     wiring behind a feature flag (real OTLP needs `opentelemetry_otlp`
//!     and that pulls a lot of weight; production deployments enable it
//!     via the `otlp` feature).

pub mod audit;
pub mod metrics;
pub mod tracing_init;

pub use audit::{AuditEntry, AuditLog, AuditOutcome, AuditSink, JsonStderrSink, JsonStdoutSink};
pub use metrics::{HistogramBucketSet, Metric, MetricKind, MetricsRegistry, Snapshot};
pub use tracing_init::init_default;
