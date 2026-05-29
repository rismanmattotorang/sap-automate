//! SOAP RFC client — the real RFC path without the NetWeaver RFC SDK.
//!
//! SAP exposes RFC-enabled function modules over HTTP via the SOAP runtime
//! at `/sap/bc/soap/rfc`.  This module posts a SOAP envelope naming the RFC
//! and its parameters, then maps the response back to JSON / the typed
//! `SapClient` shapes.  It needs only `reqwest` + `quick-xml` — no C SDK,
//! no FFI, no non-redistributable download.
//!
//! ## What is live vs. curated
//!
//! Data operations hit the real system:
//!   - `read_table`      → `RFC_READ_TABLE`   (DELIMITER mode)
//!   - `system_info`     → `RFC_SYSTEM_INFO`
//!   - `table_structure` → `DDIF_FIELDINFO_GET`
//!   - `call_rfc`        → the named function, generically
//!
//! Metadata operations (`rfc_metadata`, `search_rfc`, `bulk_rfc_metadata`)
//! delegate to a **curated catalogue** (`Arc<dyn SapClient>`, typically the
//! shipped RFC catalogue).  That catalogue also drives the read-only safety
//! gate in `call_rfc`: a function the catalogue marks as state-modifying
//! (or doesn't know at all) is refused in read-only mode — fail-closed.

use crate::client::{
    BulkMetadata, ReadTableRequest, RfcCallRequest, RfcFunctionMeta, RfcSearchResult, SapClient,
    SystemInfo, TableField, TableRow, TableStructure, MAX_ROWS_HARD_CAP,
};
use crate::error::{RfcError, RfcResult};
use async_trait::async_trait;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const RFC_NAMESPACE: &str = "urn:sap-com:document:sap:rfc:functions";
const ROW_DELIMITER: char = '|';

/// Connection settings for the SOAP RFC endpoint.
#[derive(Debug, Clone)]
pub struct SoapRfcConfig {
    /// Host root, e.g. `https://s4dev.example.com:44300`.  The
    /// `/sap/bc/soap/rfc` path is appended automatically.
    pub base_url: String,
    pub client: String,
    pub user: String,
    /// Never logged.
    pub password: String,
    pub language: String,
    pub timeout: Duration,
}

impl SoapRfcConfig {
    /// Build from dedicated `SAP_RFC_*` env vars, independent of the native
    /// RFC credential provider (which requires `SAP_ASHOST`/`SAP_SYSNR`,
    /// irrelevant for the HTTP SOAP transport).  Enabled only when
    /// `SAP_RFC_HTTP_URL` is set (else `None` → offline mock):
    ///
    ///   - `SAP_RFC_HTTP_URL`  (required) e.g. `https://host:44300`
    ///   - `SAP_RFC_CLIENT`    (default `100`)
    ///   - `SAP_RFC_USER`      (default empty → 401 until set)
    ///   - `SAP_RFC_PASSWORD`  (default empty)
    ///   - `SAP_RFC_LANG`      (default `EN`)
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("SAP_RFC_HTTP_URL").ok().filter(|s| !s.is_empty())?;
        let var = |k: &str, default: &str| {
            std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
        };
        Some(Self {
            base_url,
            client: var("SAP_RFC_CLIENT", "100"),
            user: var("SAP_RFC_USER", ""),
            password: var("SAP_RFC_PASSWORD", ""),
            language: var("SAP_RFC_LANG", "EN"),
            timeout: Duration::from_secs(60),
        })
    }

    /// Redacted form for logs.
    pub fn redacted(&self) -> Value {
        json!({
            "base_url": self.base_url,
            "client": self.client,
            "user": self.user,
            "language": self.language,
        })
    }
}

pub struct SoapRfcClient {
    http: reqwest::Client,
    config: SoapRfcConfig,
    /// Curated metadata catalogue + read-only safety source.
    catalogue: Arc<dyn SapClient>,
}

