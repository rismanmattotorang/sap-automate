//! Sprint 1 — live ADT smoke test against a real SAP development system.
//!
//! Gated on `SAP_AUTOMATE_DESTINATION`: with no destination configured the
//! test prints a skip notice and passes, so CI (and any contributor without
//! SAP access) stays green.  This mirrors the gated SAP Business Hub sandbox
//! test in `crates/sap-automate-rfc/src/odata.rs`.
//!
//! ## Running it against a dev S/4HANA system
//!
//! 1. Create `./.sap-automate/destinations/<name>.toml` — see
//!    `deploy/sap-automate-destination.example.toml` for the schema.
//! 2. Run:
//!
//! ```bash
//! SAP_AUTOMATE_DESTINATION=<name> \
//!   cargo test -p sap-automate-server --test live_adt -- --nocapture
//! ```
//!
//! Override the probed class with `SAP_AUTOMATE_TEST_CLASS` if the default
//! is unavailable on your stack.

use sap_automate_adt::{AdtAuth, AdtClient, AdtDestination, HttpAdtClient};

#[tokio::test]
async fn live_adt_get_class_smoke() {
    let Ok(name) = std::env::var("SAP_AUTOMATE_DESTINATION") else {
        eprintln!(
            "SKIP live_adt_get_class_smoke: set SAP_AUTOMATE_DESTINATION=<name> \
             (with a destination TOML on the search path) to exercise a real SAP system"
        );
        return;
    };
    if name.is_empty() {
        eprintln!("SKIP live_adt_get_class_smoke: SAP_AUTOMATE_DESTINATION is empty");
        return;
    }

    let dest = AdtDestination::load(&name)
        .unwrap_or_else(|e| panic!("destination '{name}' load failed: {e}"));
    assert!(
        !matches!(dest.auth, AdtAuth::Mock),
        "live test needs a non-mock destination; '{name}' declares auth=mock"
    );

    let client = HttpAdtClient::new(dest).expect("HttpAdtClient init");

    // A class that exists on essentially every ABAP stack.
    let class = std::env::var("SAP_AUTOMATE_TEST_CLASS")
        .unwrap_or_else(|_| "CL_ABAP_CHAR_UTILITIES".to_string());

    let src = client
        .get_class(&class)
        .await
        .unwrap_or_else(|e| panic!("get_class({class}) against '{name}' failed: {e}"));

    assert!(
        !src.source.is_empty(),
        "expected non-empty ABAP source for {class}"
    );
    eprintln!(
        "live_adt OK: fetched {} from destination '{}' ({} bytes of source)",
        class,
        name,
        src.source.len()
    );
}
