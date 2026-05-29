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
    /// Logical name.  Optional in a TOML file — the lookup key used to
    /// select the file always wins (see [`AdtDestination::load`]), so the
    /// on-disk label can't silently drift from the name it's loaded under.
    #[serde(default)]
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

#[derive(Clone, Serialize, Deserialize)]
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

#[cfg(all(test, feature = "http"))]
mod loader_tests {
    use super::{AdtAuth, AdtDestination};
    use std::io::Write;

    fn temp_toml(body: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "sap-automate-dest-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        path.push(unique);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn loads_basic_auth_destination_and_overrides_name() {
        let path = temp_toml(
            r#"
            name = "stale-on-disk-label"
            base_url = "https://s4dev.example.com:44300"
            client = "100"
            language = "EN"

            [auth]
            type = "basic"
            user = "TECHUSER"
            password = "s3cret"
        "#,
        );
        let dest = AdtDestination::load_from_path(&path, "dev-s4").unwrap();
        std::fs::remove_file(&path).ok();

        // The lookup key wins over the on-disk label.
        assert_eq!(dest.name, "dev-s4");
        assert_eq!(dest.base_url, "https://s4dev.example.com:44300");
        assert_eq!(dest.client, "100");
        match &dest.auth {
            AdtAuth::Basic { user, password } => {
                assert_eq!(user, "TECHUSER");
                assert_eq!(password, "s3cret");
            }
            other => panic!("expected basic auth, got {other:?}"),
        }
    }

    #[test]
    fn name_field_is_optional_in_file() {
        let path = temp_toml(
            r#"
            base_url = "https://s4dev.example.com:44300"
            client = "100"

            [auth]
            type = "bearer"
            token = "jwt-abc"
        "#,
        );
        let dest = AdtDestination::load_from_path(&path, "dev-s4").unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(dest.name, "dev-s4");
        assert_eq!(dest.language, "EN"); // serde default
        assert_eq!(dest.auth.label(), "bearer");
    }

    #[test]
    fn redacted_never_leaks_password() {
        let path = temp_toml(
            r#"
            base_url = "https://s4dev.example.com:44300"
            client = "100"

            [auth]
            type = "basic"
            user = "TECHUSER"
            password = "do-not-leak"
        "#,
        );
        let dest = AdtDestination::load_from_path(&path, "dev-s4").unwrap();
        std::fs::remove_file(&path).ok();
        let redacted = dest.redacted().to_string();
        assert!(!redacted.contains("do-not-leak"), "password leaked: {redacted}");
        assert!(!redacted.contains("TECHUSER"), "user leaked: {redacted}");
        assert!(redacted.contains("\"auth_type\":\"basic\""));
    }

    #[test]
    fn search_paths_order_with_dir_override() {
        // Pure builder — no env mutation, fully deterministic.
        let paths = AdtDestination::build_search_paths(
            "dev-s4",
            Some("/etc/sap-dest"),
            Some(std::path::PathBuf::from("/home/u/.config")),
        );
        assert_eq!(paths[0], std::path::PathBuf::from("/etc/sap-dest/dev-s4.toml"));
        assert_eq!(paths[1], std::path::PathBuf::from("./.sap-automate/destinations/dev-s4.toml"));
        assert_eq!(paths[2], std::path::PathBuf::from("/home/u/.config/sap-automate/destinations/dev-s4.toml"));
    }

    #[test]
    fn search_paths_without_override_start_project_local() {
        let paths = AdtDestination::build_search_paths("dev-s4", None, None);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], std::path::PathBuf::from("./.sap-automate/destinations/dev-s4.toml"));
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

impl AdtAuth {
    /// Secret-free label for logs / diagnostics.
    pub fn label(&self) -> &'static str {
        auth_type_label(self)
    }
}

// Manual Debug so a stray `{:?}` (on AdtAuth or the AdtDestination that
// contains it) can never leak a password / bearer token / key path.
impl std::fmt::Debug for AdtAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AdtAuth::{}", self.label())
    }
}