impl SoapRfcClient {
    pub fn new(config: SoapRfcConfig, catalogue: Arc<dyn SapClient>) -> RfcResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!("sap-automate/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| RfcError::Internal(format!("reqwest: {e}")))?;
        Ok(Self { http, config, catalogue })
    }

    pub fn config(&self) -> &SoapRfcConfig {
        &self.config
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/sap/bc/soap/rfc?sap-client={}",
            self.config.base_url.trim_end_matches('/'),
            self.config.client
        )
    }

    /// Post a SOAP envelope for `function` with the pre-rendered parameter
    /// XML, returning the response payload (the function element's children)
    /// as JSON.
    async fn invoke(&self, function: &str, params_xml: &str) -> RfcResult<Value> {
        // Defense in depth: the function name becomes an XML tag, so it must
        // be a safe ABAP identifier.  (call_rfc validates caller input up
        // front; the data-op methods pass constants.)
        if !is_safe_name(function) {
            return Err(RfcError::InvalidParameter {
                name: "function".into(),
                reason: format!("'{function}' is not a valid RFC name"),
            });
        }
        let url = self.endpoint();
        let envelope = build_envelope(function, params_xml);
        debug!(function, "POST /sap/bc/soap/rfc");

        let resp = self
            .http
            .post(&url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=utf-8")
            .header("SOAPAction", format!("\"{RFC_NAMESPACE}:{function}\""))
            .body(envelope)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    RfcError::Timeout { timeout_ms: self.config.timeout.as_millis() as u64 }
                } else {
                    RfcError::DestinationDown {
                        destination: self.config.base_url.clone(),
                        reason: e.to_string(),
                    }
                }
            })?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| RfcError::Internal(e.to_string()))?;

        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(RfcError::AuthFailed(format!("SOAP RFC returned HTTP {status}")));
        }

        // A SOAP fault may arrive with HTTP 500 (or 200).  Parse first and
        // surface the faultstring if present; otherwise map the HTTP status.
        let root = xml_to_json_root(&text)?;
        if let Some(fault) = extract_fault(&root) {
            return Err(RfcError::Internal(format!("SOAP fault: {fault}")));
        }
        if !status.is_success() {
            return Err(RfcError::Internal(format!(
                "SOAP RFC {function} -> HTTP {status}: {}",
                truncate(&text)
            )));
        }
        extract_body_payload(root)
    }
}

#[async_trait]
impl SapClient for SoapRfcClient {
    async fn system_info(&self) -> RfcResult<SystemInfo> {
        let payload = self.invoke("RFC_SYSTEM_INFO", "").await?;
        let si = payload.get("RFCSI_EXPORT").unwrap_or(&payload);
        let get = |k: &str| si.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        Ok(SystemInfo {
            sid: get("RFCSYSID"),
            client: self.config.client.clone(),
            release: get("RFCSAPRL"),
            system_role: String::new(),
            host: get("RFCHOST"),
            // RFC_SYSTEM_INFO has no clean instance-number field; leave it
            // empty rather than mislabel RFCDEST (the destination name).
            instance: String::new(),
            identity: self.config.redacted(),
        })
    }

    async fn search_rfc(&self, query: &str, limit: usize) -> RfcResult<RfcSearchResult> {
        // Curated knowledge — descriptions, read-only flags, authorizations.
        self.catalogue.search_rfc(query, limit).await
    }

    async fn rfc_metadata(&self, function: &str, language: &str) -> RfcResult<RfcFunctionMeta> {
        self.catalogue.rfc_metadata(function, language).await
    }

    async fn bulk_rfc_metadata(&self, functions: &[String], language: &str) -> RfcResult<BulkMetadata> {
        self.catalogue.bulk_rfc_metadata(functions, language).await
    }

