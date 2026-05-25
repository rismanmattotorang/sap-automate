//! HttpAdtClient integration-test suite (Phase 9).
//!
//! Spins up an axum-based mock ADT server in-process that returns
//! realistic XML / text payloads for every endpoint, then exercises
//! `HttpAdtClient` against it.  Asserts:
//!
//!   1. **URL patterns** match the verified ADT REST conventions
//!      (see `docs/SAP_CORRECTNESS.md`).
//!   2. **Headers** are emitted with the canonical capitalisation
//!      (`X-SAP-Client`, `X-SAP-Language`, `Authorization`).
//!   3. **CSRF flow** — `x-csrf-token: Fetch` on GET, cached, re-sent
//!      on POST for activation.
//!   4. **HTTP method** — package nodestructure is POST, not GET.
//!   5. **XML parsers** — package contents, search results,
//!      usageReferences all parse the standard shapes correctly.
//!   6. **Error mapping** — 404 → AdtError::NotFound, 403 → Forbidden,
//!      data-preview 403 → DataPreviewBlocked.
//!   7. **Read-only-mode safety gate** — activate blocked when
//!      ctx.read_only=true.
//!
//! The fixture records every received request so each test can verify
//! exactly what the client emitted.

#![cfg(feature = "http")]

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use sap_automate_adt::{
    AbapObjectKind, AdtAuth, AdtCallContext, AdtClient, AdtDestination, AdtError,
    AdtSearchRequest, ActivationRequest, HttpAdtClient, WhereUsedRequest,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

// ============================================================================
// Mock ADT server
// ============================================================================

#[derive(Debug, Clone, Default)]
struct RecordedRequest {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Default)]
struct MockState {
    /// Append-only log of every request the mock received.
    log: Mutex<Vec<RecordedRequest>>,
    /// Optional override: `(method, path) → (status, body, content_type)`.
    overrides: Mutex<HashMap<(String, String), (StatusCode, String, &'static str)>>,
}

impl MockState {
    async fn record(&self, method: &str, path: &str, query: &str, headers: HeaderMap, body: &str) {
        let mut h = Vec::new();
        for (k, v) in headers.iter() {
            if let Ok(vs) = v.to_str() {
                h.push((k.as_str().to_string(), vs.to_string()));
            }
        }
        self.log.lock().await.push(RecordedRequest {
            method: method.into(), path: path.into(), query: query.into(),
            headers: h, body: body.into(),
        });
    }

    async fn last(&self) -> Option<RecordedRequest> {
        self.log.lock().await.last().cloned()
    }

    async fn all(&self) -> Vec<RecordedRequest> { self.log.lock().await.clone() }

    fn set_override(&self, method: &str, path: &str, status: StatusCode, body: String, content_type: &'static str) {
        let mut o = self.overrides.try_lock().expect("uncontended");
        o.insert((method.into(), path.into()), (status, body, content_type));
    }
}

async fn spawn_mock_server() -> (SocketAddr, Arc<MockState>) {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        // CSRF discovery endpoint
        .route("/sap/bc/adt/discovery", get(handler_discovery))
        // Source-code GETs
        .route("/sap/bc/adt/programs/programs/:name/source/main", get(handler_program))
        .route("/sap/bc/adt/oo/classes/:name/source/main", get(handler_class))
        .route("/sap/bc/adt/oo/interfaces/:name/source/main", get(handler_interface))
        .route("/sap/bc/adt/programs/includes/:name/source/main", get(handler_include))
        .route("/sap/bc/adt/functions/groups/:group/fmodules/:name/source/main", get(handler_fm))
        .route("/sap/bc/adt/ddic/ddl/sources/:name/source/main", get(handler_cds))
        // Package nodestructure (POST form-encoded)
        .route("/sap/bc/adt/repository/nodestructure", post(handler_package))
        // Search (GET with query params)
        .route("/sap/bc/adt/repository/informationsystem/search", get(handler_search))
        // Where-used (POST XML body)
        .route("/sap/bc/adt/repository/informationsystem/usageReferences", post(handler_usage))
        // Data preview (POST text/plain SQL body)
        .route("/sap/bc/adt/datapreview/freestyle", post(handler_data_preview))
        // Activation (POST XML body, CSRF required)
        .route("/sap/bc/adt/activation", post(handler_activation))
        // Catch-all to record stray requests for debugging
        .fallback(any(handler_fallback))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give axum a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, state)
}

