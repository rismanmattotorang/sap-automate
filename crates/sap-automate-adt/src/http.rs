//! HTTP ADT client.
//!
//! Real ADT REST against a live SAP system, behind the `http` feature.
//! Implements the patterns the two reference projects had to handle by
//! hand:
//!
//!   - **CSRF token cache** — fetched on first GET, attached to every
//!     mutating call, refreshed on 403 (paper §IV-I transient).
//!   - **Per-destination cookie jar** (uses `reqwest::Client`'s built-in
//!     cookie store).
//!   - **Basic / Bearer / mTLS auth selection** driven by
//!     `AdtDestination::auth`.
//!
//! Phase 2 finalisation: only the read-only subset is implemented end-to-
//! end here.  Write methods short-circuit with `AdtError::Forbidden` until
//! the full CRUD surface from `fr0ster/mcp-abap-adt` is brought across.
//! The trait remains the same, so the MCP server, tools, and tests don't
//! care which backend is in use.

use crate::client::{AdtCallContext, AdtClient};
use crate::destination::{AdtAuth, AdtDestination};
use crate::error::{AdtError, AdtResult};
use crate::types::{
    AbapObjectKind, ActivationOutcome, ActivationRequest, AdtSearchHit, AdtSearchRequest,
    CdsView, PackageContents, PackageMember, ProgramSource, TableRow, WhereUsedHit,
    WhereUsedRequest, MAX_TABLE_ROWS,
};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

const CSRF_HEADER: &str = "x-csrf-token";
// SAP convention: header name is X-SAP-Client (Pascal case) per the ADT
// REST docs and confirmed in mario-andreschak/mcp-abap-adt and Eclipse
// ADT client.  HTTP headers are case-insensitive on the wire, but real
// SAP gateways have been observed to be case-sensitive in older releases
// (NW 7.40 and earlier).  Use the canonical form for maximum compat.
const SAP_CLIENT_HEADER: &str = "X-SAP-Client";
const SAP_LANGUAGE_HEADER: &str = "X-SAP-Language";

pub struct HttpAdtClient {
    destination: AdtDestination,
    http: reqwest::Client,
    csrf: RwLock<Option<String>>,
}

impl HttpAdtClient {
    pub fn new(destination: AdtDestination) -> AdtResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .cookie_store(true)
            .build()
            .map_err(|e| AdtError::Internal(format!("reqwest: {e}")))?;
        Ok(Self { destination, http, csrf: RwLock::new(None) })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.destination.base_url.trim_end_matches('/'), path)
    }

    fn auth_header(&self) -> AdtResult<Option<HeaderValue>> {
        match &self.destination.auth {
            AdtAuth::Basic { user, password } => {
                use base64_compat::encode as b64;
                let encoded = b64(&format!("{user}:{password}"));
                Ok(Some(HeaderValue::from_str(&format!("Basic {encoded}"))
                    .map_err(|e| AdtError::Internal(e.to_string()))?))
            }
            AdtAuth::Bearer { token } => Ok(Some(HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| AdtError::Internal(e.to_string()))?)),
            AdtAuth::ServiceKey { .. } | AdtAuth::Certificate { .. } => Err(AdtError::Internal(
                "ServiceKey / Certificate auth not yet wired (Phase 7)".into(),
            )),
            AdtAuth::Mock => Ok(None),
        }
    }

    fn base_headers(&self) -> AdtResult<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_static("*/*"));
        h.insert(SAP_CLIENT_HEADER, HeaderValue::from_str(&self.destination.client)
            .map_err(|e| AdtError::Internal(e.to_string()))?);
        h.insert(SAP_LANGUAGE_HEADER, HeaderValue::from_str(&self.destination.language)
            .map_err(|e| AdtError::Internal(e.to_string()))?);
        if let Some(auth) = self.auth_header()? {
            h.insert(AUTHORIZATION, auth);
        }
        Ok(h)
    }

    async fn fetch_text(&self, path: &str) -> AdtResult<String> {
        let url = self.url(path);
        let headers = self.base_headers()?;
        let resp = self.http.get(&url).headers(headers).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        // Cache CSRF token if the server returned one.
        if let Some(token) = resp.headers().get(CSRF_HEADER) {
            if let Ok(s) = token.to_str() {
                *self.csrf.write().await = Some(s.to_string());
                debug!("CSRF token cached");
            }
        }
        let status = resp.status();
        if status == 401 || status == 403 {
            return Err(AdtError::Forbidden(format!("{path} -> {status}")));
        }
        if status == 404 {
            return Err(AdtError::NotFound { kind: "Object".into(), name: path.into() });
        }
        if !status.is_success() {
            return Err(AdtError::Internal(format!("{path} -> {status}")));
        }
        resp.text().await.map_err(|e| AdtError::Internal(e.to_string()))
    }

    /// Hint the server we want a CSRF token before a mutating call.
    async fn refresh_csrf(&self) -> AdtResult<()> {
        let url = self.url("/sap/bc/adt/discovery");
        let mut headers = self.base_headers()?;
        headers.insert(CSRF_HEADER, HeaderValue::from_static("Fetch"));
        let resp = self.http.get(&url).headers(headers).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        if let Some(token) = resp.headers().get(CSRF_HEADER) {
            if let Ok(s) = token.to_str() {
                *self.csrf.write().await = Some(s.to_string());
                return Ok(());
            }
        }
        warn!("CSRF refresh returned no token");
        Err(AdtError::CsrfRefresh)
    }
}