    async fn call_rfc(&self, request: RfcCallRequest, read_only_mode: bool) -> RfcResult<Value> {
        // Read-only safety gate via the curated catalogue (fail-closed for
        // unknown functions).
        match self.catalogue.rfc_metadata(&request.function, &self.config.language).await {
            Ok(meta) => {
                if read_only_mode && !meta.read_only {
                    return Err(RfcError::PermissionDenied(format!(
                        "function '{}' modifies state; not callable in read-only mode",
                        request.function
                    )));
                }
            }
            Err(RfcError::NotFound(_)) => {
                if read_only_mode {
                    return Err(RfcError::PermissionDenied(format!(
                        "function '{}' is not in the curated read-only catalogue; \
                         refusing to call it in read-only mode",
                        request.function
                    )));
                }
                warn!(function = %request.function, "calling uncatalogued RFC (writes enabled)");
            }
            Err(e) => return Err(e),
        }

        let args = match &request.parameters {
            Value::Object(m) => m.clone(),
            Value::Null => Map::new(),
            other => {
                return Err(RfcError::InvalidParameter {
                    name: "parameters".into(),
                    reason: format!("expected object, got {other}"),
                })
            }
        };
        // Parameter / structure-component / table names become XML tags, so
        // every key must be a safe ABAP identifier — otherwise a crafted key
        // could break out of its element and smuggle a *second* RFC into the
        // envelope, bypassing the read-only gate above (XML injection).
        validate_keys(&Value::Object(args.clone()))?;
        let mut params_xml = String::new();
        write_params(&args, &mut params_xml);
        let outputs = self.invoke(&request.function, &params_xml).await?;
        Ok(json!({
            "function": request.function,
            "executed_on": self.config.base_url,
            "outputs": outputs,
        }))
    }

    async fn read_table(&self, request: ReadTableRequest) -> RfcResult<Vec<TableRow>> {
        if request.max_rows == 0 {
            return Err(RfcError::InvalidParameter {
                name: "max_rows".into(),
                reason: "must be >= 1".into(),
            });
        }
        if request.max_rows > MAX_ROWS_HARD_CAP {
            return Err(RfcError::TableBufferOverflow {
                table: request.table.clone(),
                max_rows: request.max_rows,
            });
        }

        // RFC_READ_TABLE OPTIONS clauses are capped at 72 chars each.
        for c in &request.where_conditions {
            if c.len() > 72 {
                return Err(RfcError::InvalidParameter {
                    name: "where_conditions".into(),
                    reason: format!("clause exceeds RFC_READ_TABLE's 72-char limit: '{c}'"),
                });
            }
        }

        let mut params = Map::new();
        params.insert("QUERY_TABLE".into(), json!(request.table));
        params.insert("DELIMITER".into(), json!(ROW_DELIMITER.to_string()));
        params.insert("ROWCOUNT".into(), json!(request.max_rows));
        if !request.fields.is_empty() {
            params.insert(
                "FIELDS".into(),
                Value::Array(request.fields.iter().map(|f| json!({ "FIELDNAME": f })).collect()),
            );
        }
        if !request.where_conditions.is_empty() {
            params.insert(
                "OPTIONS".into(),
                Value::Array(request.where_conditions.iter().map(|w| json!({ "TEXT": w })).collect()),
            );
        }

        let mut params_xml = String::new();
        write_params(&params, &mut params_xml);
        let payload = self.invoke("RFC_READ_TABLE", &params_xml).await?;
        parse_read_table(&payload)
    }

    async fn table_structure(&self, table: &str) -> RfcResult<TableStructure> {
        let mut params = Map::new();
        params.insert("TABNAME".into(), json!(table));
        params.insert("LANGU".into(), json!(self.config.language));
        let mut params_xml = String::new();
        write_params(&params, &mut params_xml);
        let payload = self.invoke("DDIF_FIELDINFO_GET", &params_xml).await?;
        parse_field_info(table, &payload)
    }
}

// ===========================================================================
// Envelope building
// ===========================================================================