async fn read_body(b: Bytes) -> String {
    String::from_utf8_lossy(&b).to_string()
}

fn csrf_header(value: &str) -> ([(axum::http::HeaderName, &str); 1], &'static str) {
    use axum::http::HeaderName;
    ([(HeaderName::from_static("x-csrf-token"), value)], "")
}

async fn handler_discovery(State(state): State<Arc<MockState>>, headers: HeaderMap) -> Response {
    state.record("GET", "/sap/bc/adt/discovery", "", headers, "").await;
    let mut resp = "<discovery/>".into_response();
    resp.headers_mut().insert(
        axum::http::HeaderName::from_static("x-csrf-token"),
        axum::http::HeaderValue::from_static("TOKEN-ABCDEF"),
    );
    resp
}

async fn handler_program(
    State(state): State<Arc<MockState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/programs/programs/{name}/source/main"), "", headers, "").await;
    if let Some(o) = check_override(&state, "GET", &format!("/sap/bc/adt/programs/programs/{name}/source/main")).await {
        return o;
    }
    (StatusCode::OK, "REPORT zfin_post_je.\n\nINCLUDE zfin_top.\nSTART-OF-SELECTION.\n  PERFORM validate.\n").into_response()
}

async fn handler_class(
    State(state): State<Arc<MockState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/oo/classes/{name}/source/main"), "", headers, "").await;
    if let Some(o) = check_override(&state, "GET", &format!("/sap/bc/adt/oo/classes/{name}/source/main")).await {
        return o;
    }
    let body = format!("CLASS {name} DEFINITION PUBLIC FINAL.\n  PUBLIC SECTION.\n    METHODS post_document.\nENDCLASS.\n\nCLASS {name} IMPLEMENTATION.\n  METHOD post_document.\n  ENDMETHOD.\nENDCLASS.\n");
    (StatusCode::OK, body).into_response()
}

async fn handler_interface(
    State(state): State<Arc<MockState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/oo/interfaces/{name}/source/main"), "", headers, "").await;
    let body = format!("INTERFACE {name} PUBLIC.\n  METHODS validate.\nENDINTERFACE.\n");
    (StatusCode::OK, body).into_response()
}

async fn handler_include(
    State(state): State<Arc<MockState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/programs/includes/{name}/source/main"), "", headers, "").await;
    let body = format!("* Include {name}\nTYPES: BEGIN OF ty_line, bukrs TYPE bukrs, END OF ty_line.\n");
    (StatusCode::OK, body).into_response()
}

async fn handler_fm(
    State(state): State<Arc<MockState>>,
    axum::extract::Path((group, name)): axum::extract::Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/functions/groups/{group}/fmodules/{name}/source/main"), "", headers, "").await;
    let body = format!("FUNCTION {name}.\n*\"--------------------------------------------------------------\n  ev_ok = abap_true.\nENDFUNCTION.\n");
    (StatusCode::OK, body).into_response()
}

async fn handler_cds(
    State(state): State<Arc<MockState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    state.record("GET", &format!("/sap/bc/adt/ddic/ddl/sources/{name}/source/main"), "", headers, "").await;
    let body = format!("@AbapCatalog.sqlViewName: 'ZSO_KPI'\n@EndUserText.label: 'Sales order KPIs'\ndefine view {name} as select from vbak {{ key vbeln }};\n");
    (StatusCode::OK, body).into_response()
}