/// Disk loading for named destinations.  Lives behind the `http` feature
/// because it depends on `toml`; the type itself is always available so
/// the offline `MockAdtClient` path needs no feature.
#[cfg(feature = "http")]
mod loader {
    use super::AdtDestination;
    use crate::error::{AdtError, AdtResult};
    use std::path::{Path, PathBuf};

    impl AdtDestination {
        /// Candidate paths for a named destination, highest priority first:
        ///
        /// 1. `$SAP_AUTOMATE_DESTINATION_DIR/<name>.toml`
        /// 2. `./.sap-automate/destinations/<name>.toml` (project-local)
        /// 3. `$XDG_CONFIG_HOME` (or `~/.config`)`/sap-automate/destinations/<name>.toml`
        pub fn config_search_paths(name: &str) -> Vec<PathBuf> {
            let dir_override = std::env::var("SAP_AUTOMATE_DESTINATION_DIR")
                .ok()
                .filter(|s| !s.is_empty());
            let config_home = std::env::var("XDG_CONFIG_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")));
            Self::build_search_paths(name, dir_override.as_deref(), config_home)
        }

        /// Pure path assembly, factored out of [`Self::config_search_paths`]
        /// so it can be unit-tested without mutating process-global env
        /// (which is `unsafe` and racy in a multi-threaded test binary).
        pub(crate) fn build_search_paths(
            name: &str,
            dir_override: Option<&str>,
            config_home: Option<PathBuf>,
        ) -> Vec<PathBuf> {
            let file = format!("{name}.toml");
            let mut paths = Vec::new();
            if let Some(dir) = dir_override {
                paths.push(PathBuf::from(dir).join(&file));
            }
            paths.push(PathBuf::from("./.sap-automate/destinations").join(&file));
            if let Some(base) = config_home {
                paths.push(base.join("sap-automate/destinations").join(&file));
            }
            paths
        }

        /// Parse a destination from a specific TOML file.  The supplied
        /// `name` always overrides any `name` field in the file.
        pub fn load_from_path(path: &Path, name: &str) -> AdtResult<Self> {
            let raw = std::fs::read_to_string(path).map_err(|e| {
                AdtError::Internal(format!("read destination {}: {e}", path.display()))
            })?;
            // SECURITY: never interpolate the `toml` error's Display — it
            // echoes the offending source line verbatim, which for a syntax
            // error on a `password`/`token` line would leak the secret into
            // stderr/logs.  Report location-free, detail-free.
            let mut dest: AdtDestination = toml::from_str(&raw).map_err(|_e| {
                AdtError::Internal(format!(
                    "destination {} is not valid TOML (syntax error; \
                     detail withheld to avoid logging credential lines)",
                    path.display()
                ))
            })?;
            dest.name = name.to_string();
            Self::warn_if_world_readable(path);
            Ok(dest)
        }

        /// Warn (non-fatally) when a credential-bearing destination file is
        /// group- or world-accessible.  Unix-only; a no-op elsewhere.
        fn warn_if_world_readable(path: &Path) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(path) {
                    let mode = meta.permissions().mode() & 0o777;
                    if mode & 0o077 != 0 {
                        tracing::warn!(
                            path = %path.display(),
                            mode = format!("{mode:o}"),
                            "destination file is group/other-accessible — it holds \
                             credentials; restrict to 0600"
                        );
                    }
                }
            }
            #[cfg(not(unix))]
            let _ = path;
        }

        /// Load a destination by name from the first file found on the
        /// search path.  Errors with `NotFound` if no file matches.
        pub fn load(name: &str) -> AdtResult<Self> {
            for path in Self::config_search_paths(name) {
                if path.is_file() {
                    return Self::load_from_path(&path, name);
                }
            }
            Err(AdtError::NotFound {
                kind: "Destination".into(),
                name: name.into(),
            })
        }
    }
}