fn build_envelope(function: &str, params_xml: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\" \
xmlns:rfc=\"{RFC_NAMESPACE}\">\
<soap:Body><rfc:{function}>{params_xml}</rfc:{function}></soap:Body>\
</soap:Envelope>"
    )
}

/// Render a JSON parameter object into RFC SOAP child elements.
fn write_params(obj: &Map<String, Value>, out: &mut String) {
    for (k, v) in obj {
        write_value(k, v, out);
    }
}

fn write_value(tag: &str, v: &Value, out: &mut String) {
    match v {
        Value::Null => {}
        Value::Bool(b) => {
            // ABAP CHAR1 flag convention: 'X' true, ' ' false.
            out.push_str(&format!("<{tag}>{}</{tag}>", if *b { "X" } else { "" }));
        }
        Value::Number(n) => out.push_str(&format!("<{tag}>{n}</{tag}>")),
        Value::String(s) => out.push_str(&format!("<{tag}>{}</{tag}>", xml_escape(s))),
        Value::Array(items) => {
            out.push_str(&format!("<{tag}>"));
            for item in items {
                out.push_str("<item>");
                match item {
                    Value::Object(m) => write_params(m, out),
                    scalar => write_scalar(scalar, out),
                }
                out.push_str("</item>");
            }
            out.push_str(&format!("</{tag}>"));
        }
        Value::Object(m) => {
            out.push_str(&format!("<{tag}>"));
            write_params(m, out);
            out.push_str(&format!("</{tag}>"));
        }
    }
}

fn write_scalar(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => out.push_str(&xml_escape(s)),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::Bool(b) => out.push_str(if *b { "X" } else { "" }),
        _ => {}
    }
}

/// True for valid ABAP RFC / field / structure-component names.  These are
/// emitted as XML element names, so anything outside this charset is
/// rejected to prevent XML injection.  Allows `/` for namespaced objects
/// (e.g. `/SAPAPO/…`).
fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 60
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/')
}

/// Recursively reject any object key that isn't a safe identifier.  Values
/// are not checked here — they're XML-escaped at render time.
fn validate_keys(v: &Value) -> RfcResult<()> {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if !is_safe_name(k) {
                    return Err(RfcError::InvalidParameter {
                        name: k.clone(),
                        reason: "invalid RFC parameter/field name (must be A-Z 0-9 _ /)".into(),
                    });
                }
                validate_keys(val)?;
            }
        }
        Value::Array(a) => {
            for item in a {
                validate_keys(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ===========================================================================
// Response parsing (generic XML → JSON)
// ===========================================================================

/// Cap on XML nesting depth, to bound the recursive parser's stack against a
/// hostile or malformed (untrusted) response.
const MAX_XML_DEPTH: usize = 256;

fn xml_to_json_root(xml: &str) -> RfcResult<Value> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => return parse_children(&mut reader, 1),
            Ok(Event::Eof) => return Err(RfcError::Internal("empty SOAP response".into())),
            Ok(_) => {}
            Err(e) => return Err(RfcError::Internal(format!("XML parse: {e}"))),
        }
        buf.clear();
    }
}

/// Parse the children of the element whose `Start` was just consumed, up to
/// its matching `End`.  Text-only elements become a JSON string; elements
/// with children become a JSON object (repeated child tags fold to arrays).
/// Each recursion level owns its read buffer, so there are no cross-level
/// borrow hazards.
fn parse_children(reader: &mut Reader<&[u8]>, depth: usize) -> RfcResult<Value> {
    if depth > MAX_XML_DEPTH {
        return Err(RfcError::Internal(format!(
            "XML nesting exceeds {MAX_XML_DEPTH} levels"
        )));
    }
    let mut buf = Vec::new();
    let mut map: Map<String, Value> = Map::new();
    let mut text = String::new();
    let mut child_count = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                let child = parse_children(reader, depth + 1)?;
                insert_merge(&mut map, name, child);
                child_count += 1;
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                insert_merge(&mut map, name, Value::String(String::new()));
                child_count += 1;
            }
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.unescape() {
                    text.push_str(&s);
                }
            }
            Ok(Event::CData(t)) => text.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(RfcError::Internal(format!("XML parse: {e}"))),
        }
    }
    if child_count == 0 {
        Ok(Value::String(text.trim().to_string()))
    } else {
        Ok(Value::Object(map))
    }
}

