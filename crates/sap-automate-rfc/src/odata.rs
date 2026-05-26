//! SAP S/4HANA OData v4 client — Business Accelerator Hub sandbox.
//!
//! This module is the "Live SAP backend" tier in SAP-Automate's three-tier
//! integration testing strategy:
//!
//!   1. **CI tier** — in-process axum mocks (already shipped for ADT).
//!   2. **Demo tier** — *this module* — hits the SAP Business Accelerator
//!      Hub sandbox at `sandbox.api.sap.com` with the operator's
//!      `APIKey` header.  No registration friction: a free SAP Community
//!      login gets a key.  Real OData semantics; rate-limited; public.
//!   3. **Power-user tier** — `sapse/abap-platform-trial` Docker
//!      (documented in `docs/INTEGRATION.md`).
//!
//! The `API_BUSINESS_PARTNER` v4 service is the pilot endpoint — it has
//! the richest schema (Organization + Person dual-role, addresses, roles,
//! tax data) and is read-stable across releases.  Other services slot in
//! behind the same `BusinessHubClient` shape.
//!
//! **Auth**: `APIKey: <key>` header.  Never log or persist the key.  The
//! integration tests skip cleanly when `SAP_BUSINESS_HUB_KEY` is unset
//! so CI without secrets is unaffected.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum OdataError {
    #[error("http: {0}")]
    Http(String),
    #[error("auth: API key missing or empty")]
    MissingApiKey,
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed OData response: {0}")]
    Parse(String),
}

pub type OdataResult<T> = Result<T, OdataError>;

/// Connection config.  `base_url` is the OData v4 service root; `api_key`
/// is the operator's SAP Business Accelerator Hub key.
#[derive(Debug, Clone)]
pub struct BusinessHubConfig {
    pub base_url: String,
    pub api_key: String,
    pub timeout: Duration,
}

impl BusinessHubConfig {
    /// Defaults for the public sandbox `API_BUSINESS_PARTNER` v4 service.
    /// Override `base_url` for on-prem destinations.
    pub fn business_partner_sandbox(api_key: impl Into<String>) -> Self {
        Self {
            base_url: "https://sandbox.api.sap.com/s4hanacloud/sap/opu/odata4/sap/api_business_partner/srvd_a2x/sap/businesspartner/0001".into(),
            api_key: api_key.into(),
            timeout: Duration::from_secs(15),
        }
    }
}

/// One row from the V4 `A_BusinessPartner` entity set.  Subset projection —
/// the full schema has ~80 fields; we ship the ones SAP-Automate's MCP
/// tool surface actually returns.  Add fields here as new tools land.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessPartner {
    #[serde(rename = "BusinessPartner")]
    pub id: String,
    #[serde(rename = "BusinessPartnerFullName", default)]
    pub full_name: Option<String>,
    /// `1` = Person, `2` = Organization, `3` = Group.  S/4HANA convention.
    #[serde(rename = "BusinessPartnerCategory", default)]
    pub category: Option<String>,
    #[serde(rename = "OrganizationBPName1", default)]
    pub organization_name: Option<String>,
    #[serde(rename = "FirstName", default)]
    pub first_name: Option<String>,
    #[serde(rename = "LastName", default)]
    pub last_name: Option<String>,
    #[serde(rename = "BusinessPartnerGrouping", default)]
    pub grouping: Option<String>,
    #[serde(rename = "CreationDate", default)]
    pub creation_date: Option<String>,
}

/// OData v4 collection wire shape: `{"@odata.context": "...", "value": [...]}`.
#[derive(Debug, Deserialize)]
struct ODataCollection<T> {
    value: Vec<T>,
}

#[derive(Debug)]
pub struct BusinessHubClient {
    http: reqwest::Client,
    config: BusinessHubConfig,
}

