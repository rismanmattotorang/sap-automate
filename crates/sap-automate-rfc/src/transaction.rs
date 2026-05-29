//! Transactional write orchestration.
//!
//! A standard SAP write BAPI does **not** persist on its own — the caller
//! must follow a successful call with `BAPI_TRANSACTION_COMMIT` (and a
//! failed one with `BAPI_TRANSACTION_ROLLBACK`).  This module wraps that
//! protocol so every write path enforces it identically:
//!
//! 1. refuse outright in read-only mode (fail-closed);
//! 2. call the BAPI;
//! 3. inspect its `BAPIRET2` — any `E`/`A`/unknown severity is a failure;
//! 4. failure → `BAPI_TRANSACTION_ROLLBACK`, success → `BAPI_TRANSACTION_COMMIT`.
//!
//! The decision (`has_failure`) is a pure function so it can be tested
//! directly; the orchestration is generic over the `SapClient` trait, so it
//! works against the mock and the live SOAP backend alike.

use crate::bapiret2::{parse_bapiret2, BapiRet2Message, BapiRet2Severity};
use crate::client::{RfcCallRequest, SapClient};
use crate::error::{RfcError, RfcResult};
use serde::Serialize;
use serde_json::{json, Value};

const COMMIT_FN: &str = "BAPI_TRANSACTION_COMMIT";
const ROLLBACK_FN: &str = "BAPI_TRANSACTION_ROLLBACK";

/// Build a synthetic message so the orchestration can surface decisions
/// (e.g. "outcome unconfirmed") in the same `messages` list as SAP's own.
fn note(severity: BapiRet2Severity, text: &str) -> BapiRet2Message {
    BapiRet2Message {
        severity,
        message_class: "SAPAUTO".into(),
        message_number: "000".into(),
        text: text.into(),
        parameter: None,
        row: None,
        field: None,
        system: None,
    }
}

/// Issue `BAPI_TRANSACTION_ROLLBACK`, returning whether it was *confirmed*
/// and any messages (including a synthetic warning when it couldn't be
/// confirmed, so a `rolled_back: true` is never reported on faith).
async fn rollback(client: &dyn SapClient) -> (bool, Vec<BapiRet2Message>) {
    match client.call_rfc(rollback_req(), false).await {
        Ok(v) => {
            let msgs = parse_bapiret2(&v);
            (!has_failure(&msgs), msgs)
        }
        Err(e) => (
            false,
            vec![note(
                BapiRet2Severity::Warning,
                &format!("rollback could not be confirmed: {e}"),
            )],
        ),
    }
}

/// Result of a transactional write.
#[derive(Debug, Clone, Serialize)]
pub struct WriteOutcome {
    pub function: String,
    /// Whether the LUW was committed (true) — i.e. the change is persisted.
    pub committed: bool,
    /// Whether a rollback was issued (because the BAPI or the commit failed).
    pub rolled_back: bool,
    /// Combined BAPIRET2 messages from the BAPI and the commit.
    pub messages: Vec<BapiRet2Message>,
    /// The raw BAPI result (export/tables), for the caller to mine for the
    /// resulting document number etc.
    pub result: Value,
}

/// True if any message indicates the BAPI failed (so we must NOT commit).
/// Unknown severities count as failures — fail-closed (see `bapiret2`).
pub fn has_failure(messages: &[BapiRet2Message]) -> bool {
    messages.iter().any(BapiRet2Message::is_failure)
}