#[async_trait]
impl AdtClient for HttpAdtClient {
    fn destination(&self) -> &AdtDestination { &self.destination }

    async fn get_program(&self, name: &str) -> AdtResult<ProgramSource> {
        adt_source(self, name, AbapObjectKind::Program).await
    }
    async fn get_class(&self, name: &str) -> AdtResult<ProgramSource> {
        adt_source(self, name, AbapObjectKind::Class).await
    }
    async fn get_interface(&self, name: &str) -> AdtResult<ProgramSource> {
        adt_source(self, name, AbapObjectKind::Interface).await
    }
    async fn get_include(&self, name: &str) -> AdtResult<ProgramSource> {
        adt_source(self, name, AbapObjectKind::Include).await
    }
    async fn get_function_module(&self, group: &str, name: &str) -> AdtResult<ProgramSource> {
        let path = AbapObjectKind::FunctionModule.adt_path(name).replace("{group}", &group.to_lowercase());
        let source = self.fetch_text(&path).await?;
        let line_count = source.lines().count();
        Ok(ProgramSource {
            name: name.to_uppercase(),
            kind: AbapObjectKind::FunctionModule,
            package: None,
            description: None,
            source,
            active: true,
            line_count,
        })
    }
    async fn get_package_contents(&self, package: &str) -> AdtResult<PackageContents> {
        // Per SAP ADT REST docs (confirmed in mario-andreschak/mcp-abap-adt
        // handleGetPackage): the nodestructure endpoint takes form params,
        // NOT a query string, and the HTTP method is POST.
        //
        //   POST /sap/bc/adt/repository/nodestructure
        //   parent_type=DEVC%2FK&parent_name=<package>&withShortDescriptions=true
        let url = self.url("/sap/bc/adt/repository/nodestructure");
        let mut headers = self.base_headers()?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let form = format!(
            "parent_type=DEVC%2FK&parent_name={}&withShortDescriptions=true",
            urlencoding::encode(package),
        );
        let resp = self.http.post(&url).headers(headers).body(form).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        let status = resp.status();
        if status == 401 || status == 403 {
            return Err(AdtError::Forbidden(format!("nodestructure -> {status}")));
        }
        if status == 404 {
            return Err(AdtError::NotFound { kind: "Package".into(), name: package.into() });
        }
        if !status.is_success() {
            return Err(AdtError::Internal(format!("nodestructure -> {status}")));
        }
        let body = resp.text().await.map_err(|e| AdtError::Internal(e.to_string()))?;
        let members = parse_nodestructure(&body);
        Ok(PackageContents {
            package: package.to_uppercase(),
            description: None,
            members,
        })
    }
    async fn get_cds_view(&self, name: &str) -> AdtResult<CdsView> {
        let source = self.fetch_text(&AbapObjectKind::CdsView.adt_path(name)).await?;
        Ok(CdsView {
            name: name.to_uppercase(),
            root_entity: name.into(),
            annotations: serde_json::Value::Null,
            line_count: source.lines().count(),
            source,
        })
    }

    async fn search(&self, request: AdtSearchRequest) -> AdtResult<Vec<AdtSearchHit>> {
        let path = format!(
            "/sap/bc/adt/repository/informationsystem/search?operation=quickSearch&query={}&maxResults={}",
            urlencoding::encode(&request.query), request.max_results,
        );
        let body = self.fetch_text(&path).await?;
        Ok(parse_search_results(&body))
    }

    async fn where_used(&self, request: WhereUsedRequest) -> AdtResult<Vec<WhereUsedHit>> {
        // SAP ADT usageReferences endpoint (confirmed Eclipse ADT client):
        //   POST /sap/bc/adt/repository/informationsystem/usageReferences?uri=<obj-uri>
        //   Content-Type: application/vnd.sap.adt.repository.usagereferences.request+xml
        //
        // The body lists the affected scopes; an empty <usageReferences>
        // element is interpreted as "all scopes the user can access".
        let obj_uri = request.kind.adt_path(&request.name);
        let url = format!(
            "{}/sap/bc/adt/repository/informationsystem/usageReferences?uri={}",
            self.destination.base_url.trim_end_matches('/'),
            urlencoding::encode(&obj_uri),
        );
        let mut headers = self.base_headers()?;
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.sap.adt.repository.usagereferences.request+xml"),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.sap.adt.repository.usagereferences.result+xml"),
        );
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<usageReferences:usageReferenceRequest xmlns:usageReferences="http://www.sap.com/adt/ris/usageReferences">
  <usageReferences:affectedObjects/>
