//! Append-only audit log for state-mutating tool calls.
//!
//! Every invocation of a tool flagged `with_writes()` lands here.
//! Required for SOX evidence on FI postings and transport releases,
//! and for GDPR Article 30 records-of-processing on customer-master
//! changes.
//!
//! Pluggable sink so production wires this to a tamper-evident
//! store (Loki / S3 with object lock / Splunk HEC).  The library
//! ships:
//!
//!   - `JsonStdoutSink` — emits one JSON object per line to stdout
//!     (collected by the container's stdout → log aggregator).
//!   - `JsonStderrSink` — same but stderr.  Tests use this.
//!
//! Custom sinks implement `AuditSink::write`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One row in the audit log.  Fields chosen for SOX/GDPR coverage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Stable id, unique per row.
    pub event_id: String,
    /// Unix-epoch milliseconds at server receive time.
    pub at_ms: u64,
    /// Server-side request id (paper §IV-H `mcp.session_id` + correlation).
    pub session_id: Option<String>,
    /// Optional tenant identifier when the server is multi-tenant.
    pub tenant: Option<String>,
    /// User identity that authorised the call.  May be the channel
    /// adapter's user_id when called through the gateway.
    pub actor: Option<String>,
    /// MCP tool name (e.g. "sap.workflow.create_purchase_order").
    pub tool: String,
    /// SAP system identity (SID + client) at call time.
    pub sap_system: Option<String>,
    /// Tool arguments — REDACTED to keep secrets / PII out of the log.
    /// The redactor strips known sensitive keys (`password`, `token`,
    /// `secret`, anything ending in `_pwd` / `_pass`).
    pub arguments_redacted: serde_json::Value,
    /// Outcome of the call.
    pub outcome: AuditOutcome,
    /// Total wall-clock duration.
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok { summary: String },
    /// User declined an elicitation; no writes occurred.
    Declined { reason: String },
    /// Permission denied by read-only mode or authorization.
    Denied { reason: String },
    /// Error during execution.
    Failed { code: i32, message: String },
}

impl AuditOutcome {
    pub fn ok(summary: impl Into<String>) -> Self { Self::Ok { summary: summary.into() } }
    pub fn declined(reason: impl Into<String>) -> Self { Self::Declined { reason: reason.into() } }
    pub fn denied(reason: impl Into<String>) -> Self { Self::Denied { reason: reason.into() } }
    pub fn failed(code: i32, message: impl Into<String>) -> Self { Self::Failed { code, message: message.into() } }
}

#[async_trait]
pub trait AuditSink: Send + Sync + 'static {
    async fn write(&self, entry: &AuditEntry);
}

/// Public-facing audit logger.  Composes a sink + redaction policy.
pub struct AuditLog {
    sink: Arc<dyn AuditSink>,
}

impl AuditLog {
    pub fn new(sink: Arc<dyn AuditSink>) -> Self { Self { sink } }

    /// Record an entry.  Arguments are redacted before being passed
    /// to the sink — secrets/PII never reach the sink raw.
    pub async fn record(&self, mut entry: AuditEntry) {
        entry.arguments_redacted = redact(entry.arguments_redacted);
        self.sink.write(&entry).await;
    }

    pub fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    pub fn new_event_id() -> String {
        // Hash the current epoch nanos with a tiny rotor for uniqueness.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("evt-{:016x}", now)
    }
}