/// Execute a write BAPI and finish its LUW with commit-or-rollback.
///
/// `read_only_mode` mirrors the server's safety flag; when true this refuses
/// to run at all.  The underlying `call_rfc` still applies the client's own
/// read-only gate, so this is defence in depth.
pub async fn execute_write_bapi(
    client: &dyn SapClient,
    request: RfcCallRequest,
    read_only_mode: bool,
) -> RfcResult<WriteOutcome> {
    if read_only_mode {
        return Err(RfcError::PermissionDenied(format!(
            "write workflow for '{}' requires write mode (--enable-writes)",
            request.function
        )));
    }
    if request.function == COMMIT_FN || request.function == ROLLBACK_FN {
        return Err(RfcError::InvalidParameter {
            name: "function".into(),
            reason: "commit/rollback are issued automatically; call the business BAPI instead"
                .into(),
        });
    }

    let function = request.function.clone();
    let result = client.call_rfc(request, false).await?;
    let mut messages = parse_bapiret2(&result);

    if has_failure(&messages) {
        let (rolled_back, rb_msgs) = rollback(client).await;
        messages.extend(rb_msgs);
        return Ok(WriteOutcome { function, committed: false, rolled_back, messages, result });
    }

    // FAIL-CLOSED: if the BAPI returned no parseable BAPIRET2 at all, we have
    // no positive confirmation of success — do NOT commit on faith.  Roll
    // back and report the outcome as unconfirmed (a BAPI that genuinely
    // returns zero rows on success is rare; the safe default for a write
    // gate is to refuse to persist an unverified change).
    if messages.is_empty() {
        let (rolled_back, rb_msgs) = rollback(client).await;
        let mut out = vec![note(
            BapiRet2Severity::Warning,
            "BAPI returned no BAPIRET2; outcome unconfirmed — not committed",
        )];
        out.extend(rb_msgs);
        return Ok(WriteOutcome { function, committed: false, rolled_back, messages: out, result });
    }

    // Non-empty and no failure → commit synchronously (WAIT = 'X').
    let commit_result = client.call_rfc(commit_req(), false).await?;
    messages.extend(parse_bapiret2(&commit_result));

    if has_failure(&messages) {
        // Commit itself reported an error — roll back to be safe.
        let (rolled_back, rb_msgs) = rollback(client).await;
        messages.extend(rb_msgs);
        return Ok(WriteOutcome { function, committed: false, rolled_back, messages, result });
    }

    Ok(WriteOutcome { function, committed: true, rolled_back: false, messages, result })
}

fn commit_req() -> RfcCallRequest {
    RfcCallRequest {
        function: COMMIT_FN.into(),
        parameters: json!({ "WAIT": "X" }),
        timeout_ms: 30_000,
        require_read_only_safe: false,
    }
}

