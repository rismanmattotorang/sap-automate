//! BAPIRET2 parser.
//!
//! Every standard SAP BAPI returns at least one row of `BAPIRET2`
//! (mostly via a `TABLES RETURN` parameter; some via a single
//! `EXPORTING RETURN`).  Agents that just look at the JSON-RPC error
//! object miss the structured business-side messages SAP emits.
//!
//! This helper turns the raw BAPIRET2 rows into a typed list that
//! tools can surface in their CallToolResult.  The shape matches the
//! DDIC structure documented in transaction SE11 (BAPIRET2):
//!
//! - `TYPE`        CHAR 1  — severity (S/E/W/I/A)
//! - `ID`          CHAR 20 — message class
//! - `NUMBER`      NUMC 3  — message number
//! - `MESSAGE`     CHAR 220 — formatted text
//! - `LOG_NO`      CHAR 20
//! - `LOG_MSG_NO`  NUMC 6
//! - `MESSAGE_V1..V4` CHAR 50 each
//! - `PARAMETER`   CHAR 32
//! - `ROW`         INT4
//! - `FIELD`       CHAR 30
//! - `SYSTEM`      CHAR 10

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BapiRet2Severity {
    /// `S` — success.
    Success,
    /// `E` — error: the BAPI did NOT complete its work.
    Error,
    /// `W` — warning.
    Warning,
    /// `I` — info.
    Info,
    /// `A` — abort: the BAPI aborted unrecoverably.
    Abort,
    /// Anything else (forward-compat with future SAP extensions).
    Unknown(char),
}

impl BapiRet2Severity {
    pub fn from_char(c: char) -> Self {
        match c.to_ascii_uppercase() {
            'S' => Self::Success,
            'E' => Self::Error,
            'W' => Self::Warning,
            'I' => Self::Info,
            'A' => Self::Abort,
            other => Self::Unknown(other),
        }
    }

    /// Whether this severity indicates the BAPI failed (the caller
    /// should NOT proceed to BAPI_TRANSACTION_COMMIT).
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Error | Self::Abort | Self::Unknown(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BapiRet2Message {
    pub severity: BapiRet2Severity,
    pub message_class: String,
    pub message_number: String,
    pub text: String,
    pub parameter: Option<String>,
    pub row: Option<i32>,
    pub field: Option<String>,
    pub system: Option<String>,
}

impl BapiRet2Message {
    pub fn is_failure(&self) -> bool { self.severity.is_failure() }
}

/// Parse a JSON value into BAPIRET2 messages.  Accepts:
///   - a single object with the standard BAPIRET2 keys
///   - an array of such objects (the common `TABLES RETURN` shape)
///   - an `outputs.RETURN` slot (the standard `mock_outputs` shape)
///
/// Returns an empty list if no recognised BAPIRET2 shape is found.
pub fn parse_bapiret2(value: &serde_json::Value) -> Vec<BapiRet2Message> {
    // Walk the value looking for BAPIRET2-shaped entries.  This
    // tolerates the various wrapping styles SAP / our mock outputs
    // emit (e.g. {"outputs":{"RETURN":[...]}}).
    let candidates = collect_candidates(value);
    candidates.into_iter().filter_map(parse_one).collect()
}

fn collect_candidates(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut out = Vec::new();
    walk(value, &mut out, 0);
    out
}

fn walk<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>, depth: usize) {
    if depth > 8 { return; }
    match v {
        serde_json::Value::Object(map) => {
            // Heuristic: looks like a BAPIRET2 row if it has both TYPE
            // and (MESSAGE or NUMBER).
            let has_type = map.contains_key("TYPE") || map.contains_key("type");
            let has_msg = map.contains_key("MESSAGE") || map.contains_key("message");
            if has_type && has_msg {
                out.push(v);
                return; // don't descend further into a single row
            }
            for v in map.values() { walk(v, out, depth + 1); }
        }
        serde_json::Value::Array(arr) => {
            for v in arr { walk(v, out, depth + 1); }
        }
        _ => {}
    }
}

fn parse_one(v: &serde_json::Value) -> Option<BapiRet2Message> {
    let obj = v.as_object()?;
    let typ = first_str(obj, &["TYPE", "type"]).unwrap_or_default();
    let sev = typ.chars().next().map(BapiRet2Severity::from_char).unwrap_or(BapiRet2Severity::Unknown(' '));
    let message_class = first_str(obj, &["ID", "id"]).unwrap_or_default();
    let message_number = first_str(obj, &["NUMBER", "number"]).unwrap_or_default();
    let text = first_str(obj, &["MESSAGE", "message"]).unwrap_or_default();
    Some(BapiRet2Message {
        severity: sev,
        message_class,
        message_number,
        text,
        parameter: first_str(obj, &["PARAMETER", "parameter"]),
        row: first_str(obj, &["ROW", "row"]).and_then(|s| s.parse().ok()),
        field: first_str(obj, &["FIELD", "field"]),
        system: first_str(obj, &["SYSTEM", "system"]),
    })
}

fn first_str(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(s) = v.as_str() { return Some(s.to_string()); }
            if v.is_number() { return Some(v.to_string()); }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_of_rows() {
        let v = serde_json::json!([
            {"TYPE": "E", "ID": "F5", "NUMBER": "806", "MESSAGE": "Posting period is not open"},
            {"TYPE": "S", "ID": "F5", "NUMBER": "099", "MESSAGE": "Document posted"},
        ]);
        let parsed = parse_bapiret2(&v);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].severity, BapiRet2Severity::Error);
        assert!(parsed[0].is_failure());
        assert_eq!(parsed[1].severity, BapiRet2Severity::Success);
        assert!(!parsed[1].is_failure());
    }

    #[test]
    fn parses_nested_outputs_return() {
        let v = serde_json::json!({
            "executed_on": "PRD",
            "outputs": {
                "RETURN": [
                    {"TYPE": "W", "ID": "FB", "NUMBER": "001", "MESSAGE": "Cost centre overridden"},
                ],
                "OBJ_KEY": "0100000123"
            }
        });
        let parsed = parse_bapiret2(&v);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "Cost centre overridden");
        assert_eq!(parsed[0].message_class, "FB");
    }

    #[test]
    fn empty_value_returns_empty_list() {
        assert!(parse_bapiret2(&serde_json::Value::Null).is_empty());
        assert!(parse_bapiret2(&serde_json::json!({"unrelated": 1})).is_empty());
    }

    #[test]
    fn severity_is_failure_classification() {
        assert!(BapiRet2Severity::Error.is_failure());
        assert!(BapiRet2Severity::Abort.is_failure());
        assert!(BapiRet2Severity::Unknown('Z').is_failure(),
            "Unknown severities must be treated as failures so unrecognised SAP responses don't cause silent commits");
        assert!(!BapiRet2Severity::Success.is_failure());
        assert!(!BapiRet2Severity::Warning.is_failure());
        assert!(!BapiRet2Severity::Info.is_failure());
    }
}