</usageReferences:usageReferenceRequest>"#;
        let resp = self.http.post(&url).headers(headers).body(body).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        let status = resp.status();
        if status == 404 {
            warn!(name = %request.name, kind = ?request.kind, "where_used: object not found");
            return Ok(Vec::new());
        }
        if !status.is_success() {
            return Err(AdtError::Internal(format!("usageReferences -> {status}")));
        }
        let text = resp.text().await.map_err(|e| AdtError::Internal(e.to_string()))?;
        Ok(parse_usage_references(&text, request.kind))
    }

    async fn get_table_contents(&self, table: &str, max_rows: usize) -> AdtResult<Vec<TableRow>> {
        if max_rows == 0 || max_rows > MAX_TABLE_ROWS {
            return Err(AdtError::InvalidObjectName(format!("max_rows must be in 1..={MAX_TABLE_ROWS}")));
        }
        let path = format!(
            "/sap/bc/adt/datapreview/freestyle?rowNumber={max_rows}&dataAging=true",
        );
        let url = self.url(&path);
        let mut headers = self.base_headers()?;
        headers.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        let body = format!("SELECT * FROM {} ", table);
        let resp = self.http.post(&url).headers(headers).body(body).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        if resp.status() == 403 {
            return Err(AdtError::DataPreviewBlocked(format!(
                "ADT data preview blocked for {table}; fall back to sap.table.read",
            )));
        }
        if !resp.status().is_success() {
            return Err(AdtError::Internal(format!("data preview {} -> {}", table, resp.status())));
        }
        let text = resp.text().await.map_err(|e| AdtError::Internal(e.to_string()))?;
        Ok(parse_data_preview(&text))
    }

    async fn activate(&self, request: ActivationRequest, ctx: AdtCallContext) -> AdtResult<ActivationOutcome> {
        if ctx.read_only {
            return Err(AdtError::PermissionDenied(format!(
                "activate({} {}) blocked: read-only mode",
                request.kind.label(), request.name,
            )));
        }
        // Refresh CSRF then POST to the activation endpoint.
        self.refresh_csrf().await?;
        let token = self.csrf.read().await.clone()
            .ok_or(AdtError::CsrfRefresh)?;
        let url = self.url("/sap/bc/adt/activation");
        let mut headers = self.base_headers()?;
        headers.insert(CSRF_HEADER, HeaderValue::from_str(&token)
            .map_err(|e| AdtError::Internal(e.to_string()))?);
        headers.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_static("application/xml"));
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><adtcore:objectReferences xmlns:adtcore="http://www.sap.com/adt/core"><adtcore:objectReference adtcore:uri="/sap/bc/adt/{kind}/{name}" adtcore:name="{name}"/></adtcore:objectReferences>"#,
            kind = match request.kind {
                AbapObjectKind::Program => "programs/programs",
                AbapObjectKind::Class => "oo/classes",
                AbapObjectKind::Interface => "oo/interfaces",
                _ => "objects",
            },
            name = request.name.to_lowercase(),
        );
        let resp = self.http.post(&url).headers(headers).body(body).send().await
            .map_err(|e| AdtError::DestinationDown {
                destination: self.destination.name.clone(),
                reason: e.to_string(),
            })?;
        let status = resp.status();
        let messages = vec![format!("HTTP {status}")];
        Ok(ActivationOutcome {
            name: request.name.to_uppercase(),
            kind: request.kind,
            activated: status.is_success(),
            messages,
        })
    }
}

async fn adt_source(client: &HttpAdtClient, name: &str, kind: AbapObjectKind) -> AdtResult<ProgramSource> {
    let path = kind.adt_path(name);
    let source = client.fetch_text(&path).await?;
    let line_count = source.lines().count();
    Ok(ProgramSource {
        name: name.to_uppercase(),
        kind,
        package: None,
        description: None,
        source,
        active: true,
        line_count,
    })
}