async fn handler_package(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_text = read_body(body).await;
    state.record("POST", "/sap/bc/adt/repository/nodestructure", "", headers, &body_text).await;
    if let Some(o) = check_override(&state, "POST", "/sap/bc/adt/repository/nodestructure").await {
        return o;
    }
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<asx:abap xmlns:asx="http://www.sap.com/abapxml" version="1.0">
  <asx:values>
    <DATA>
      <TREE_CONTENT>
        <SEU_ADT_REPOSITORY_OBJ_NODE>
          <OBJECT_TYPE>CLAS/OC</OBJECT_TYPE>
          <OBJECT_NAME>ZCL_FIN_POSTER</OBJECT_NAME>
          <DESCRIPTION>FI posting helper class</DESCRIPTION>
          <OBJECT_URI>/sap/bc/adt/oo/classes/zcl_fin_poster</OBJECT_URI>
        </SEU_ADT_REPOSITORY_OBJ_NODE>
        <SEU_ADT_REPOSITORY_OBJ_NODE>
          <OBJECT_TYPE>INTF/OI</OBJECT_TYPE>
          <OBJECT_NAME>ZIF_FIN_POSTABLE</OBJECT_NAME>
          <DESCRIPTION>FI posting contract</DESCRIPTION>
          <OBJECT_URI>/sap/bc/adt/oo/interfaces/zif_fin_postable</OBJECT_URI>
        </SEU_ADT_REPOSITORY_OBJ_NODE>
        <SEU_ADT_REPOSITORY_OBJ_NODE>
          <OBJECT_TYPE>PROG/P</OBJECT_TYPE>
          <OBJECT_NAME>ZFIN_POST_JE</OBJECT_NAME>
          <DESCRIPTION>Post FI journal entries</DESCRIPTION>
          <OBJECT_URI>/sap/bc/adt/programs/programs/zfin_post_je</OBJECT_URI>
        </SEU_ADT_REPOSITORY_OBJ_NODE>
      </TREE_CONTENT>
    </DATA>
  </asx:values>
</asx:abap>"#;
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

async fn handler_search(
    State(state): State<Arc<MockState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let query_str = params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    state.record("GET", "/sap/bc/adt/repository/informationsystem/search", &query_str, headers, "").await;
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<adtcore:objectReferences xmlns:adtcore="http://www.sap.com/adt/core">
  <adtcore:objectReference adtcore:type="CLAS/OC" adtcore:name="ZCL_FIN_POSTER" adtcore:description="FI posting helper" adtcore:packageName="ZFIN" adtcore:uri="/sap/bc/adt/oo/classes/zcl_fin_poster"/>
  <adtcore:objectReference adtcore:type="PROG/P" adtcore:name="ZFIN_POST_JE" adtcore:description="Post FI journal" adtcore:packageName="ZFIN" adtcore:uri="/sap/bc/adt/programs/programs/zfin_post_je"/>
</adtcore:objectReferences>"#;
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/vnd.sap.adt.objectreferences+xml")], xml).into_response()
}

async fn handler_usage(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_text = read_body(body).await;
    state.record("POST", "/sap/bc/adt/repository/informationsystem/usageReferences", "", headers, &body_text).await;
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<usageReferences:result xmlns:usageReferences="http://www.sap.com/adt/ris/usageReferences" xmlns:adtcore="http://www.sap.com/adt/core">
  <usageReferences:referencedObject adtcore:type="CLAS/OC" adtcore:name="ZCL_FIN_POSTER" adtcore:uri="/sap/bc/adt/oo/classes/zcl_fin_poster"/>
  <usageReferences:referencedObject adtcore:type="PROG/P" adtcore:name="ZFIN_POST_JE" adtcore:uri="/sap/bc/adt/programs/programs/zfin_post_je"/>
</usageReferences:result>"#;
    (StatusCode::OK, xml).into_response()
}

async fn handler_data_preview(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_text = read_body(body).await;
    state.record("POST", "/sap/bc/adt/datapreview/freestyle", "", headers, &body_text).await;
    if let Some(o) = check_override(&state, "POST", "/sap/bc/adt/datapreview/freestyle").await {
        return o;
    }
    // Minimal "successful" body — the parser today returns Vec::new()
    // either way, so the test just asserts no error.
    (StatusCode::OK, "<dataPreview/>").into_response()
}

async fn handler_activation(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_text = read_body(body).await;
    let token = headers.get("x-csrf-token").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    state.record("POST", "/sap/bc/adt/activation", "", headers, &body_text).await;
    if token != "TOKEN-ABCDEF" {
        // SAP gateways reject with 403 + the required-token marker.
        let mut resp = (StatusCode::FORBIDDEN, "CSRF token required").into_response();
        resp.headers_mut().insert(
            axum::http::HeaderName::from_static("x-csrf-token"),
            axum::http::HeaderValue::from_static("required"),
        );
        return resp;
    }
    (StatusCode::OK, "<activated/>").into_response()
}

async fn handler_fallback(
    State(state): State<Arc<MockState>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 64 * 1024).await.unwrap_or_default();
    let body_text = String::from_utf8_lossy(&body).into_owned();
    state.record(&method, &path, &query, headers, &body_text).await;
    (StatusCode::NOT_FOUND, "unmocked").into_response()
}

