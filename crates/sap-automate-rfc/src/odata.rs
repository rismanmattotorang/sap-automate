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
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum OdataError {
    #[error("http: {0}")]
    Http(String),
    #[error("auth: API key missing or empty")]
    MissingApiKey,
    #[error("auth misconfigured: {0}")]
    AuthConfig(String),
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed OData response: {0}")]
    Parse(String),
}

pub type OdataResult<T> = Result<T, OdataError>;

/// OData service path of the v4 `API_BUSINESS_PARTNER` service.  Identical
/// on the public sandbox and on a tenant — only the host differs.
const BUSINESS_PARTNER_V4_PATH: &str =
    "/sap/opu/odata4/sap/api_business_partner/srvd_a2x/sap/businesspartner/0001";

/// How a request authenticates to the OData endpoint.  The sandbox uses an
/// `APIKey` header; a customer tenant uses Basic, a pre-fetched Bearer, or
/// an OAuth2 client-credentials grant (BTP / XSUAA-fronted).
#[derive(Clone)]
pub enum OdataAuth {
    /// SAP Business Accelerator Hub `APIKey` header.
    ApiKey(String),
    /// HTTP Basic — on-prem / tenant technical user.
    Basic { user: String, password: String },
    /// Static pre-fetched bearer token (JWT).
    Bearer(String),
    /// OAuth2 client-credentials grant.  The token is fetched on first use
    /// and cached until shortly before it expires.
    OAuth2ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
    },
}