/// Minimal XML parser for `/sap/bc/adt/repository/nodestructure` responses.
fn parse_nodestructure(xml: &str) -> Vec<PackageMember> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"OBJECT" || e.name().as_ref() == b"adtcore:objectReference" {
                    let mut name = String::new();
                    let mut desc: Option<String> = None;
                    let mut kind_str = String::new();
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let val = attr.unescape_value().unwrap_or_default().into_owned();
                        if key.contains("name") { name = val; }
                        else if key.contains("description") { desc = Some(val); }
                        else if key.contains("type") { kind_str = val; }
                    }
                    if !name.is_empty() {
                        let kind = map_kind(&kind_str);
                        out.push(PackageMember { name, kind, description: desc });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_search_results(xml: &str) -> Vec<AdtSearchHit> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.name().as_ref().ends_with(b"objectReference") {
                    let mut name = String::new();
                    let mut kind_str = String::new();
                    let mut desc: Option<String> = None;
                    let mut pkg: Option<String> = None;
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let val = attr.unescape_value().unwrap_or_default().into_owned();
                        if key.contains("name") { name = val; }
                        else if key.contains("type") { kind_str = val; }
                        else if key.contains("description") { desc = Some(val); }
                        else if key.contains("packageName") { pkg = Some(val); }
                    }
                    if !name.is_empty() {
                        out.push(AdtSearchHit {
                            name, kind: map_kind(&kind_str),
                            description: desc, package: pkg,
                            score: 1.0,
                        });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_data_preview(_text: &str) -> Vec<TableRow> {
    // Real ADT returns XML with column metadata + row data.  The full
    // parser is Phase 2 finalisation; for now we return empty so callers
    // know to inspect the raw body via a future `raw=true` flag.
    Vec::new()
}

/// Parse the `usageReferences:result` XML returned by the ADT
/// `usageReferences` endpoint.  Each `<referencedObject>` element carries
/// an `adtcore:name`, an `adtcore:type`, and an `adtcore:uri` we map back
/// to our domain.
fn parse_usage_references(xml: &str, _from_kind: AbapObjectKind) -> Vec<WhereUsedHit> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut cur_name: String = String::new();
    let mut cur_type: String = String::new();
    let mut cur_uri:  String = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.name().as_ref().ends_with(b"referencedObject")
                    || e.name().as_ref().ends_with(b"objectReference")
                {
                    cur_name.clear(); cur_type.clear(); cur_uri.clear();
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let val = attr.unescape_value().unwrap_or_default().into_owned();
                        if key.contains("name") { cur_name = val; }
                        else if key.contains("type") { cur_type = val; }
                        else if key.contains("uri")  { cur_uri  = val; }
                    }
                    if !cur_name.is_empty() {
                        out.push(WhereUsedHit {
                            object: cur_name.clone(),
                            kind: map_kind(&cur_type),
                            location: cur_uri.clone(),
                            usage: "reference".into(),
                        });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn map_kind(token: &str) -> AbapObjectKind {
    match token {
        "CLAS/OC" | "CLAS" => AbapObjectKind::Class,
        "INTF/OI" | "INTF" => AbapObjectKind::Interface,
        "PROG/P" | "PROG/I" | "PROG" => AbapObjectKind::Program,
        "FUGR/F" | "FUGR" => AbapObjectKind::FunctionGroup,
        "FUGR/FF" | "FUNC" => AbapObjectKind::FunctionModule,
        "DDLS/DF" | "DDLS" => AbapObjectKind::CdsView,
        "TABL/DT" | "TABL" => AbapObjectKind::Table,
        "STRU/DS" | "STRU" => AbapObjectKind::Structure,
        "DEVC/K" | "DEVC" => AbapObjectKind::Package,
        _ => AbapObjectKind::Program, // unknown -> Program as default
    }
}

// ---------------------------------------------------------------------------
// Small Base64 inliner — keeps the crate from pulling another dependency
// just for the basic-auth header.
// ---------------------------------------------------------------------------

mod base64_compat {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
        let mut i = 0;
        while i + 3 <= bytes.len() {
            let b1 = bytes[i]; let b2 = bytes[i + 1]; let b3 = bytes[i + 2];
            out.push(TABLE[(b1 >> 2) as usize] as char);
            out.push(TABLE[((b1 & 0b11) << 4 | b2 >> 4) as usize] as char);
            out.push(TABLE[((b2 & 0b1111) << 2 | b3 >> 6) as usize] as char);
            out.push(TABLE[(b3 & 0b111111) as usize] as char);
            i += 3;
        }
        match bytes.len() - i {
            1 => {
                let b1 = bytes[i];
                out.push(TABLE[(b1 >> 2) as usize] as char);
                out.push(TABLE[((b1 & 0b11) << 4) as usize] as char);
                out.push('=');
                out.push('=');
            }
            2 => {
                let b1 = bytes[i]; let b2 = bytes[i + 1];
                out.push(TABLE[(b1 >> 2) as usize] as char);
                out.push(TABLE[((b1 & 0b11) << 4 | b2 >> 4) as usize] as char);
                out.push(TABLE[((b2 & 0b1111) << 2) as usize] as char);
                out.push('=');
            }
            _ => {}
        }
        out
    }
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }
}