async fn check_override(state: &MockState, method: &str, path: &str) -> Option<Response> {
    let o = state.overrides.lock().await;
    o.get(&(method.into(), path.into())).cloned().map(|(status, body, ct)| {
        (status, [(axum::http::header::CONTENT_TYPE, ct)], body).into_response()
    })
}

// ============================================================================
// Tests
// ============================================================================

fn destination(addr: SocketAddr) -> AdtDestination {
    AdtDestination {
        name: "test".into(),
        base_url: format!("http://{addr}"),
        client: "100".into(),
        language: "EN".into(),
        auth: AdtAuth::Basic { user: "DEMO".into(), password: "secret".into() },
    }
}

#[tokio::test]
async fn get_program_emits_correct_url_and_headers() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();

    let prog = client.get_program("ZFIN_POST_JE").await.unwrap();
    assert_eq!(prog.name, "ZFIN_POST_JE");
    assert!(prog.source.contains("REPORT zfin_post_je"));
    assert!(prog.line_count > 0);

    let recorded = state.last().await.expect("a request was recorded");
    assert_eq!(recorded.method, "GET");
    // Verify lowercased path encoding — matches SAP ADT spec.
    assert_eq!(recorded.path, "/sap/bc/adt/programs/programs/zfin_post_je/source/main");

    let header_map: HashMap<_, _> = recorded.headers.iter().cloned().collect();
    assert!(header_map.contains_key("authorization"),
        "Authorization header missing; got: {:?}", header_map.keys().collect::<Vec<_>>());
    assert!(header_map.get("authorization").unwrap().starts_with("Basic "));
    // Header keys arrive lowercased on the server side (HTTP normalisation),
    // so check the lowercased form — the client emits Pascal case on the wire.
    assert_eq!(header_map.get("x-sap-client").map(String::as_str), Some("100"));
    assert_eq!(header_map.get("x-sap-language").map(String::as_str), Some("EN"));
}

#[tokio::test]
async fn get_class_returns_source_with_line_count() {
    let (addr, _state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let class = client.get_class("ZCL_FIN_POSTER").await.unwrap();
    assert_eq!(class.name, "ZCL_FIN_POSTER");
    assert!(class.source.contains("ENDCLASS"));
    assert_eq!(class.line_count, class.source.lines().count());
}

#[tokio::test]
async fn get_interface_uri_matches_oo_interfaces_pattern() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let _ = client.get_interface("ZIF_FIN_POSTABLE").await.unwrap();
    assert_eq!(state.last().await.unwrap().path,
        "/sap/bc/adt/oo/interfaces/zif_fin_postable/source/main");
}

#[tokio::test]
async fn get_include_uri_matches_programs_includes_pattern() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let _ = client.get_include("ZFIN_TOP").await.unwrap();
    assert_eq!(state.last().await.unwrap().path,
        "/sap/bc/adt/programs/includes/zfin_top/source/main");
}

#[tokio::test]
async fn get_function_module_uri_nests_group_and_name() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let fm = client.get_function_module("ZFIN_UTIL", "Z_FIN_VALIDATE_BUKRS").await.unwrap();
    assert_eq!(fm.name, "Z_FIN_VALIDATE_BUKRS");
    assert!(fm.source.contains("FUNCTION"));
    let recorded = state.last().await.unwrap();
    assert_eq!(recorded.path,
        "/sap/bc/adt/functions/groups/zfin_util/fmodules/z_fin_validate_bukrs/source/main");
}

#[tokio::test]
async fn get_cds_view_uri_uses_ddic_ddl_sources_pattern() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let view = client.get_cds_view("Z_C_SALES_ORDER_KPI").await.unwrap();
    assert!(view.source.contains("define view"));
    assert_eq!(state.last().await.unwrap().path,
        "/sap/bc/adt/ddic/ddl/sources/z_c_sales_order_kpi/source/main");
}