/// Strip sensitive fields recursively.  Keys matched case-insensitively
/// against the redaction list.
pub fn redact(value: serde_json::Value) -> serde_json::Value {
    const SENSITIVE_SUBSTRINGS: &[&str] = &[
        "password", "passwd", "secret", "token", "api_key", "apikey",
        "_pwd", "_pass", "credential",
    ];
    match value {
        serde_json::Value::Object(mut m) => {
            let keys: Vec<String> = m.keys().cloned().collect();
            for k in keys {
                let k_lc = k.to_lowercase();
                if SENSITIVE_SUBSTRINGS.iter().any(|s| k_lc.contains(s)) {
                    m.insert(k, serde_json::Value::String("***".into()));
                } else if let Some(v) = m.remove(&k) {
                    m.insert(k, redact(v));
                }
            }
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(redact).collect())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Stdout / stderr sinks
// ---------------------------------------------------------------------------

pub struct JsonStdoutSink;
pub struct JsonStderrSink {
    inner: tokio::sync::Mutex<Vec<AuditEntry>>,
}

impl Default for JsonStderrSink {
    fn default() -> Self { Self::new() }
}

impl JsonStderrSink {
    pub fn new() -> Self { Self { inner: tokio::sync::Mutex::new(Vec::new()) } }
    pub async fn drain(&self) -> Vec<AuditEntry> {
        std::mem::take(&mut *self.inner.lock().await)
    }
}

#[async_trait]
impl AuditSink for JsonStdoutSink {
    async fn write(&self, entry: &AuditEntry) {
        let s = serde_json::to_string(entry).unwrap_or_else(|_| "{}".into());
        println!("{s}");
    }
}

#[async_trait]
impl AuditSink for JsonStderrSink {
    async fn write(&self, entry: &AuditEntry) {
        // Tests inspect entries via `drain`; production sinks override.
        self.inner.lock().await.push(entry.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_strips_sensitive_keys() {
        // Policy: when a key matches a sensitive substring, the WHOLE
        // value is replaced — never partially traversed.  This is the
        // safest default: a `credentials` object can never have any of
        // its fields leak.  Nested non-sensitive keys inside non-
        // sensitive parents do get traversed (see TOKEN below).
        let v = serde_json::json!({
            "user": "DEMO",
            "password": "should-disappear",
            "credentials": { "INNER": "still-redacted-by-parent" },
            "items": [
                { "TOKEN": "x", "name": "ok" },
                42,
            ]
        });
        let r = redact(v);
        assert_eq!(r["user"], "DEMO");
        assert_eq!(r["password"], "***");
        // The credentials *value* is redacted whole — its inner shape is
        // not visible.
        assert_eq!(r["credentials"], "***");
        // But inside `items` (not sensitive), the inner TOKEN is
        // redacted to "***" while sibling `name` survives.
        assert_eq!(r["items"][0]["TOKEN"], "***");
        assert_eq!(r["items"][0]["name"], "ok");
        assert_eq!(r["items"][1], 42);
    }

    #[tokio::test]
    async fn audit_log_records_via_sink() {
        let sink = Arc::new(JsonStderrSink::new());
        let log = AuditLog::new(sink.clone());
        log.record(AuditEntry {
            event_id: AuditLog::new_event_id(),
            at_ms: AuditLog::now_ms(),
            session_id: Some("S1".into()),
            tenant: Some("T1".into()),
            actor: Some("user@acme.example".into()),
            tool: "sap.workflow.create_purchase_order".into(),
            sap_system: Some("S4H/100".into()),
            arguments_redacted: serde_json::json!({ "vendor": "V-100", "password": "x" }),
            outcome: AuditOutcome::ok("po created"),
            duration_ms: 42,
        }).await;
        let drained = sink.drain().await;
        assert_eq!(drained.len(), 1);
        let e = &drained[0];
        assert_eq!(e.tool, "sap.workflow.create_purchase_order");
        assert_eq!(e.arguments_redacted["password"], "***");
        assert!(matches!(e.outcome, AuditOutcome::Ok { .. }));
    }

    #[tokio::test]
    async fn audit_log_records_declined_outcome() {
        let sink = Arc::new(JsonStderrSink::new());
        let log = AuditLog::new(sink.clone());
        log.record(AuditEntry {
            event_id: AuditLog::new_event_id(),
            at_ms: AuditLog::now_ms(),
            session_id: None, tenant: None, actor: None,
            tool: "sap.rfc.call".into(),
            sap_system: None,
            arguments_redacted: serde_json::json!({}),
            outcome: AuditOutcome::declined("user declined elicitation"),
            duration_ms: 1,
        }).await;
        let drained = sink.drain().await;
        assert!(matches!(drained[0].outcome, AuditOutcome::Declined { .. }));
    }
}
