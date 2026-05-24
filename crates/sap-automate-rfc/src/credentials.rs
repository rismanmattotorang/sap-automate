//! Layered credential provider.
//!
//! Mirrors the priority chain from `thupalo/sap-rfc-mcp-server`
//! (env → keyring → encrypted file → .env), but the chain itself is the
//! configurable artefact — callers compose any number of providers in any
//! order, and the first one that yields credentials wins.
//!
//! For Phase 2 we ship `EnvCredentialProvider` (env vars) and
//! `StaticCredentialProvider` (literal values, useful for tests).  Keyring
//! and encrypted-file providers will follow in Phase 7 (security hardening)
//! when the OAuth flow is also finalised.

use crate::error::{RfcError, RfcResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub ashost: String,
    pub sysnr: String,
    pub client: String,
    pub user: String,
    /// Stored only for the lifetime of the running process.  Never logged.
    #[serde(skip_serializing)]
    pub password: String,
    pub language: String,
    #[serde(default)]
    pub saprouter: Option<String>,
    /// Where this credential came from (for audit logs).
    pub source: CredentialSource,
}

impl Credentials {
    /// Redacted summary safe for logs and the `sap.system.info` resource.
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "ashost": self.ashost,
            "sysnr": self.sysnr,
            "client": self.client,
            "user": self.user,
            "language": self.language,
            "saprouter": self.saprouter,
            "source": format!("{:?}", self.source),
            "password": "***",
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CredentialSource {
    Env,
    Keyring,
    EncryptedFile,
    DotEnv,
    Static,
    None,
}

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    /// Returns `Ok(None)` if this provider has no credentials configured
    /// (so the caller can move to the next in the chain).
    async fn fetch(&self) -> RfcResult<Option<Credentials>>;
}

// ---------------------------------------------------------------------------
// Environment provider
// ---------------------------------------------------------------------------

pub struct EnvCredentialProvider;

impl EnvCredentialProvider {
    pub fn new() -> Self { Self }
}

impl Default for EnvCredentialProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl CredentialProvider for EnvCredentialProvider {
    async fn fetch(&self) -> RfcResult<Option<Credentials>> {
        let needed = ["SAP_ASHOST", "SAP_SYSNR", "SAP_CLIENT", "SAP_USER", "SAP_PASSWD"];
        let present: Vec<_> = needed.iter().filter(|k| std::env::var(*k).is_ok()).collect();
        if present.is_empty() { return Ok(None); }
        if present.len() < needed.len() {
            return Err(RfcError::AuthFailed(format!(
                "partial SAP env vars: missing {:?}",
                needed.iter().filter(|k| std::env::var(*k).is_err()).collect::<Vec<_>>(),
            )));
        }
        Ok(Some(Credentials {
            ashost: std::env::var("SAP_ASHOST").unwrap(),
            sysnr: std::env::var("SAP_SYSNR").unwrap(),
            client: std::env::var("SAP_CLIENT").unwrap(),
            user: std::env::var("SAP_USER").unwrap(),
            password: std::env::var("SAP_PASSWD").unwrap(),
            language: std::env::var("SAP_LANG").unwrap_or_else(|_| "EN".to_string()),
            saprouter: std::env::var("SAP_SAPROUTER").ok(),
            source: CredentialSource::Env,
        }))
    }
}

// ---------------------------------------------------------------------------
// Static provider (tests, demos)
// ---------------------------------------------------------------------------

pub struct StaticCredentialProvider {
    creds: Credentials,
}

impl StaticCredentialProvider {
    pub fn new(creds: Credentials) -> Self { Self { creds } }
}

#[async_trait]
impl CredentialProvider for StaticCredentialProvider {
    async fn fetch(&self) -> RfcResult<Option<Credentials>> {
        Ok(Some(self.creds.clone()))
    }
}

// ---------------------------------------------------------------------------
// Layered chain
// ---------------------------------------------------------------------------

/// Tries each underlying provider in order; the first that returns
/// `Some(creds)` wins.  Returns `Ok(None)` only if every provider was empty.
pub struct LayeredCredentialProvider {
    providers: Vec<Arc<dyn CredentialProvider>>,
}

impl LayeredCredentialProvider {
    pub fn new() -> Self { Self { providers: Vec::new() } }

    pub fn add(mut self, p: Arc<dyn CredentialProvider>) -> Self {
        self.providers.push(p);
        self
    }
}

impl Default for LayeredCredentialProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl CredentialProvider for LayeredCredentialProvider {
    async fn fetch(&self) -> RfcResult<Option<Credentials>> {
        for p in &self.providers {
            match p.fetch().await {
                Ok(Some(c)) => return Ok(Some(c)),
                Ok(None) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_returns_credentials() {
        let p = StaticCredentialProvider::new(Credentials {
            ashost: "sap.example".into(),
            sysnr: "00".into(),
            client: "100".into(),
            user: "DEMO".into(),
            password: "x".into(),
            language: "EN".into(),
            saprouter: None,
            source: CredentialSource::Static,
        });
        let creds = p.fetch().await.unwrap().unwrap();
        assert_eq!(creds.client, "100");
        let r = creds.redacted();
        assert_eq!(r["password"], "***");
    }

    #[tokio::test]
    async fn layered_falls_through() {
        let layered = LayeredCredentialProvider::new()
            .add(Arc::new(EnvCredentialProvider::new())) // unset env => None
            .add(Arc::new(StaticCredentialProvider::new(Credentials {
                ashost: "fallback.sap".into(), sysnr: "01".into(), client: "100".into(),
                user: "DEMO".into(), password: "x".into(), language: "EN".into(),
                saprouter: None, source: CredentialSource::Static,
            })));
        let creds = layered.fetch().await.unwrap().unwrap();
        assert_eq!(creds.ashost, "fallback.sap");
    }
}
