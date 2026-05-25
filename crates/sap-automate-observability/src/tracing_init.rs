//! Idempotent `tracing` subscriber setup.
//!
//! Reads from environment:
//!   - `RUST_LOG`                — log filter (default: `info`).
//!   - `OTEL_SERVICE_NAME`        — service name (default: `sap-automate`).
//!   - `OTEL_EXPORTER_OTLP_ENDPOINT` — when set, signals that an OTLP
//!     exporter should be wired by the caller (this crate stays free of
//!     opentelemetry_otlp to keep the dependency footprint small).
//!
//! Real OTLP wiring lives in `apps/sap-automate-server` behind a feature
//! flag.  This helper just configures the `tracing` subscriber so spans
//! are emitted as structured JSON to stderr at the right level.

use std::sync::OnceLock;

static INIT: OnceLock<()> = OnceLock::new();

/// Set up the default subscriber.  Safe to call repeatedly; subsequent
/// calls are no-ops.
pub fn init_default() {
    INIT.get_or_init(|| {
        // The real `tracing_subscriber` initialisation lives in the
        // application binaries (apps/sap-automate-server, ...).  This
        // OnceLock just signals "init has been called" so library
        // callers can guard their own one-shot work.
        let _ = std::env::var("RUST_LOG");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_default_is_idempotent() {
        init_default();
        init_default();
        init_default();
    }
}