impl BusinessHubClient {
    pub fn new(config: BusinessHubConfig) -> OdataResult<Self> {
        if config.api_key.trim().is_empty() {
            return Err(OdataError::MissingApiKey);
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!("sap-automate/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| OdataError::Http(e.to_string()))?;
        Ok(Self { http, config })
    }

    /// Build a sandbox client from a supplied key.  Returns `None` when
    /// the key is empty / whitespace.  Caller decides whether to surface
    /// the missing-key state as an error or as a "feature disabled" hint.
    pub fn from_key(key: &str) -> Option<OdataResult<Self>> {
        if key.trim().is_empty() {
            return None;
        }
        Some(Self::new(BusinessHubConfig::business_partner_sandbox(key)))
    }

    /// Read `SAP_BUSINESS_HUB_KEY` from the process env and build a
    /// sandbox client.  Returns `None` when the env var is unset or
    /// empty — letting CI runs without secrets skip silently.
    pub fn from_env() -> Option<OdataResult<Self>> {
        let key = std::env::var("SAP_BUSINESS_HUB_KEY").ok()?;
        Self::from_key(&key)
    }

    /// `$filter=contains(BusinessPartnerFullName,'<q>')` plus a tight
    /// `$select` to keep the row size predictable.
    pub async fn search_business_partners(&self, query: &str, top: usize) -> OdataResult<Vec<BusinessPartner>> {
        let url = format!(
            "{}/A_BusinessPartner",
            self.config.base_url.trim_end_matches('/'),
        );
        // Escape single quotes per OData v4 §5.1.1.6.1 (doubled).
        let escaped = query.replace('\'', "''");
        let filter = format!("contains(BusinessPartnerFullName,'{}')", escaped);
        let select = "BusinessPartner,BusinessPartnerFullName,BusinessPartnerCategory,OrganizationBPName1,FirstName,LastName,BusinessPartnerGrouping,CreationDate";
        debug!(url = %url, top, "GET A_BusinessPartner (filtered)");

        let response = self.http.get(&url)
            .header("APIKey", &self.config.api_key)
            .header("Accept", "application/json")
            .query(&[
                ("$top", top.to_string()),
                ("$filter", filter),
                ("$select", select.to_string()),
            ])
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;

        Self::deserialize_collection(response).await
    }

    /// `$top=N` only — useful for "show me anything" smoke tests.
    pub async fn list_business_partners(&self, top: usize) -> OdataResult<Vec<BusinessPartner>> {
        let url = format!(
            "{}/A_BusinessPartner",
            self.config.base_url.trim_end_matches('/'),
        );
        let select = "BusinessPartner,BusinessPartnerFullName,BusinessPartnerCategory,OrganizationBPName1,FirstName,LastName,BusinessPartnerGrouping,CreationDate";
        let response = self.http.get(&url)
            .header("APIKey", &self.config.api_key)
            .header("Accept", "application/json")
            .query(&[
                ("$top", top.to_string()),
                ("$select", select.to_string()),
            ])
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;
        Self::deserialize_collection(response).await
    }

    /// Single-entity fetch — `A_BusinessPartner('<id>')`.
    pub async fn get_business_partner(&self, id: &str) -> OdataResult<BusinessPartner> {
        let escaped = id.replace('\'', "''");
        let url = format!(
            "{}/A_BusinessPartner('{}')",
            self.config.base_url.trim_end_matches('/'),
            escaped,
        );
        let response = self.http.get(&url)
            .header("APIKey", &self.config.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;
        let status = response.status();
        let body = response.text().await.map_err(|e| OdataError::Http(e.to_string()))?;
        if !status.is_success() {
            warn!(status = status.as_u16(), "BP get failed");
            return Err(OdataError::Status { status: status.as_u16(), body: truncate_body(&body) });
        }
        serde_json::from_str(&body)
            .map_err(|e| OdataError::Parse(format!("{e}; body prefix: {}", truncate_body(&body))))
    }

    async fn deserialize_collection(response: reqwest::Response) -> OdataResult<Vec<BusinessPartner>> {
        let status = response.status();
        let body = response.text().await.map_err(|e| OdataError::Http(e.to_string()))?;
        if !status.is_success() {
            warn!(status = status.as_u16(), "BP search failed");
            return Err(OdataError::Status { status: status.as_u16(), body: truncate_body(&body) });
        }
        let collection: ODataCollection<BusinessPartner> = serde_json::from_str(&body)
            .map_err(|e| OdataError::Parse(format!("{e}; body prefix: {}", truncate_body(&body))))?;
        Ok(collection.value)
    }

    pub fn config(&self) -> &BusinessHubConfig {
        &self.config
    }
}

fn truncate_body(s: &str) -> String {
    let limit = 300;
    if s.len() <= limit {
        s.to_string()
    } else {
        format!("{}…[+{} more chars]", &s[..limit], s.len() - limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_hub_config_carries_canonical_v4_url() {
        let c = BusinessHubConfig::business_partner_sandbox("test-key");
        assert!(c.base_url.contains("/sap/opu/odata4/"), "expected v4 URL pattern; got {}", c.base_url);
        assert!(c.base_url.contains("api_business_partner"));
        assert!(c.base_url.contains("/0001"), "expected service version 0001");
    }

    #[test]
    fn new_rejects_empty_api_key() {
        let cfg = BusinessHubConfig {
            base_url: "https://example.com/odata4".into(),
            api_key: "  ".into(),
            timeout: Duration::from_secs(1),
        };
        assert!(matches!(BusinessHubClient::new(cfg).unwrap_err(), OdataError::MissingApiKey));
    }

    #[test]
    fn new_accepts_real_key() {
        let cfg = BusinessHubConfig {
            base_url: "https://example.com/odata4".into(),
            api_key: "any-non-empty-string".into(),
            timeout: Duration::from_secs(1),
        };
        assert!(BusinessHubClient::new(cfg).is_ok());
    }

    #[test]
    fn from_key_returns_none_for_empty() {
        assert!(BusinessHubClient::from_key("").is_none());
        assert!(BusinessHubClient::from_key("   ").is_none());
    }

    #[test]
    fn from_key_returns_some_for_real_key() {
        let r = BusinessHubClient::from_key("a-real-looking-key");
        assert!(r.is_some());
        assert!(r.unwrap().is_ok());
    }

    #[test]
    fn truncate_body_keeps_short_strings() {
        assert_eq!(truncate_body("short"), "short");
    }

    #[test]
    fn truncate_body_clips_long_strings() {
        let long = "x".repeat(500);
        let out = truncate_body(&long);
        assert!(out.contains("[+200 more chars]"));
        assert!(out.len() < long.len());
    }

    /// Integration test against the live SAP Business Accelerator Hub
    /// sandbox.  Skips with a printed message when `SAP_BUSINESS_HUB_KEY`
    /// is unset — CI without secrets is unaffected.
    #[tokio::test]
    async fn live_business_partner_search() {
        let Some(client_result) = BusinessHubClient::from_env() else {
            eprintln!("SAP_BUSINESS_HUB_KEY not set — skipping live integration test");
            return;
        };
        let client = match client_result {
            Ok(c) => c,
            Err(e) => panic!("client init failed even though key is present: {e}"),
        };
        // `S` is broad enough that the sandbox should always return rows.
        let bps = match client.search_business_partners("S", 5).await {
            Ok(b) => b,
            Err(OdataError::Status { status, body }) if status == 401 || status == 403 => {
                panic!("API key was rejected ({status}). Body: {body}");
            }
            Err(e) => panic!("live sandbox search failed: {e}"),
        };
        assert!(!bps.is_empty(), "expected at least one BP from sandbox; sandbox may be cold-empty");
        eprintln!("Live A_BusinessPartner search returned {} rows", bps.len());
        for bp in bps.iter().take(3) {
            eprintln!("  - {} : {:?}", bp.id, bp.full_name);
        }
    }
}