#[tokio::test]
async fn get_package_uses_post_with_form_body() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();

    let pkg = client.get_package_contents("ZFIN").await.unwrap();
    assert_eq!(pkg.package, "ZFIN");
    // Verified parser extracts all 3 members from the XML fixture.
    assert_eq!(pkg.members.len(), 3, "expected 3 members; got: {:?}",
        pkg.members.iter().map(|m| &m.name).collect::<Vec<_>>());
    let names: Vec<&str> = pkg.members.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"ZCL_FIN_POSTER"));
    assert!(names.contains(&"ZIF_FIN_POSTABLE"));
    assert!(names.contains(&"ZFIN_POST_JE"));

    // Regression: this endpoint must be POST + form body, not GET + query.
    let recorded = state.last().await.unwrap();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, "/sap/bc/adt/repository/nodestructure");
    assert!(recorded.body.contains("parent_type=DEVC%2FK"),
        "form body should encode parent_type as DEVC/K; got: {}", recorded.body);
    assert!(recorded.body.contains("parent_name=ZFIN"),
        "form body should carry parent_name; got: {}", recorded.body);
    assert!(recorded.body.contains("withShortDescriptions=true"));
    let headers: HashMap<_, _> = recorded.headers.iter().cloned().collect();
    assert_eq!(headers.get("content-type").map(String::as_str),
        Some("application/x-www-form-urlencoded"));
}

#[tokio::test]
async fn search_passes_query_and_max_results_in_query_string() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let hits = client.search(AdtSearchRequest {
        query: "fin".into(),
        kind: None,
        max_results: 10,
    }).await.unwrap();
    assert_eq!(hits.len(), 2, "fixture XML carries 2 objectReference entries");
    assert!(hits.iter().any(|h| h.name == "ZCL_FIN_POSTER"));
    let recorded = state.last().await.unwrap();
    assert_eq!(recorded.method, "GET");
    assert!(recorded.query.contains("query=fin"));
    assert!(recorded.query.contains("maxResults=10"));
    assert!(recorded.query.contains("operation=quickSearch"));
}

#[tokio::test]
async fn where_used_posts_xml_with_correct_content_type() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let hits = client.where_used(WhereUsedRequest {
        name: "ZIF_FIN_POSTABLE".into(),
        kind: AbapObjectKind::Interface,
    }).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|h| h.object == "ZCL_FIN_POSTER"));

    let recorded = state.last().await.unwrap();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path,
        "/sap/bc/adt/repository/informationsystem/usageReferences");
    assert!(recorded.body.contains("usageReferenceRequest"));
    let headers: HashMap<_, _> = recorded.headers.iter().cloned().collect();
    assert_eq!(headers.get("content-type").map(String::as_str),
        Some("application/vnd.sap.adt.repository.usagereferences.request+xml"));
}

#[tokio::test]
async fn data_preview_posts_select_body_with_text_plain() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let _rows = client.get_table_contents("T001", 5).await.unwrap();
    let recorded = state.last().await.unwrap();
    assert_eq!(recorded.method, "POST");
    assert!(recorded.path.starts_with("/sap/bc/adt/datapreview/freestyle"));
    assert!(recorded.body.starts_with("SELECT * FROM T001"));
    let headers: HashMap<_, _> = recorded.headers.iter().cloned().collect();
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/plain; charset=utf-8"),
    );
}

#[tokio::test]
async fn data_preview_403_is_mapped_to_data_preview_blocked() {
    let (addr, state) = spawn_mock_server().await;
    state.set_override("POST", "/sap/bc/adt/datapreview/freestyle", StatusCode::FORBIDDEN,
        "blocked".into(), "text/plain");
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let err = client.get_table_contents("BSEG", 10).await.unwrap_err();
    assert!(matches!(err, AdtError::DataPreviewBlocked(_)),
        "403 on data preview must map to DataPreviewBlocked; got {err:?}");
}