fn local_name(qname: &[u8]) -> String {
    let s = String::from_utf8_lossy(qname);
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.to_string(),
    }
}

fn insert_merge(map: &mut Map<String, Value>, key: String, val: Value) {
    match map.get_mut(&key) {
        Some(Value::Array(arr)) => arr.push(val),
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, val]);
        }
        None => {
            map.insert(key, val);
        }
    }
}

/// The SOAP Body's single child element, as JSON.
fn extract_body_payload(root: Value) -> RfcResult<Value> {
    let body = root
        .get("Body")
        .and_then(Value::as_object)
        .ok_or_else(|| RfcError::Internal("SOAP response has no Body".into()))?;
    let (_, payload) = body
        .iter()
        .next()
        .ok_or_else(|| RfcError::Internal("SOAP Body is empty".into()))?;
    Ok(payload.clone())
}

fn extract_fault(root: &Value) -> Option<String> {
    let body = root.get("Body")?.as_object()?;
    let fault = body.get("Fault")?;
    let s = fault
        .get("faultstring")
        .and_then(Value::as_str)
        .unwrap_or("unknown SOAP fault");
    Some(s.to_string())
}

/// Coerce a possibly-single / possibly-array / possibly-absent node into a
/// vec of items.
fn as_items(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => vec![],
        Some(other) => vec![other.clone()],
    }
}