fn rollback_req() -> RfcCallRequest {
    RfcCallRequest {
        function: ROLLBACK_FN.into(),
        parameters: Value::Null,
        timeout_ms: 30_000,
        require_read_only_safe: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bapiret2::BapiRet2Severity;
    use crate::client::MockSapClient;

    fn msg(sev: BapiRet2Severity) -> BapiRet2Message {
        BapiRet2Message {
            severity: sev,
            message_class: "X".into(),
            message_number: "000".into(),
            text: "t".into(),
            parameter: None,
            row: None,
            field: None,
            system: None,
        }
    }

    #[test]
    fn has_failure_treats_error_abort_unknown_as_failure() {
        assert!(has_failure(&[msg(BapiRet2Severity::Error)]));
        assert!(has_failure(&[msg(BapiRet2Severity::Abort)]));
        assert!(has_failure(&[msg(BapiRet2Severity::Unknown('Z'))]));
        assert!(!has_failure(&[msg(BapiRet2Severity::Success)]));
        assert!(!has_failure(&[msg(BapiRet2Severity::Warning), msg(BapiRet2Severity::Info)]));
        assert!(!has_failure(&[]));
    }

    #[tokio::test]
    async fn refuses_in_read_only_mode() {
        let client = MockSapClient::new(2, json!({"client": "100"}));
        let req = RfcCallRequest {
            function: "BAPI_PO_CREATE1".into(),
            parameters: json!({ "POHEADER": {}, "POHEADERX": {} }),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let err = execute_write_bapi(client.as_ref(), req, true).await.unwrap_err();
        assert!(matches!(err, RfcError::PermissionDenied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_direct_commit_call() {
        let client = MockSapClient::new(2, json!({"client": "100"}));
        let req = RfcCallRequest {
            function: "BAPI_TRANSACTION_COMMIT".into(),
            parameters: json!({ "WAIT": "X" }),
            timeout_ms: 1000,
            require_read_only_safe: false,
        };
        let err = execute_write_bapi(client.as_ref(), req, false).await.unwrap_err();
        assert!(matches!(err, RfcError::InvalidParameter { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn empty_bapiret2_is_fail_closed_not_committed() {
        // The mock returns no BAPIRET2, which is an *unconfirmed* outcome —
        // the gate must NOT commit it.
        let client = MockSapClient::new(2, json!({"client": "100"}));
        let req = RfcCallRequest {
            function: "BAPI_PO_CREATE1".into(),
            parameters: json!({ "POHEADER": {}, "POHEADERX": {}, "TESTRUN": "" }),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let outcome = execute_write_bapi(client.as_ref(), req, false).await.unwrap();
        assert!(!outcome.committed, "empty BAPIRET2 must not commit");
        assert!(outcome.messages.iter().any(|m| m.text.contains("unconfirmed")));
    }

    // A scripted client that returns a canned BAPIRET2 for the business BAPI
    // and a clean success for commit/rollback, so the commit/rollback
    // decision can be exercised deterministically.
    struct ScriptedClient {
        bapi_return: Value,
    }

    #[async_trait::async_trait]
    impl SapClient for ScriptedClient {
        async fn call_rfc(&self, request: RfcCallRequest, _ro: bool) -> RfcResult<Value> {
            if request.function == COMMIT_FN || request.function == ROLLBACK_FN {
                return Ok(json!({ "outputs": { "RETURN": { "TYPE": "S", "MESSAGE": "done" } } }));
            }
            Ok(json!({ "outputs": { "RETURN": self.bapi_return.clone() } }))
        }
        async fn system_info(&self) -> RfcResult<crate::client::SystemInfo> {
            Err(RfcError::Internal("unused".into()))
        }
        async fn search_rfc(&self, _q: &str, _n: usize) -> RfcResult<crate::client::RfcSearchResult> {
            Err(RfcError::Internal("unused".into()))
        }
        async fn rfc_metadata(&self, _f: &str, _l: &str) -> RfcResult<crate::client::RfcFunctionMeta> {
            Err(RfcError::Internal("unused".into()))
        }
        async fn bulk_rfc_metadata(&self, _f: &[String], _l: &str) -> RfcResult<crate::client::BulkMetadata> {
            Err(RfcError::Internal("unused".into()))
        }
        async fn read_table(&self, _r: crate::client::ReadTableRequest) -> RfcResult<Vec<crate::client::TableRow>> {
            Err(RfcError::Internal("unused".into()))
        }
        async fn table_structure(&self, _t: &str) -> RfcResult<crate::client::TableStructure> {
            Err(RfcError::Internal("unused".into()))
        }
    }

    fn scripted(bapi_return: Value) -> ScriptedClient {
        ScriptedClient { bapi_return }
    }

    #[tokio::test]
    async fn commits_on_explicit_success_row() {
        let client = scripted(json!([{ "TYPE": "S", "ID": "06", "NUMBER": "017", "MESSAGE": "PO 4500000001 created" }]));
        let req = RfcCallRequest {
            function: "BAPI_PO_CREATE1".into(),
            parameters: json!({ "POHEADER": {} }),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let outcome = execute_write_bapi(&client, req, false).await.unwrap();
        assert!(outcome.committed, "expected commit; messages={:?}", outcome.messages);
        assert!(!outcome.rolled_back);
    }

    #[tokio::test]
    async fn rolls_back_on_error_row() {
        let client = scripted(json!([{ "TYPE": "E", "ID": "06", "NUMBER": "055", "MESSAGE": "Vendor 1 blocked" }]));
        let req = RfcCallRequest {
            function: "BAPI_PO_CREATE1".into(),
            parameters: json!({ "POHEADER": {} }),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let outcome = execute_write_bapi(&client, req, false).await.unwrap();
        assert!(!outcome.committed, "error row must not commit");
        assert!(outcome.rolled_back, "error row must roll back");
    }
}
