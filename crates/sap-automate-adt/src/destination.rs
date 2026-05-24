//! Destination model.
//!
//! Inspired by fr0ster/mcp-abap-adt's destination-first auth model:
//! a destination is a named bundle of (base URL, client, auth method,
//! credentials).  Destinations live as TOML files under
//! `~/.config/sap-automate/destinations/<name>.toml`, mirroring SAP BTP
//! Destination service semantics.
//!
//! This module ships the type + a synchronous in-memory builder.  Loading
//! from disk lives in the `http` feature (it depends on `toml`).

use serde::{Deserialize, Serialize};

/// One destination = one SAP system endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdtDestination {
    pub name: String,
    /// e.g. `https://s4hana.example.com:44300`
    pub base_url: String,
    /// SAP client number, e.g. `100`.
    pub client: String,
    /// Default ADT language, e.g. `EN`.
    #[serde(default = "default_language")]
    pub language: String,
    pub auth: AdtAuth,
}

fn default_language() -> String { "EN".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdtAuth {
    /// HTTP Basic auth.  Used by both `mario-andreschak/mcp-abap-adt` and
    /// the fallback path in `fr0ster/mcp-abap-adt`.
    Basic { user: String, password: String },
    /// Bearer token (JWT, XSUAA, etc.).
    Bearer { token: String },
    /// SAP BTP service key file — for environments using XSUAA.  The path
    /// is loaded lazily by the HTTP client.
    ServiceKey { path: String },
    /// Mutual TLS via PEM files (on-premise only per fr0ster's note).
    Certificate { cert_path: String, key_path: String },
    /// Mock destination — no network at all.
    Mock,
}

impl AdtDestination {
    pub fn mock(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: "https://mock.sap.example".into(),
            client: "100".into(),
            language: "EN".into(),
            auth: AdtAuth::Mock,
        }
    }

    /// Redacted form for logs / `agents://destinations` resource.
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "base_url": self.base_url,
            "client": self.client,
            "language": self.language,
            "auth_type": auth_type_label(&self.auth),
        })
    }
}

fn auth_type_label(a: &AdtAuth) -> &'static str {
    match a {
        AdtAuth::Basic { .. } => "basic",
        AdtAuth::Bearer { .. } => "bearer",
        AdtAuth::ServiceKey { .. } => "service_key",
        AdtAuth::Certificate { .. } => "certificate",
        AdtAuth::Mock => "mock",
    }
}