impl OdataAuth {
    /// Secret-free label for logs / diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            OdataAuth::ApiKey(_) => "api_key",
            OdataAuth::Basic { .. } => "basic",
            OdataAuth::Bearer(_) => "bearer",
            OdataAuth::OAuth2ClientCredentials { .. } => "oauth2_client_credentials",
        }
    }

    /// Reject obviously-incomplete auth before any network call.
    fn validate(&self) -> OdataResult<()> {
        match self {
            OdataAuth::ApiKey(k) if k.trim().is_empty() => Err(OdataError::MissingApiKey),
            OdataAuth::Basic { user, .. } if user.trim().is_empty() => {
                Err(OdataError::AuthConfig("basic auth requires a non-empty user".into()))
            }
            OdataAuth::Bearer(t) if t.trim().is_empty() => {
                Err(OdataError::AuthConfig("bearer auth requires a non-empty token".into()))
            }
            OdataAuth::OAuth2ClientCredentials { token_url, client_id, client_secret, .. }
                if token_url.trim().is_empty()
                    || client_id.trim().is_empty()
                    || client_secret.trim().is_empty() =>
            {
                Err(OdataError::AuthConfig(
                    "oauth2 requires token_url, client_id and client_secret".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

// Manual Debug so secrets never reach a log line via `{:?}`.
impl std::fmt::Debug for OdataAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OdataAuth::{}", self.label())
    }
}

/// Connection config.  `base_url` is the OData v4 service root; `auth`
/// selects the credential mode.
#[derive(Debug, Clone)]
pub struct BusinessHubConfig {
    pub base_url: String,
    pub auth: OdataAuth,
    pub timeout: Duration,
}

impl BusinessHubConfig {
    /// Defaults for the public sandbox `API_BUSINESS_PARTNER` v4 service
    /// (APIKey auth).  Override `base_url` for on-prem destinations.
    pub fn business_partner_sandbox(api_key: impl Into<String>) -> Self {
        Self {
            base_url: format!(
                "https://sandbox.api.sap.com/s4hanacloud{BUSINESS_PARTNER_V4_PATH}"
            ),
            auth: OdataAuth::ApiKey(api_key.into()),
            timeout: Duration::from_secs(15),
        }
    }

    /// Point the `API_BUSINESS_PARTNER` v4 service at a customer tenant by
    /// host (e.g. `https://s4hana.example.com:44300`) with any auth mode.
    pub fn tenant_business_partner(host: impl AsRef<str>, auth: OdataAuth) -> Self {
        Self {
            base_url: format!(
                "{}{BUSINESS_PARTNER_V4_PATH}",
                host.as_ref().trim_end_matches('/')
            ),
            auth,
            timeout: Duration::from_secs(15),
        }
    }

    /// Fully explicit: a service-root URL plus an auth mode.  Use this for
    /// any OData v4 service other than Business Partner.
    pub fn new(base_url: impl Into<String>, auth: OdataAuth) -> Self {
        Self {
            base_url: base_url.into(),
            auth,
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

/// `$select` projection shared by every Business Partner read.
const BP_SELECT: &str = "BusinessPartner,BusinessPartnerFullName,BusinessPartnerCategory,OrganizationBPName1,FirstName,LastName,BusinessPartnerGrouping,CreationDate";

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

#[derive(Debug)]
pub struct BusinessHubClient {
    http: reqwest::Client,
    config: BusinessHubConfig,
    /// OAuth2 token cache.  Never held across an `.await`.
    token_cache: Mutex<Option<CachedToken>>,
}

impl BusinessHubClient {
    pub fn new(config: BusinessHubConfig) -> OdataResult<Self> {
        config.auth.validate()?;
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!("sap-automate/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| OdataError::Http(e.to_string()))?;
        Ok(Self { http, config, token_cache: Mutex::new(None) })
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

    /// Build the OData client from the process environment, preferring a
    /// customer tenant over the public sandbox:
    ///
    /// - `SAP_ODATA_BASE_URL` set → tenant client.  `SAP_ODATA_AUTH` picks
    ///   the mode: `basic` (`SAP_ODATA_USER` / `SAP_ODATA_PASSWORD`),
    ///   `bearer` (`SAP_ODATA_TOKEN`), or `oauth` (`SAP_ODATA_TOKEN_URL` /
    ///   `SAP_ODATA_CLIENT_ID` / `SAP_ODATA_CLIENT_SECRET` /
    ///   `SAP_ODATA_SCOPE`).
    /// - else `SAP_BUSINESS_HUB_KEY` set → public sandbox client.
    /// - else `None` (CI without secrets skips silently).
    pub fn from_env() -> Option<OdataResult<Self>> {
        if let Some(base) = std::env::var("SAP_ODATA_BASE_URL").ok().filter(|s| !s.is_empty()) {
            return Some(Self::tenant_from_env(base));
        }
        let key = std::env::var("SAP_BUSINESS_HUB_KEY").ok()?;
        Self::from_key(&key)
    }

    fn tenant_from_env(base_url: String) -> OdataResult<Self> {
        let mode = std::env::var("SAP_ODATA_AUTH").unwrap_or_else(|_| "basic".into());
        let get = |k: &str| std::env::var(k).unwrap_or_default();
        let auth = match mode.to_ascii_lowercase().as_str() {
            "basic" => OdataAuth::Basic {
                user: get("SAP_ODATA_USER"),
                password: get("SAP_ODATA_PASSWORD"),
            },
            "bearer" => OdataAuth::Bearer(get("SAP_ODATA_TOKEN")),
            "oauth" | "oauth2" => OdataAuth::OAuth2ClientCredentials {
                token_url: get("SAP_ODATA_TOKEN_URL"),
                client_id: get("SAP_ODATA_CLIENT_ID"),
                client_secret: get("SAP_ODATA_CLIENT_SECRET"),
                scope: std::env::var("SAP_ODATA_SCOPE").ok().filter(|s| !s.is_empty()),
            },
            other => {
                return Err(OdataError::AuthConfig(format!(
                    "unknown SAP_ODATA_AUTH '{other}' (expected basic|bearer|oauth)"
                )))
            }
        };
        // If the base URL is a bare host, append the BP service path; if it
        // already looks like an OData service root, use it verbatim.
        let cfg = if base_url.contains("/sap/opu/odata") {
            BusinessHubConfig::new(base_url, auth)
        } else {
            BusinessHubConfig::tenant_business_partner(base_url, auth)
        };
        Self::new(cfg)
    }

    /// Apply the configured auth to a request builder, fetching/refreshing
    /// an OAuth2 token when needed.
    async fn apply_auth(&self, req: reqwest::RequestBuilder) -> OdataResult<reqwest::RequestBuilder> {
        Ok(match &self.config.auth {
            OdataAuth::ApiKey(k) => req.header("APIKey", k),
            OdataAuth::Basic { user, password } => req.basic_auth(user, Some(password)),
            OdataAuth::Bearer(t) => req.bearer_auth(t),
            OdataAuth::OAuth2ClientCredentials { .. } => req.bearer_auth(self.oauth_token().await?),
        })
    }

    /// Return a valid OAuth2 access token, fetching a fresh one via the
    /// client-credentials grant when the cache is empty or near expiry.
    async fn oauth_token(&self) -> OdataResult<String> {
        let OdataAuth::OAuth2ClientCredentials { token_url, client_id, client_secret, scope } =
            &self.config.auth
        else {
            return Err(OdataError::AuthConfig("oauth token requested for non-oauth auth".into()));
        };

        // Fast path: a cached, still-valid token.  Guard is dropped before
        // any await.
        if let Ok(guard) = self.token_cache.lock() {
            if let Some(t) = guard.as_ref() {
                if t.expires_at > Instant::now() {
                    return Ok(t.token.clone());
                }
            }
        }

        let mut form = vec![("grant_type", "client_credentials".to_string())];
        if let Some(s) = scope {
            form.push(("scope", s.clone()));
        }
        let resp = self
            .http
            .post(token_url)
            .basic_auth(client_id, Some(client_secret))
            .form(&form)
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| OdataError::Http(e.to_string()))?;
        if !status.is_success() {
            return Err(OdataError::Status { status: status.as_u16(), body: truncate_body(&body) });
        }
        #[derive(Deserialize)]
        struct TokenResp {
            access_token: String,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let tok: TokenResp = serde_json::from_str(&body)
            .map_err(|e| OdataError::Parse(format!("token response: {e}")))?;
        // Refresh 60 s early; default to 1 h if the server omits expires_in.
        let ttl = tok.expires_in.unwrap_or(3600).saturating_sub(60).max(1);
        if let Ok(mut guard) = self.token_cache.lock() {
            *guard = Some(CachedToken {
                token: tok.access_token.clone(),
                expires_at: Instant::now() + Duration::from_secs(ttl),
            });
        }
        Ok(tok.access_token)
    }

    fn entity_set_url(&self) -> String {
        format!("{}/A_BusinessPartner", self.config.base_url.trim_end_matches('/'))
    }

    /// `$filter=contains(BusinessPartnerFullName,'<q>')` plus a tight
    /// `$select` to keep the row size predictable.
    pub async fn search_business_partners(&self, query: &str, top: usize) -> OdataResult<Vec<BusinessPartner>> {
        let url = self.entity_set_url();
        // Escape single quotes per OData v4 §5.1.1.6.1 (doubled).
        let escaped = query.replace('\'', "''");
        let filter = format!("contains(BusinessPartnerFullName,'{}')", escaped);
        debug!(url = %url, top, "GET A_BusinessPartner (filtered)");

        let req = self.http.get(&url).header("Accept", "application/json").query(&[
            ("$top", top.to_string()),
            ("$filter", filter),
            ("$select", BP_SELECT.to_string()),
        ]);
        let response = self.apply_auth(req).await?
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;

        Self::deserialize_collection(response).await
    }

    /// `$top=N` only — useful for "show me anything" smoke tests.
    pub async fn list_business_partners(&self, top: usize) -> OdataResult<Vec<BusinessPartner>> {
        let url = self.entity_set_url();
        let req = self.http.get(&url).header("Accept", "application/json").query(&[
            ("$top", top.to_string()),
            ("$select", BP_SELECT.to_string()),
        ]);
        let response = self.apply_auth(req).await?
            .send()
            .await
            .map_err(|e| OdataError::Http(e.to_string()))?;
        Self::deserialize_collection(response).await
    }

    /// Single-entity fetch — `A_BusinessPartner('<id>')`.
    pub async fn get_business_partner(&self, id: &str) -> OdataResult<BusinessPartner> {
        let escaped = id.replace('\'', "''");
        let url = format!("{}('{}')", self.entity_set_url(), escaped);
        let req = self.http.get(&url).header("Accept", "application/json");
        let response = self.apply_auth(req).await?
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
        return s.to_string();
    }
    // Slice on a char boundary — byte-slicing panics on multibyte input,
    // and the body here is an untrusted server response.
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[+{} more chars]", &s[..end], s.len() - end)
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
        let cfg = BusinessHubConfig::new("https://example.com/odata4", OdataAuth::ApiKey("  ".into()));
        assert!(matches!(BusinessHubClient::new(cfg).unwrap_err(), OdataError::MissingApiKey));
    }

    #[test]
    fn new_accepts_real_key() {
        let cfg = BusinessHubConfig::new(
            "https://example.com/odata4",
            OdataAuth::ApiKey("any-non-empty-string".into()),
        );
        assert!(BusinessHubClient::new(cfg).is_ok());
    }

    #[test]
    fn tenant_business_partner_builds_canonical_path() {
        let cfg = BusinessHubConfig::tenant_business_partner(
            "https://s4hana.example.com:44300/",
            OdataAuth::Basic { user: "TECH".into(), password: "pw".into() },
        );
        assert_eq!(
            cfg.base_url,
            "https://s4hana.example.com:44300/sap/opu/odata4/sap/api_business_partner/srvd_a2x/sap/businesspartner/0001"
        );
        assert_eq!(cfg.auth.label(), "basic");
    }

    #[test]
    fn basic_auth_requires_user() {
        let cfg = BusinessHubConfig::new(
            "https://x/odata4",
            OdataAuth::Basic { user: "".into(), password: "pw".into() },
        );
        assert!(matches!(BusinessHubClient::new(cfg).unwrap_err(), OdataError::AuthConfig(_)));
    }

    #[test]
    fn oauth_requires_all_fields() {
        let cfg = BusinessHubConfig::new(
            "https://x/odata4",
            OdataAuth::OAuth2ClientCredentials {
                token_url: "https://auth/token".into(),
                client_id: "".into(),
                client_secret: "secret".into(),
                scope: None,
            },
        );
        assert!(matches!(BusinessHubClient::new(cfg).unwrap_err(), OdataError::AuthConfig(_)));
    }

    #[test]
    fn auth_debug_and_label_never_leak_secrets() {
        let auth = OdataAuth::Basic { user: "TECH".into(), password: "do-not-leak".into() };
        assert_eq!(auth.label(), "basic");
        assert!(!format!("{auth:?}").contains("do-not-leak"));
        let bearer = OdataAuth::Bearer("super-secret-jwt".into());
        assert!(!format!("{bearer:?}").contains("super-secret-jwt"));
        // The whole config derives Debug from OdataAuth's manual impl.
        let cfg = BusinessHubConfig::new("https://x", bearer);
        assert!(!format!("{cfg:?}").contains("super-secret-jwt"));
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

    /// Integration test against a live OData endpoint — a customer tenant
    /// (`SAP_ODATA_BASE_URL` + `SAP_ODATA_AUTH`/creds) when configured,
    /// otherwise the public Business Hub sandbox (`SAP_BUSINESS_HUB_KEY`).
    /// Skips with a printed message when neither is set — CI without
    /// secrets is unaffected.
    #[tokio::test]
    async fn live_business_partner_search() {
        let Some(client_result) = BusinessHubClient::from_env() else {
            eprintln!(
                "neither SAP_ODATA_BASE_URL nor SAP_BUSINESS_HUB_KEY set — \
                 skipping live OData integration test"
            );
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