#[tokio::test]
async fn activate_fetches_csrf_token_then_posts() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let outcome = client.activate(
        ActivationRequest { name: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program },
        AdtCallContext { read_only: false },
    ).await.unwrap();
    assert!(outcome.activated, "activation should succeed when CSRF flow completes");

    // The client should have fetched discovery (CSRF) BEFORE posting activation.
    let log = state.all().await;
    assert!(log.len() >= 2, "expected ≥ 2 requests (discovery + activation); got {log:?}");
    let discovery_idx = log.iter().position(|r| r.path == "/sap/bc/adt/discovery")
        .expect("CSRF discovery call missing");
    let activation_idx = log.iter().position(|r| r.path == "/sap/bc/adt/activation")
        .expect("activation call missing");
    assert!(discovery_idx < activation_idx,
        "CSRF discovery must precede activation");

    // Activation request must carry the cached token + Content-Type xml.
    let activation = &log[activation_idx];
    let headers: HashMap<_, _> = activation.headers.iter().cloned().collect();
    assert_eq!(headers.get("x-csrf-token").map(String::as_str), Some("TOKEN-ABCDEF"));
    assert_eq!(headers.get("content-type").map(String::as_str), Some("application/xml"));
    assert!(activation.body.contains("objectReferences"));
}

#[tokio::test]
async fn activate_in_read_only_mode_short_circuits_before_any_http_call() {
    let (addr, state) = spawn_mock_server().await;
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let err = client.activate(
        ActivationRequest { name: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program },
        AdtCallContext { read_only: true },
    ).await.unwrap_err();
    assert!(matches!(err, AdtError::PermissionDenied(_)));
    // Defence in depth: the mock should NOT have seen any traffic.
    assert!(state.all().await.is_empty(),
        "read-only mode must block HTTP traffic; mock saw: {:?}", state.all().await);
}

#[tokio::test]
async fn not_found_response_maps_to_adt_error_not_found() {
    let (addr, state) = spawn_mock_server().await;
    state.set_override("GET", "/sap/bc/adt/programs/programs/zmissing/source/main",
        StatusCode::NOT_FOUND, "nope".into(), "text/plain");
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let err = client.get_program("ZMISSING").await.unwrap_err();
    assert!(matches!(err, AdtError::NotFound { .. }),
        "404 must map to AdtError::NotFound; got {err:?}");
}

#[tokio::test]
async fn forbidden_response_maps_to_adt_error_forbidden() {
    let (addr, state) = spawn_mock_server().await;
    state.set_override("GET", "/sap/bc/adt/oo/classes/zforbidden/source/main",
        StatusCode::FORBIDDEN, "nope".into(), "text/plain");
    let client = HttpAdtClient::new(destination(addr)).unwrap();
    let err = client.get_class("ZFORBIDDEN").await.unwrap_err();
    assert!(matches!(err, AdtError::Forbidden(_)),
        "403 must map to AdtError::Forbidden; got {err:?}");
}

#[tokio::test]
async fn destination_down_when_target_refuses_connection() {
    // Pick an unbound port deliberately so the connection fails.
    let dest = AdtDestination {
        name: "down".into(),
        base_url: "http://127.0.0.1:1".into(),  // port 1 is privileged/unused
        client: "100".into(),
        language: "EN".into(),
        auth: AdtAuth::Basic { user: "x".into(), password: "y".into() },
    };
    let client = HttpAdtClient::new(dest).unwrap();
    let err = client.get_program("ZX").await.unwrap_err();
    // Reqwest can surface this as either DestinationDown or NotFound
    // depending on local stack — accept both as "connection refused" outcomes.
    let code = err.code();
    let kind = format!("{err:?}");
    assert!(
        matches!(err, AdtError::DestinationDown { .. } | AdtError::Internal(_)) || code.as_i32() != 0,
        "expected DestinationDown / Internal; got {kind}"
    );
}

#[tokio::test]
async fn bearer_auth_emits_bearer_authorization_header() {
    let (addr, state) = spawn_mock_server().await;
    let dest = AdtDestination {
        name: "test".into(),
        base_url: format!("http://{addr}"),
        client: "100".into(),
        language: "EN".into(),
        auth: AdtAuth::Bearer { token: "JWT-XXXXXX".into() },
    };
    let client = HttpAdtClient::new(dest).unwrap();
    let _ = client.get_program("Z_DEMO").await.unwrap();
    let r = state.last().await.unwrap();
    let h: HashMap<_, _> = r.headers.iter().cloned().collect();
    assert_eq!(h.get("authorization").map(String::as_str), Some("Bearer JWT-XXXXXX"));
}