fn field_str(item: &Value, key: &str) -> String {
    item.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Map an `RFC_READ_TABLE` response payload (DELIMITER mode) to rows.
fn parse_read_table(payload: &Value) -> RfcResult<Vec<TableRow>> {
    let fields_node = payload.get("FIELDS");
    let field_names: Vec<String> = as_items(fields_node.and_then(|f| f.get("item")))
        .iter()
        .map(|it| field_str(it, "FIELDNAME"))
        .filter(|s| !s.is_empty())
        .collect();
    if field_names.is_empty() {
        return Err(RfcError::Internal("RFC_READ_TABLE returned no FIELDS metadata".into()));
    }

    let data_items = as_items(payload.get("DATA").and_then(|d| d.get("item")));
    let mut rows = Vec::with_capacity(data_items.len());
    for item in &data_items {
        let wa = field_str(item, "WA");
        // DELIMITER mode: values are '|'-joined in field order.  A value
        // containing the delimiter is a known RFC_READ_TABLE limitation.
        let parts: Vec<&str> = wa.splitn(field_names.len(), ROW_DELIMITER).collect();
        let mut values = Map::new();
        for (i, name) in field_names.iter().enumerate() {
            let raw = parts.get(i).copied().unwrap_or("");
            values.insert(name.clone(), Value::String(raw.trim_end().to_string()));
        }
        rows.push(TableRow { values });
    }
    Ok(rows)
}

/// Map a `DDIF_FIELDINFO_GET` response payload to a `TableStructure`.
fn parse_field_info(table: &str, payload: &Value) -> RfcResult<TableStructure> {
    let dfies = as_items(payload.get("DFIES_TAB").and_then(|d| d.get("item")));
    if dfies.is_empty() {
        return Err(RfcError::NotFound(table.to_string()));
    }
    let mut fields = Vec::with_capacity(dfies.len());
    let mut key_fields = Vec::new();
    let mut table_text = String::new();
    for it in &dfies {
        let name = field_str(it, "FIELDNAME");
        if name.is_empty() {
            continue;
        }
        let is_key = field_str(it, "KEYFLAG") == "X";
        if is_key {
            key_fields.push(name.clone());
        }
        if table_text.is_empty() {
            table_text = field_str(it, "DDTEXT");
        }
        let length = field_str(it, "LENG").parse::<u32>().unwrap_or(0);
        let desc = field_str(it, "FIELDTEXT");
        fields.push(TableField {
            name,
            data_element: field_str(it, "ROLLNAME"),
            type_token: field_str(it, "DATATYPE"),
            length,
            description: if desc.is_empty() { None } else { Some(desc) },
            key: is_key,
        });
    }
    Ok(TableStructure {
        table: table.to_string(),
        description: table_text,
        fields,
        key_fields,
        authorization_group: String::new(),
        s4hana_storage: None,
    })
}

fn truncate(s: &str) -> String {
    const LIMIT: usize = 300;
    if s.len() <= LIMIT {
        return s.to_string();
    }
    // Slice on a char boundary — byte-slicing panics on multibyte input,
    // and the body here is an untrusted server response.
    let mut end = LIMIT;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[+{} chars]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::MockSapClient;

    fn catalogue() -> Arc<dyn SapClient> {
        MockSapClient::new(2, json!({"client": "100"}))
    }

    fn cfg() -> SoapRfcConfig {
        SoapRfcConfig {
            base_url: "https://s4dev.example.com:44300".into(),
            client: "100".into(),
            user: "TECH".into(),
            password: "pw".into(),
            language: "EN".into(),
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn envelope_wraps_function_and_params() {
        let env = build_envelope("RFC_SYSTEM_INFO", "<X>1</X>");
        assert!(env.contains("<rfc:RFC_SYSTEM_INFO><X>1</X></rfc:RFC_SYSTEM_INFO>"));
        assert!(env.contains(RFC_NAMESPACE));
        assert!(env.contains("soap:Body"));
    }

    #[test]
    fn write_params_renders_scalars_tables_and_escapes() {
        let mut out = String::new();
        let params = json!({
            "QUERY_TABLE": "T001",
            "ROWCOUNT": 5,
            "FIELDS": [{"FIELDNAME": "BUKRS"}, {"FIELDNAME": "BUTXT"}],
            "OPTIONS": [{"TEXT": "BUKRS = '10 & 20'"}]
        });
        write_params(params.as_object().unwrap(), &mut out);
        assert!(out.contains("<QUERY_TABLE>T001</QUERY_TABLE>"));
        assert!(out.contains("<ROWCOUNT>5</ROWCOUNT>"));
        assert!(out.contains("<FIELDS><item><FIELDNAME>BUKRS</FIELDNAME></item><item><FIELDNAME>BUTXT</FIELDNAME></item></FIELDS>"));
        assert!(out.contains("&amp;"), "ampersand must be escaped: {out}");
        assert!(!out.contains("10 & 20"));
    }

    #[test]
    fn bool_renders_as_abap_flag() {
        let mut out = String::new();
        write_value("FLAG", &json!(true), &mut out);
        assert_eq!(out, "<FLAG>X</FLAG>");
        out.clear();
        write_value("FLAG", &json!(false), &mut out);
        assert_eq!(out, "<FLAG></FLAG>");
    }

    #[test]
    fn xml_to_json_folds_repeated_tags_to_arrays() {
        let xml = r#"<root><a>1</a><a>2</a><b>x</b></root>"#;
        let v = xml_to_json_root(xml).unwrap();
        assert_eq!(v["a"], json!(["1", "2"]));
        assert_eq!(v["b"], json!("x"));
    }

    #[test]
    fn parse_read_table_splits_delimited_rows() {
        // Synthetic RFC_READ_TABLE response (DELIMITER mode).
        let xml = r#"<?xml version="1.0"?>
        <soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
          <soap:Body>
            <rfc:RFC_READ_TABLE.Response xmlns:rfc="urn:sap-com:document:sap:rfc:functions">
              <FIELDS>
                <item><FIELDNAME>BUKRS</FIELDNAME></item>
                <item><FIELDNAME>BUTXT</FIELDNAME></item>
              </FIELDS>
              <DATA>
                <item><WA>1000|ACME Berlin     </WA></item>
                <item><WA>2000|ACME Paris      </WA></item>
              </DATA>
            </rfc:RFC_READ_TABLE.Response>
          </soap:Body>
        </soap:Envelope>"#;
        let root = xml_to_json_root(xml).unwrap();
        let payload = extract_body_payload(root).unwrap();
        let rows = parse_read_table(&payload).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].values["BUKRS"], json!("1000"));
        assert_eq!(rows[0].values["BUTXT"], json!("ACME Berlin"));
        assert_eq!(rows[1].values["BUKRS"], json!("2000"));
    }

    #[test]
    fn parse_field_info_maps_keys_and_types() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
          <soap:Body>
            <n:DDIF_FIELDINFO_GET.Response xmlns:n="urn:sap-com:document:sap:rfc:functions">
              <DFIES_TAB>
                <item><FIELDNAME>MANDT</FIELDNAME><ROLLNAME>MANDT</ROLLNAME><DATATYPE>CLNT</DATATYPE><LENG>3</LENG><KEYFLAG>X</KEYFLAG><FIELDTEXT>Client</FIELDTEXT><DDTEXT>Company Codes</DDTEXT></item>
                <item><FIELDNAME>BUKRS</FIELDNAME><ROLLNAME>BUKRS</ROLLNAME><DATATYPE>CHAR</DATATYPE><LENG>4</LENG><KEYFLAG>X</KEYFLAG><FIELDTEXT>Company Code</FIELDTEXT></item>
                <item><FIELDNAME>BUTXT</FIELDNAME><ROLLNAME>BUTXT</ROLLNAME><DATATYPE>CHAR</DATATYPE><LENG>25</LENG><KEYFLAG></KEYFLAG><FIELDTEXT>Name</FIELDTEXT></item>
              </DFIES_TAB>
            </n:DDIF_FIELDINFO_GET.Response>
          </soap:Body>
        </soap:Envelope>"#;
        let payload = extract_body_payload(xml_to_json_root(xml).unwrap()).unwrap();
        let st = parse_field_info("T001", &payload).unwrap();
        assert_eq!(st.table, "T001");
        assert_eq!(st.description, "Company Codes");
        assert_eq!(st.fields.len(), 3);
        assert_eq!(st.key_fields, vec!["MANDT", "BUKRS"]);
        assert_eq!(st.fields[1].name, "BUKRS");
        assert_eq!(st.fields[1].length, 4);
        assert!(st.fields[1].key);
        assert!(!st.fields[2].key);
    }

    #[test]
    fn fault_is_surfaced() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
          <soap:Body><soap:Fault><faultcode>SOAP-ENV:Client</faultcode><faultstring>Function not found</faultstring></soap:Fault></soap:Body>
        </soap:Envelope>"#;
        let root = xml_to_json_root(xml).unwrap();
        assert_eq!(extract_fault(&root).as_deref(), Some("Function not found"));
    }

    #[tokio::test]
    async fn read_only_gate_blocks_uncatalogued_function() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        let req = RfcCallRequest {
            function: "ZZ_UNKNOWN_WRITE".into(),
            parameters: json!({}),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        // read_only_mode = true → fail closed (no network call attempted).
        let err = client.call_rfc(req, true).await.unwrap_err();
        assert!(matches!(err, RfcError::PermissionDenied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn read_only_gate_blocks_known_write_bapi() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        // BAPI_PO_CREATE1 is in the seeded catalogue and is read_only=false.
        let req = RfcCallRequest {
            function: "BAPI_PO_CREATE1".into(),
            parameters: json!({}),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let err = client.call_rfc(req, true).await.unwrap_err();
        assert!(matches!(err, RfcError::PermissionDenied(_)), "got {err:?}");
    }

    #[test]
    fn is_safe_name_accepts_rfc_identifiers_rejects_injection() {
        assert!(is_safe_name("RFC_READ_TABLE"));
        assert!(is_safe_name("/SAPAPO/OM_ORDER"));
        assert!(is_safe_name("BUKRS"));
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("FOO></rfc:X><rfc:BAPI_USER_CREATE1>"));
        assert!(!is_safe_name("HAS SPACE"));
        assert!(!is_safe_name("a<b"));
    }

    #[tokio::test]
    async fn call_rfc_rejects_injection_in_param_keys() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        // RFC_READ_TABLE is catalogued read-only, so it passes the gate;
        // the malicious *key* must still be rejected before any network I/O.
        let req = RfcCallRequest {
            function: "RFC_READ_TABLE".into(),
            parameters: json!({ "QUERY_TABLE></x><y": "T001" }),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        let err = client.call_rfc(req, false).await.unwrap_err();
        assert!(matches!(err, RfcError::InvalidParameter { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn call_rfc_rejects_injection_in_function_name() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        let req = RfcCallRequest {
            function: "RFC_READ_TABLE></rfc:X><rfc:BAPI_PO_CREATE1".into(),
            parameters: json!({}),
            timeout_ms: 1000,
            require_read_only_safe: true,
        };
        // Not in catalogue + read-only → blocked by the gate anyway; with
        // writes enabled it must be rejected as an invalid name (not sent).
        let err = client.call_rfc(req, false).await.unwrap_err();
        assert!(matches!(err, RfcError::InvalidParameter { .. }), "got {err:?}");
    }

    #[test]
    fn truncate_is_char_boundary_safe() {
        // 200 multibyte chars (2 bytes each = 400 bytes) → must not panic
        // and must stay valid UTF-8.
        let s = "ü".repeat(200);
        let out = truncate(&s);
        assert!(out.contains("more chars") || out.contains("chars]"));
        assert!(out.is_char_boundary(0)); // trivially valid UTF-8 String
    }

    #[tokio::test]
    async fn read_table_rejects_oversized_rowcount() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        let req = ReadTableRequest {
            table: "T001".into(),
            fields: vec![],
            where_conditions: vec![],
            max_rows: MAX_ROWS_HARD_CAP + 1,
        };
        let err = client.read_table(req).await.unwrap_err();
        assert!(matches!(err, RfcError::TableBufferOverflow { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn read_table_rejects_overlong_where_clause() {
        let client = SoapRfcClient::new(cfg(), catalogue()).unwrap();
        let req = ReadTableRequest {
            table: "T001".into(),
            fields: vec![],
            where_conditions: vec!["X".repeat(80)],
            max_rows: 10,
        };
        let err = client.read_table(req).await.unwrap_err();
        assert!(matches!(err, RfcError::InvalidParameter { .. }), "got {err:?}");
    }

    /// Live SOAP RFC smoke test against a real system.  Skips unless
    /// `SAP_RFC_HTTP_URL` (+ `SAP_RFC_USER` / `SAP_RFC_PASSWORD`) are set,
    /// so CI without SAP access stays green.
    #[tokio::test]
    async fn live_read_table_t000() {
        let Some(config) = SoapRfcConfig::from_env() else {
            eprintln!("SAP_RFC_HTTP_URL not set — skipping live SOAP RFC test");
            return;
        };
        let client = SoapRfcClient::new(config, catalogue()).unwrap();
        let rows = client
            .read_table(ReadTableRequest {
                table: "T000".into(), // client table — exists everywhere
                fields: vec!["MANDT".into(), "MTEXT".into()],
                where_conditions: vec![],
                max_rows: 5,
            })
            .await
            .expect("live RFC_READ_TABLE T000");
        assert!(!rows.is_empty(), "expected at least one client row from T000");
        eprintln!("live RFC_READ_TABLE T000 returned {} rows", rows.len());
    }
}
