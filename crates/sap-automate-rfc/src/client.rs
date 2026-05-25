//! SapClient trait and offline mock implementation.
//!
//! The trait is the central abstraction the MCP server depends on.  Two
//! concrete backends are envisioned:
//!   - `MockSapClient` — ships now; deterministic in-memory fixtures so the
//!     full MCP tool surface (system info / RFC search / RFC metadata / RFC
//!     call / table read / table structure / bulk metadata) is callable
//!     offline and in CI.  This is what makes Phase 2 demonstrable without
//!     a live SAP system.
//!   - `NetweaverSapClient` (Phase 2 finalisation): wraps a real RFC SDK
//!     binding behind the same trait.  Adoption needs no MCP server change.
//!
//! Pattern note: every method takes `&self` and returns a `RfcResult`.
//! The pool / circuit-breaker / retry helpers wrap calls externally so
//! individual backends stay simple.

use crate::error::{RfcError, RfcResult};
use crate::pool::ConnectionPool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

// ===========================================================================
// Shared types
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// e.g. "S4H"
    pub sid: String,
    /// e.g. "100"
    pub client: String,
    /// e.g. "SAP S/4HANA 2024 FPS00"
    pub release: String,
    /// e.g. "PRD"
    pub system_role: String,
    pub host: String,
    pub instance: String,
    /// `Credentials::redacted()`.
    pub identity: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RfcParamDirection { Import, Export, Changing, Tables }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfcParameter {
    pub name: String,
    pub direction: RfcParamDirection,
    /// ABAP type token (e.g. `CHAR(10)`, `MATNR`, `STRUCT(BAPIMATHEAD)`).
    #[serde(rename = "type")]
    pub type_token: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfcFunctionMeta {
    pub function: String,
    pub description: String,
    /// e.g. "FBAS" / "MM" / "SD"
    pub function_group: String,
    /// Devclass / package, e.g. "ZFIN".
    #[serde(default)]
    pub package: Option<String>,
    pub parameters: Vec<RfcParameter>,
    #[serde(default)]
    pub deprecated: bool,
    /// Whether the function is safe to call read-only.  Surfaces the
    /// MDK-inspired read-only-mode safety property (CData pattern).
    pub read_only: bool,
    /// Whether the function modifies SAP state and therefore needs a
    /// follow-up BAPI_TRANSACTION_COMMIT to persist.  Paper Phase 7
    /// correctness audit: every write-side BAPI in the standard SAP
    /// catalogue is uncommitted by default.
    #[serde(default)]
    pub commit_required: bool,
    /// S_RFC authorization entries required to execute this function.
    /// Used by the server to advise the agent before a call goes out.
    #[serde(default)]
    pub authorization: Vec<S_RfcAuth>,
    /// S/4HANA-specific note.  Many BAPIs were either deprecated, had
    /// their storage layer redirected to ACDOCA, or were superseded by
    /// the Business Partner unification.
    #[serde(default)]
    pub s4hana_notes: Option<String>,
}

/// One row of an S_RFC authorization object.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S_RfcAuth {
    /// Authorization object name.  Always "S_RFC" for now.
    pub object: String,
    /// e.g. "FUGR" (function group) or "FUNC" (function).
    pub rfc_type: String,
    /// Function or group name; "*" for wildcard.
    pub rfc_name: String,
    /// Activity: "16" = execute.
    pub actvt: String,
}

impl S_RfcAuth {
    pub fn execute_group(group: &str) -> Self {
        Self { object: "S_RFC".into(), rfc_type: "FUGR".into(), rfc_name: group.into(), actvt: "16".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfcFunctionSummary {
    pub function: String,
    pub description: String,
    pub function_group: String,
    pub read_only: bool,
    /// Rank score from the search; higher = better match.
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfcSearchResult {
    pub query: String,
    pub hits: Vec<RfcFunctionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfcCallRequest {
    pub function: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// If true, the call will be rejected when the client is in read-only
    /// mode AND the function is not declared `read_only` in its metadata.
    #[serde(default = "default_true")]
    pub require_read_only_safe: bool,
}

fn default_timeout_ms() -> u64 { 30_000 }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkMetadata {
    pub language: String,
    pub functions: Vec<RfcFunctionMeta>,
    /// Functions that were requested but not found.
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableField {
    pub name: String,
    pub data_element: String,
    /// ABAP-side type (e.g. `CHAR`, `NUMC`, `DEC`, `DATS`).
    #[serde(rename = "type")]
    pub type_token: String,
    pub length: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStructure {
    pub table: String,
    pub description: String,
    pub fields: Vec<TableField>,
    pub key_fields: Vec<String>,
    /// SAP authorization group (TBRG_D).  Drives S_TABU_DIS.  Sensitive
    /// tables (BSEG, PA0008) carry restricted groups; open tables carry
    /// `&NC&` ("not classified").  Empty string for views.
    #[serde(default)]
    pub authorization_group: String,
    /// S/4HANA storage note.  Empty for tables that are unchanged
    /// between ECC and S/4HANA; populated for compatibility views.
    #[serde(default)]
    pub s4hana_storage: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadTableRequest {
    pub table: String,
    /// Column projection; empty = all fields.
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub where_conditions: Vec<String>,
    /// Hard cap.  We default to 100 and refuse more than 1000 (buffer
    /// overflow safety, matching the Python reference project).
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
}

fn default_max_rows() -> usize { 100 }

pub const MAX_ROWS_HARD_CAP: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub values: serde_json::Map<String, serde_json::Value>,
}

// ===========================================================================
// SapClient trait
// ===========================================================================

#[async_trait]
pub trait SapClient: Send + Sync {
    async fn system_info(&self) -> RfcResult<SystemInfo>;

    async fn search_rfc(&self, query: &str, limit: usize) -> RfcResult<RfcSearchResult>;

    async fn rfc_metadata(&self, function: &str, language: &str) -> RfcResult<RfcFunctionMeta>;

    async fn bulk_rfc_metadata(&self, functions: &[String], language: &str) -> RfcResult<BulkMetadata>;

    async fn call_rfc(&self, request: RfcCallRequest, read_only_mode: bool) -> RfcResult<serde_json::Value>;

    async fn read_table(&self, request: ReadTableRequest) -> RfcResult<Vec<TableRow>>;

    async fn table_structure(&self, table: &str) -> RfcResult<TableStructure>;

    /// Pool snapshot for the TUI / Prometheus dashboards.
    fn pool_status(&self) -> PoolStatus {
        PoolStatus { cap: 0, available: 0 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PoolStatus { pub cap: usize, pub available: usize }

// ===========================================================================
// MockSapClient — offline reference implementation
// ===========================================================================

/// Mock client backed by realistic SAP-shaped fixtures.
///
/// The fixture set is intentionally small but covers FI, MM, SD, and HR
/// canon: ATC-relevant BAPIs, common tables, expected error shapes.  Lets
/// the MCP server be exercised end-to-end without an SAP system.
pub struct MockSapClient {
    pool: ConnectionPool,
    functions: HashMap<String, RfcFunctionMeta>,
    tables: HashMap<String, MockTable>,
    identity: serde_json::Value,
}

struct MockTable {
    structure: TableStructure,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

impl MockSapClient {
    pub fn new(pool_size: usize, identity: serde_json::Value) -> Arc<Self> {
        let mut s = Self {
            pool: ConnectionPool::new(pool_size),
            functions: HashMap::new(),
            tables: HashMap::new(),
            identity,
        };
        s.seed_functions();
        s.seed_tables();
        Arc::new(s)
    }

    fn seed_functions(&mut self) {
        for f in seed_functions() {
            self.functions.insert(f.function.clone(), f);
        }
    }

    fn seed_tables(&mut self) {
        for t in seed_tables() {
            self.tables.insert(t.structure.table.clone(), t);
        }
    }
}

#[async_trait]
impl SapClient for MockSapClient {
    async fn system_info(&self) -> RfcResult<SystemInfo> {
        let _p = self.pool.acquire().await?;
        Ok(SystemInfo {
            sid: "S4H".into(),
            client: "100".into(),
            release: "SAP S/4HANA 2024 FPS00 (mock)".into(),
            system_role: "DEV".into(),
            host: "mock.sap.example".into(),
            instance: "00".into(),
            identity: self.identity.clone(),
        })
    }

    async fn search_rfc(&self, query: &str, limit: usize) -> RfcResult<RfcSearchResult> {
        let _p = self.pool.acquire().await?;
        let q = query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<RfcFunctionSummary> = self.functions.values()
            .filter_map(|f| {
                let hay = format!("{} {} {}", f.function.to_lowercase(), f.description.to_lowercase(), f.function_group.to_lowercase());
                let score: usize = terms.iter().map(|t| hay.matches(t).count()).sum();
                if score == 0 { None }
                else {
                    Some(RfcFunctionSummary {
                        function: f.function.clone(),
                        description: f.description.clone(),
                        function_group: f.function_group.clone(),
                        read_only: f.read_only,
                        score: score as f32,
                    })
                }
            })
            .collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit.max(1));
        Ok(RfcSearchResult { query: query.into(), hits })
    }

    async fn rfc_metadata(&self, function: &str, _language: &str) -> RfcResult<RfcFunctionMeta> {
        let _p = self.pool.acquire().await?;
        self.functions.get(function)
            .cloned()
            .ok_or_else(|| RfcError::NotFound(function.into()))
    }

    async fn bulk_rfc_metadata(&self, functions: &[String], language: &str) -> RfcResult<BulkMetadata> {
        let _p = self.pool.acquire().await?;
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for f in functions {
            match self.functions.get(f) {
                Some(meta) => out.push(meta.clone()),
                None => missing.push(f.clone()),
            }
        }
        Ok(BulkMetadata { language: language.into(), functions: out, missing })
    }

    async fn call_rfc(&self, request: RfcCallRequest, read_only_mode: bool) -> RfcResult<serde_json::Value> {
        let _p = self.pool.acquire().await?;
        let meta = self.functions.get(&request.function)
            .ok_or_else(|| RfcError::NotFound(request.function.clone()))?;
        if read_only_mode && !meta.read_only {
            return Err(RfcError::PermissionDenied(format!(
                "function '{}' modifies state; not callable in read-only mode",
                request.function,
            )));
        }

        // Validate that every required parameter is present.
        let args = match &request.parameters {
            serde_json::Value::Object(m) => m.clone(),
            serde_json::Value::Null => serde_json::Map::new(),
            other => return Err(RfcError::InvalidParameter {
                name: "parameters".into(),
                reason: format!("expected object, got {}", other),
            }),
        };
        for p in &meta.parameters {
            if p.direction == RfcParamDirection::Import && !p.optional && !args.contains_key(&p.name) {
                return Err(RfcError::InvalidParameter {
                    name: p.name.clone(),
                    reason: "required import parameter missing".into(),
                });
            }
        }

        // Mock execution: echo + synthetic export.  Real backends invoke the RFC.
        debug!(function = %request.function, "mock RFC executed");
        Ok(serde_json::json!({
            "function": request.function,
            "executed_on": "mock.sap.example",
            "inputs": args,
            "outputs": mock_outputs(meta, &args),
        }))
    }

    async fn read_table(&self, request: ReadTableRequest) -> RfcResult<Vec<TableRow>> {
        let _p = self.pool.acquire().await?;
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
        let table = self.tables.get(&request.table)
            .ok_or_else(|| RfcError::NotFound(request.table.clone()))?;

        // Field projection.
        let projection: Vec<String> = if request.fields.is_empty() {
            table.structure.fields.iter().map(|f| f.name.clone()).collect()
        } else {
            for f in &request.fields {
                if !table.structure.fields.iter().any(|tf| tf.name.eq_ignore_ascii_case(f)) {
                    return Err(RfcError::InvalidParameter {
                        name: "fields".into(),
                        reason: format!("unknown field '{f}'"),
                    });
                }
            }
            request.fields.clone()
        };

        let conditions = parse_conditions(&request.where_conditions)?;
        let mut rows: Vec<TableRow> = Vec::new();
        for row in &table.rows {
            if conditions.iter().all(|(field, op, value)| match_row(row, field, op, value)) {
                let projected: serde_json::Map<String, serde_json::Value> = projection.iter()
                    .filter_map(|f| row.iter().find(|(k, _)| k.eq_ignore_ascii_case(f)).map(|(k, v)| (k.clone(), v.clone())))
                    .collect();
                rows.push(TableRow { values: projected });
                if rows.len() >= request.max_rows { break; }
            }
        }
        Ok(rows)
    }

    async fn table_structure(&self, table: &str) -> RfcResult<TableStructure> {
        let _p = self.pool.acquire().await?;
        self.tables.get(table)
            .map(|t| t.structure.clone())
            .ok_or_else(|| RfcError::NotFound(table.into()))
    }

    fn pool_status(&self) -> PoolStatus {
        PoolStatus { cap: self.pool.cap(), available: self.pool.available() }
    }
}

fn mock_outputs(meta: &RfcFunctionMeta, _args: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for p in &meta.parameters {
        if p.direction == RfcParamDirection::Export {
            out.insert(p.name.clone(), serde_json::Value::String(format!("<mock {}>", p.type_token)));
        }
    }
    serde_json::Value::Object(out)
}

/// Parse "FIELD = 'value'" / "FIELD LIKE 'pattern'" into (field, op, value).
fn parse_conditions(raw: &[String]) -> RfcResult<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for s in raw {
        let trimmed = s.trim();
        // Supported operators: = , LIKE
        let (field, op, val) = if let Some(idx) = trimmed.to_uppercase().find(" LIKE ") {
            let f = trimmed[..idx].trim().to_string();
            let v = trimmed[idx + 6..].trim().trim_matches('\'').to_string();
            (f, "LIKE".into(), v)
        } else if let Some(idx) = trimmed.find('=') {
            let f = trimmed[..idx].trim().to_string();
            let v = trimmed[idx + 1..].trim().trim_matches('\'').to_string();
            (f, "=".into(), v)
        } else {
            return Err(RfcError::InvalidParameter {
                name: "where_conditions".into(),
                reason: format!("unsupported clause '{s}' (expected FIELD = 'value' or FIELD LIKE 'pattern')"),
            });
        };
        out.push((field, op, val));
    }
    Ok(out)
}

fn match_row(row: &serde_json::Map<String, serde_json::Value>, field: &str, op: &str, value: &str) -> bool {
    let actual = row.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(field))
        .map(|(_, v)| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    match op {
        "=" => actual.eq_ignore_ascii_case(value),
        "LIKE" => sql_like(&actual, value),
        _ => false,
    }
}

fn sql_like(haystack: &str, pattern: &str) -> bool {
    let h = haystack.to_lowercase();
    let p = pattern.to_lowercase();
    // Translate '%' -> '.*' and '_' -> '.' minimally.
    let mut re = String::with_capacity(p.len() + 4);
    re.push('^');
    for c in p.chars() {
        match c {
            '%' => re.push_str(".*"),
            '_' => re.push('.'),
            c if "\\.+?^${}()|[]".contains(c) => { re.push('\\'); re.push(c); }
            c => re.push(c),
        }
    }
    re.push('$');
    // Cheap substring fallback if pattern has no wildcards.
    if !p.contains('%') && !p.contains('_') { return h == p; }
    // Without a regex crate, we approximate %prefix% and prefix% / %suffix.
    let stripped: String = re.chars().filter(|c| !matches!(c, '^' | '$' | '\\')).collect();
    if let Some(rest) = stripped.strip_prefix(".*") {
        let rest = rest.strip_suffix(".*").unwrap_or(rest);
        h.contains(rest)
    } else if let Some(prefix) = stripped.strip_suffix(".*") {
        h.starts_with(prefix)
    } else {
        h == stripped
    }
}

// ===========================================================================
// Fixtures
// ===========================================================================

// ---------------------------------------------------------------------------
// SAP signature constants — sourced from SAP API Hub / DDIC.
// Every write BAPI carries `commit_required: true` because the standard SAP
// convention is that BAPIs do NOT commit on their own; the caller must
// follow up with BAPI_TRANSACTION_COMMIT to persist (paper §VII-F note;
// confirmed in SAP Help SE37 documentation).
// ---------------------------------------------------------------------------

fn p_imp(name: &str, ty: &str, opt: bool, desc: &str) -> RfcParameter {
    RfcParameter { name: name.into(), direction: RfcParamDirection::Import, type_token: ty.into(), optional: opt, description: if desc.is_empty() { None } else { Some(desc.into()) }, default_value: None }
}
fn p_exp(name: &str, ty: &str, opt: bool, desc: &str) -> RfcParameter {
    RfcParameter { name: name.into(), direction: RfcParamDirection::Export, type_token: ty.into(), optional: opt, description: if desc.is_empty() { None } else { Some(desc.into()) }, default_value: None }
}
fn p_tab(name: &str, ty: &str, opt: bool, desc: &str) -> RfcParameter {
    RfcParameter { name: name.into(), direction: RfcParamDirection::Tables, type_token: ty.into(), optional: opt, description: if desc.is_empty() { None } else { Some(desc.into()) }, default_value: None }
}
fn p_imp_default(name: &str, ty: &str, default: &str, desc: &str) -> RfcParameter {
    RfcParameter { name: name.into(), direction: RfcParamDirection::Import, type_token: ty.into(), optional: true, description: if desc.is_empty() { None } else { Some(desc.into()) }, default_value: Some(default.into()) }
}

fn seed_functions() -> Vec<RfcFunctionMeta> {
    vec![
        // ---- System / diagnostics ----------------------------------------
        RfcFunctionMeta {
            function: "RFC_SYSTEM_INFO".into(),
            description: "Retrieve system identity (SID, client, release, host).".into(),
            function_group: "SUTL".into(),
            package: Some("SCSR".into()),
            parameters: vec![
                p_exp("RFCSI_EXPORT", "STRUCT(RFCSI)", false, "System identity structure (RFCSI)"),
            ],
            deprecated: false, read_only: true, commit_required: false,
            authorization: vec![S_RfcAuth::execute_group("SUTL")],
            s4hana_notes: None,
        },
        // ---- Material Master --------------------------------------------
        RfcFunctionMeta {
            function: "BAPI_MATERIAL_GET_DETAIL".into(),
            description: "Read material master detail.  Read-only; ECC and S/4HANA signatures identical.".into(),
            function_group: "MGV3".into(),
            package: Some("MGV3".into()),
            parameters: vec![
                p_imp("MATERIAL", "MATNR", false, "Material number (CHAR 40 in S/4HANA, was CHAR 18 in ECC ≤ 7.50)"),
                p_imp("PLANT", "WERKS_D", true, "Plant (for plant-specific view)"),
                p_imp("VALUATIONAREA", "BWKEY", true, "Valuation area"),
                p_imp("MATERIALEVG", "STRUCT(BAPIMATEVG)", true, "EAN / GTIN-only view filter"),
                p_exp("MATERIAL_GENERAL_DATA", "STRUCT(BAPIMATDOA)", false, "General data view (MARA)"),
                p_exp("RETURN", "STRUCT(BAPIRET2)", false, "Standard return structure"),
                p_exp("MATERIALPLANTDATA", "STRUCT(BAPIE1MARCRT)", true, "Plant view (MARC)"),
                p_exp("MATERIALVALUATIONDATA", "STRUCT(BAPIE1MBEWRT)", true, "Valuation view (MBEW)"),
            ],
            deprecated: false, read_only: true, commit_required: false,
            authorization: vec![S_RfcAuth::execute_group("MGV3")],
            s4hana_notes: Some("Material number length increased to CHAR(40) in S/4HANA per the MATN1 → MATN9 conversion exit change.".into()),
        },
        // ---- FI: Post journal entry --------------------------------------
        RfcFunctionMeta {
            function: "BAPI_ACC_DOCUMENT_POST".into(),
            description: "Post an accounting document (FI journal entry).  Does NOT auto-commit; caller must invoke BAPI_TRANSACTION_COMMIT.".into(),
            function_group: "ACC4".into(),
            package: Some("FBAS".into()),
            parameters: vec![
                p_imp("DOCUMENTHEADER",  "STRUCT(BAPIACHE09)", false, "Document header"),
                p_imp("CUSTOMERCPD",     "STRUCT(BAPIACPCA09)", true,  "One-time customer header"),
                p_imp("CONTRACTHEADER",  "STRUCT(BAPIACCAHD)",  true,  "Contract header (FI-CA)"),
                p_exp("OBJ_TYPE", "AWTYP", false, "Reference object type"),
                p_exp("OBJ_KEY",  "AWKEY", false, "Reference object key"),
                p_exp("OBJ_SYS",  "AWSYS", false, "Logical system of object"),
                p_tab("ACCOUNTGL",          "STRUCT(BAPIACGL09)",  true, "G/L account items"),
                p_tab("ACCOUNTRECEIVABLE",  "STRUCT(BAPIACAR09)",  true, "Customer (AR) items"),
                p_tab("ACCOUNTPAYABLE",     "STRUCT(BAPIACAP09)",  true, "Vendor (AP) items"),
                p_tab("ACCOUNTTAX",         "STRUCT(BAPIACTX09)",  true, "Tax items"),
                p_tab("CURRENCYAMOUNT",     "STRUCT(BAPIACCR09)",  false, "Currency amounts per item"),
                p_tab("CRITERIA",           "STRUCT(BAPIACKEC9)",  true,  "CO-PA characteristics"),
                p_tab("VALUEFIELD",         "STRUCT(BAPIACKEV9)",  true,  "CO-PA value fields"),
                p_tab("EXTENSION1",         "STRUCT(BAPIPAREX)",   true,  "Customer extension"),
                p_tab("RETURN",             "STRUCT(BAPIRET2)",    false, "Return messages"),
                p_tab("PAYMENTCARD",        "STRUCT(BAPIACPC09)",  true,  "Payment card data"),
                p_tab("REALESTATE",         "STRUCT(BAPIACRE09)",  true,  "Real estate items (FI-RE)"),
                p_tab("ACCOUNTWT",          "STRUCT(BAPIACWT09)",  true,  "Withholding tax items"),
                p_tab("EXTENSION2",         "STRUCT(BAPIPAREXC)",  true,  "Customer extension v2"),
            ],
            deprecated: false, read_only: false, commit_required: true,
            authorization: vec![S_RfcAuth::execute_group("ACC4")],
            s4hana_notes: Some("In S/4HANA the data lands in ACDOCA (Universal Journal). BSEG/BKPF/FAGLFLEXA remain queryable as compatibility views. ACDOCA carries the granular cost/profitability accounting too — no separate CO-PA database.".into()),
        },
        // ---- FI: Commit ---------------------------------------------------
        RfcFunctionMeta {
            function: "BAPI_TRANSACTION_COMMIT".into(),
            description: "Persist the current LUW.  Call this after any state-changing BAPI to actually commit the changes.".into(),
            function_group: "SBPT".into(),
            package: Some("SBPT".into()),
            parameters: vec![
                p_imp_default("WAIT", "CHAR(1)", " ", "If 'X', commit synchronously and wait for completion."),
                p_exp("RETURN", "STRUCT(BAPIRET2)", false, "Standard return"),
            ],
            deprecated: false, read_only: false, commit_required: false,
            authorization: vec![S_RfcAuth::execute_group("SBPT")],
            s4hana_notes: None,
        },
        // ---- FI: Rollback ------------------------------------------------
        RfcFunctionMeta {
            function: "BAPI_TRANSACTION_ROLLBACK".into(),
            description: "Roll back the current LUW.  Call after any failed BAPI to discard partial changes.".into(),
            function_group: "SBPT".into(),
            package: Some("SBPT".into()),
            parameters: vec![
                p_exp("RETURN", "STRUCT(BAPIRET2)", false, ""),
            ],
            deprecated: false, read_only: false, commit_required: false,
            authorization: vec![S_RfcAuth::execute_group("SBPT")],
            s4hana_notes: None,
        },
        // ---- MM: Purchase Order ------------------------------------------
        RfcFunctionMeta {
            function: "BAPI_PO_CREATE1".into(),
            description: "Create a purchase order.  Does NOT auto-commit.".into(),
            function_group: "2012".into(),
            package: Some("MEBAPI".into()),
            parameters: vec![
                p_imp("POHEADER", "STRUCT(BAPIMEPOHEADER)", false, "Purchase order header"),
                p_imp("POHEADERX", "STRUCT(BAPIMEPOHEADERX)", false, "Change indicators for header"),
                p_imp_default("TESTRUN", "CHAR(1)", " ", "If 'X', simulate without DB writes"),
                p_exp("EXPHEADER",  "STRUCT(BAPIMEPOHEADER)", false, ""),
                p_exp("EXPPOEXPIMPHEADER", "STRUCT(BAPIMEPOEXPIMPHEADER)", true, "Export/import header"),
                p_exp("EXPPURCHASEORDER", "EBELN", false, "Resulting PO number"),
                p_tab("POITEM",   "STRUCT(BAPIMEPOITEM)",   false, "Purchase order items"),
                p_tab("POITEMX",  "STRUCT(BAPIMEPOITEMX)",  false, "Change indicators for items"),
                p_tab("POSCHEDULE", "STRUCT(BAPIMEPOSCHEDULE)", true, "Schedule lines"),
                p_tab("POSCHEDULEX", "STRUCT(BAPIMEPOSCHEDULX)", true, ""),
                p_tab("POACCOUNT", "STRUCT(BAPIMEPOACCOUNT)", true, "Account assignment"),
                p_tab("POACCOUNTX", "STRUCT(BAPIMEPOACCOUNTX)", true, ""),
                p_tab("POSERVICES", "STRUCT(BAPIESLLC)", true, "Services lines"),
                p_tab("RETURN", "STRUCT(BAPIRET2)", false, ""),
            ],
            deprecated: false, read_only: false, commit_required: true,
            authorization: vec![S_RfcAuth::execute_group("2012")],
            s4hana_notes: Some("Cost-centre validation is stricter in S/4HANA (CKM3N replaced by ML simplification); the BAPI rejects postings to closed periods (T001B) earlier in the flow.".into()),
        },
        // ---- SD: Sales Order ---------------------------------------------
        RfcFunctionMeta {
            function: "BAPI_SALESORDER_CREATEFROMDAT2".into(),
            description: "Create a sales order from external data.  Does NOT auto-commit.".into(),
            function_group: "2032".into(),
            package: Some("V_BAPI".into()),
            parameters: vec![
                p_imp("ORDER_HEADER_IN",  "STRUCT(BAPISDHD1)", false, "Header data"),
                p_imp("ORDER_HEADER_INX", "STRUCT(BAPISDHD1X)", true, "Change indicators for header"),
                p_imp_default("TESTRUN", "CHAR(1)", " ", "Simulate without DB writes"),
                p_imp_default("CONVERT", "CHAR(1)", " ", "Convert business partner numbers"),
                p_exp("SALESDOCUMENT", "VBELN", false, "Resulting sales document"),
                p_tab("ORDER_ITEMS_IN",   "STRUCT(BAPISDITM)",  false, "Item data"),
                p_tab("ORDER_ITEMS_INX",  "STRUCT(BAPISDITMX)", true,  ""),
                p_tab("ORDER_PARTNERS",   "STRUCT(BAPIPARNR)",  true,  "Partner functions"),
                p_tab("ORDER_SCHEDULES_IN", "STRUCT(BAPISCHDL)", true, "Schedule lines"),
                p_tab("ORDER_CONDITIONS_IN", "STRUCT(BAPICOND)", true, "Conditions (pricing)"),
                p_tab("ORDER_TEXT", "STRUCT(BAPISDTEXT)", true, "Text"),
                p_tab("EXTENSIONIN", "STRUCT(BAPIPAREX)", true, ""),
                p_tab("RETURN", "STRUCT(BAPIRET2)", false, ""),
            ],
            deprecated: false, read_only: false, commit_required: true,
            authorization: vec![S_RfcAuth::execute_group("2032")],
            s4hana_notes: Some("Customer (KUNNR) → Business Partner (BUT000) unification: in S/4HANA the sold-to party in ORDER_PARTNERS is the BP role FLCU01.".into()),
        },
        // ---- SD: Customer master change ----------------------------------
        RfcFunctionMeta {
            function: "BAPI_CUSTOMER_CHANGEFROMDATA1".into(),
            description: "Change customer master data.  Does NOT auto-commit.".into(),
            function_group: "DEBI".into(),
            package: Some("VS".into()),
            parameters: vec![
                p_imp("CUSTOMERNO",        "KUNNR", false, "Customer (KUNNR)"),
                p_imp("PI_CUSTOMERHEADER", "STRUCT(BAPIKNA101_HEAD)", true, "General data (KNA1)"),
                p_imp("PI_CUSTOMERCOMPANY","STRUCT(BAPIKNB101)", true, "Company-code data (KNB1)"),
                p_imp("PI_CUSTOMERSALES",  "STRUCT(BAPIKNVV01)", true, "Sales-area data (KNVV)"),
                p_imp("PI_COPYREFERENCE",  "STRUCT(BAPIKNA110)", true, "Copy reference customer"),
                p_tab("PIT_BANKDETAILS", "STRUCT(BAPIKNBK01)", true, "Bank details (KNBK)"),
                p_tab("RETURN", "STRUCT(BAPIRET2)", false, ""),
            ],
            deprecated: false, read_only: false, commit_required: true,
            authorization: vec![S_RfcAuth::execute_group("DEBI")],
            s4hana_notes: Some("In S/4HANA, customers are part of Business Partner.  This BAPI still works (KNA1 is a compatibility view) but the recommended path is the BP BAPI surface (CVI_*).".into()),
        },
        // ---- Basis: Transport release ------------------------------------
        RfcFunctionMeta {
            function: "TMS_MGR_FORWARD_TR_REQUEST".into(),
            description: "Forward a transport request to a target SAP system via TMS.".into(),
            function_group: "STMS_QA".into(),
            package: Some("STMS".into()),
            parameters: vec![
                p_imp("IV_TARGET_SYSTEM", "TMSSYSNAM", false, "Target system (e.g. QAA, PRD)"),
                p_imp("IV_REQUEST",       "TRKORR", false, "Transport request ID (TRKORR)"),
                p_imp_default("IV_LANGU", "LANGU", "EN", "Language"),
                p_imp_default("IV_TEST_IMPORT", "CHAR(1)", " ", "Test import without commit"),
                p_exp("EV_RC", "INT4", false, "Return code (0 = ok)"),
                p_tab("ET_MSG", "STRUCT(STMSMESS)", true, "TMS messages"),
            ],
            deprecated: false, read_only: false, commit_required: false, // TMS commits internally
            authorization: vec![
                S_RfcAuth::execute_group("STMS_QA"),
                S_RfcAuth { object: "S_CTS_ADMI".into(), rfc_type: "CTS_ADMFCT".into(), rfc_name: "TABL".into(), actvt: "*".into() },
            ],
            s4hana_notes: Some("Transport request shape is unchanged in S/4HANA.  E070/E071 still authoritative.".into()),
        },
        // ---- DDIC: Generic table read ------------------------------------
        RfcFunctionMeta {
            function: "RFC_READ_TABLE".into(),
            description: "Generic transparent-table read with field projection + WHERE clause.  Subject to T999 row-length cap (512 bytes per row).".into(),
            function_group: "SDTX".into(),
            package: Some("SDTX".into()),
            parameters: vec![
                p_imp("QUERY_TABLE", "DDOBJNAME", false, "Table name (DDIC)"),
                p_imp_default("DELIMITER", "CHAR(1)", " ", "Field delimiter for DATA rows"),
                p_imp_default("NO_DATA", "CHAR(1)", " ", "If 'X', return only field metadata, no rows"),
                p_imp_default("ROWSKIPS", "INT4", "0", "Skip n rows (paging)"),
                p_imp_default("ROWCOUNT", "INT4", "0", "Max rows; 0 = unlimited (subject to memory)"),
                p_tab("OPTIONS", "STRUCT(RFC_DB_OPT)", true, "WHERE clauses (each row = one fragment ≤ 72 chars)"),
                p_tab("FIELDS",  "STRUCT(RFC_DB_FLD)", true, "Field projection"),
                p_tab("DATA",    "STRUCT(TAB512)",     false, "Returned rows (delimited)"),
            ],
            deprecated: false, read_only: true, commit_required: false,
            authorization: vec![
                S_RfcAuth::execute_group("SDTX"),
                S_RfcAuth { object: "S_TABU_DIS".into(), rfc_type: "DICBERCLS".into(), rfc_name: "*".into(), actvt: "03".into() },
            ],
            s4hana_notes: Some("In S/4HANA, RFC_READ_TABLE returns data from the underlying ACDOCA when the queried table (BSEG, FAGLFLEXA, COEP, etc.) is a compatibility view.  Per-row 512-byte cap still applies.".into()),
        },
        // ---- DDIC: Field info --------------------------------------------
        RfcFunctionMeta {
            function: "DDIF_FIELDINFO_GET".into(),
            description: "Retrieve DDIC field metadata for a table or structure.".into(),
            function_group: "SDIC".into(),
            package: Some("SDIC".into()),
            parameters: vec![
                p_imp("TABNAME",  "DDOBJNAME", false, "Table or structure name"),
                p_imp("FIELDNAME","DFIES-FIELDNAME", true, "Restrict to one field"),
                p_imp_default("LANGU", "LANGU", "EN", "Description language"),
                p_imp("LFIELDNAME", "DFIES-LFIELDNAME", true, "Long-text restriction"),
                p_imp_default("ALL_TYPES", "CHAR(1)", " ", "Include reference + table types"),
                p_imp_default("GROUP_NAMES", "CHAR(1)", " ", "Include group field names"),
                p_exp("X030L_WA", "STRUCT(X030L)", false, "Header metadata"),
                p_exp("DDOBJTYPE", "DDOBJTYP", false, "Object type (TABL / STRU / VIEW / ...)"),
                p_exp("DFIES_WA", "STRUCT(DFIES)", true, "Single-field metadata"),
                p_exp("LINES_DESCR", "STRUCT(DDFIELDDESCR)", true, ""),
                p_tab("DFIES_TAB", "STRUCT(DFIES)", false, "Field metadata rows"),
                p_tab("FIXED_VALUES", "STRUCT(DDFIXVALUES)", true, "Domain fixed values"),
            ],
            deprecated: false, read_only: true, commit_required: false,
            authorization: vec![S_RfcAuth::execute_group("SDIC")],
            s4hana_notes: None,
        },
    ]
}

fn tf_key(name: &str, data_element: &str, ty: &str, length: u32, desc: &str) -> TableField {
    TableField {
        name: name.into(), data_element: data_element.into(),
        type_token: ty.into(), length, description: Some(desc.into()), key: true,
    }
}
fn tf(name: &str, data_element: &str, ty: &str, length: u32, desc: &str) -> TableField {
    TableField {
        name: name.into(), data_element: data_element.into(),
        type_token: ty.into(), length, description: Some(desc.into()), key: false,
    }
}

fn seed_tables() -> Vec<MockTable> {
    vec![
        // ---- MARA — Material master, general data -----------------------
        MockTable {
            structure: TableStructure {
                table: "MARA".into(),
                description: "General Material Data".into(),
                key_fields: vec!["MANDT".into(), "MATNR".into()],
                fields: vec![
                    tf_key("MANDT", "MANDT", "CLNT", 3, "Client (SAP system tenant)"),
                    tf_key("MATNR", "MATNR", "CHAR", 40, "Material number — CHAR(40) in S/4HANA, CHAR(18) in ECC ≤ 7.50"),
                    tf("ERSDA", "ERSDA", "DATS", 8, "Created on (YYYYMMDD)"),
                    tf("ERNAM", "ERNAM", "CHAR", 12, "Created by (user)"),
                    tf("MTART", "MTART", "CHAR", 4, "Material type (ROH, FERT, HAWA, ...)"),
                    tf("MBRSH", "MBRSH", "CHAR", 1, "Industry sector"),
                    tf("MATKL", "MATKL", "CHAR", 9, "Material group"),
                    tf("MEINS", "MEINS", "UNIT", 3, "Base unit of measure (T006)"),
                    tf("BISMT", "BISMT", "CHAR", 18, "Old material number (ECC backwards)"),
                ],
                authorization_group: "MA".into(),
                s4hana_storage: None,
            },
            rows: vec![
                row(&[("MANDT","100"),("MATNR","FIN-RAW-001"),("MTART","ROH"),("MEINS","KG"),("MBRSH","M"),("ERSDA","20240901"),("ERNAM","SAP_DEV")]),
                row(&[("MANDT","100"),("MATNR","FIN-FERT-77"),("MTART","FERT"),("MEINS","PC"),("MBRSH","M"),("ERSDA","20240915"),("ERNAM","SAP_DEV")]),
                row(&[("MANDT","100"),("MATNR","TRADE-HAWA-12"),("MTART","HAWA"),("MEINS","PC"),("MBRSH","H"),("ERSDA","20251001"),("ERNAM","SAP_DEV")]),
            ],
        },
        // ---- T001 — Company codes ---------------------------------------
        MockTable {
            structure: TableStructure {
                table: "T001".into(),
                description: "Company Codes".into(),
                key_fields: vec!["MANDT".into(), "BUKRS".into()],
                fields: vec![
                    tf_key("MANDT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("BUKRS", "BUKRS", "CHAR", 4, "Company code"),
                    tf("BUTXT", "BUTXT", "CHAR", 25, "Company code name"),
                    tf("ORT01", "ORT01", "CHAR", 25, "City"),
                    tf("LAND1", "LAND1", "CHAR", 3, "Country key (ISO 3166-1 alpha-3 mapping in T005)"),
                    tf("WAERS", "WAERS", "CUKY", 5, "Local currency"),
                    tf("SPRAS", "SPRAS", "LANG", 1, "Language"),
                    tf("KTOPL", "KTOPL", "CHAR", 4, "Chart of accounts"),
                    tf("PERIV", "PERIV", "CHAR", 2, "Fiscal year variant"),
                ],
                authorization_group: "FC".into(),
                s4hana_storage: None,
            },
            rows: vec![
                row(&[("MANDT","100"),("BUKRS","1000"),("BUTXT","Acme Global HQ"),("ORT01","New York"),("LAND1","USA"),("WAERS","USD"),("SPRAS","E"),("KTOPL","CAUS"),("PERIV","K4")]),
                row(&[("MANDT","100"),("BUKRS","2000"),("BUTXT","Acme EMEA"),("ORT01","Berlin"),("LAND1","DEU"),("WAERS","EUR"),("SPRAS","D"),("KTOPL","CADE"),("PERIV","K4")]),
                row(&[("MANDT","100"),("BUKRS","3000"),("BUTXT","Acme APAC"),("ORT01","Singapore"),("LAND1","SGP"),("WAERS","SGD"),("SPRAS","E"),("KTOPL","CASG"),("PERIV","K4")]),
            ],
        },
        // ---- T001B — Posting period variants ----------------------------
        MockTable {
            structure: TableStructure {
                table: "T001B".into(),
                description: "Posting periods: permitted intervals per variant".into(),
                key_fields: vec!["MANDT".into(), "RRCTY".into(), "BUKRS".into(), "MKOAR".into(), "BKONT".into()],
                fields: vec![
                    tf_key("MANDT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("RRCTY", "RRCTY", "CHAR", 1, "Record type"),
                    tf_key("BUKRS", "BUKRS", "CHAR", 4, "Company code (or PERSL = period variant)"),
                    tf_key("MKOAR", "KOART", "CHAR", 1, "Account type (+/A/D/K/M/S)"),
                    tf_key("BKONT", "BKONT", "CHAR", 10, "Account-from"),
                    tf("BUKON", "BUKON", "CHAR", 10, "Account-to"),
                    tf("FRPE1", "FRPE1_BKPF", "NUMC", 3, "From period 1"),
                    tf("FRYE1", "FRYE1_BKPF", "NUMC", 4, "From fiscal year 1"),
                    tf("TOPE1", "TOPE1_BKPF", "NUMC", 3, "To period 1"),
                    tf("TOYE1", "TOYE1_BKPF", "NUMC", 4, "To fiscal year 1"),
                ],
                authorization_group: "FC".into(),
                s4hana_storage: None,
            },
            rows: vec![
                row(&[("MANDT","100"),("RRCTY","0"),("BUKRS","1000"),("MKOAR","+"),("BKONT","0"),("BUKON","ZZZZZZZZZZ"),("FRPE1","003"),("FRYE1","2026"),("TOPE1","003"),("TOYE1","2026")]),
                row(&[("MANDT","100"),("RRCTY","0"),("BUKRS","2000"),("MKOAR","+"),("BKONT","0"),("BUKON","ZZZZZZZZZZ"),("FRPE1","003"),("FRYE1","2026"),("TOPE1","003"),("TOYE1","2026")]),
            ],
        },
        // ---- BSEG — Document segment (compatibility view in S/4HANA) ---
        MockTable {
            structure: TableStructure {
                table: "BSEG".into(),
                description: "Accounting document segment".into(),
                key_fields: vec!["MANDT".into(), "BUKRS".into(), "BELNR".into(), "GJAHR".into(), "BUZEI".into()],
                fields: vec![
                    tf_key("MANDT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("BUKRS", "BUKRS", "CHAR", 4, "Company code"),
                    tf_key("BELNR", "BELNR_D", "CHAR", 10, "Accounting document number"),
                    tf_key("GJAHR", "GJAHR", "NUMC", 4, "Fiscal year"),
                    tf_key("BUZEI", "BUZEI", "NUMC", 3, "Line item"),
                    tf("HKONT", "HKONT", "CHAR", 10, "G/L account"),
                    tf("KOART", "KOART", "CHAR", 1, "Account type (+/A/D/K/M/S)"),
                    tf("SHKZG", "SHKZG", "CHAR", 1, "Debit/credit indicator (S/H)"),
                    tf("DMBTR", "DMBTR", "CURR", 13, "Amount in local currency"),
                    tf("WRBTR", "WRBTR", "CURR", 13, "Amount in document currency"),
                    tf("WAERS", "WAERS", "CUKY", 5, "Document currency"),
                    tf("KOSTL", "KOSTL", "CHAR", 10, "Cost centre"),
                ],
                authorization_group: "FC".into(),
                s4hana_storage: Some("S/4HANA compatibility view: actual storage is ACDOCA (Universal Journal). Schema unchanged for backward compatibility.".into()),
            },
            rows: vec![
                row(&[("MANDT","100"),("BUKRS","1000"),("BELNR","0100000123"),("GJAHR","2026"),("BUZEI","001"),("HKONT","0000400000"),("KOART","S"),("SHKZG","S"),("DMBTR","1500.00"),("WAERS","USD"),("KOSTL","CC-FIN-100")]),
                row(&[("MANDT","100"),("BUKRS","1000"),("BELNR","0100000123"),("GJAHR","2026"),("BUZEI","002"),("HKONT","0000113100"),("KOART","S"),("SHKZG","H"),("DMBTR","1500.00"),("WAERS","USD"),("KOSTL","")]),
            ],
        },
        // ---- FAGLFLEXA — New G/L line items (compatibility in S/4HANA) -
        MockTable {
            structure: TableStructure {
                table: "FAGLFLEXA".into(),
                description: "General Ledger: actual line items".into(),
                key_fields: vec!["RCLNT".into(), "RLDNR".into(), "RRCTY".into(), "RVERS".into(), "DOCNR".into(), "DOCLN".into(), "RYEAR".into()],
                fields: vec![
                    tf_key("RCLNT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("RLDNR", "RLDNR", "CHAR", 2, "Ledger"),
                    tf_key("RRCTY", "RRCTY", "CHAR", 1, "Record type"),
                    tf_key("RVERS", "RVERS", "CHAR", 3, "Version"),
                    tf_key("RYEAR", "GJAHR", "NUMC", 4, "Fiscal year"),
                    tf_key("DOCNR", "BELNR_D", "CHAR", 10, "Document number"),
                    tf_key("DOCLN", "DOCLN6", "NUMC", 6, "Line item (6-digit)"),
                    tf("RBUKRS", "BUKRS", "CHAR", 4, "Company code"),
                    tf("RACCT", "RACCT", "CHAR", 10, "Account number"),
                    tf("HSL", "HSL", "CURR", 23, "Amount in local currency"),
                    tf("KSL", "KSL", "CURR", 23, "Amount in group currency"),
                ],
                authorization_group: "FC".into(),
                s4hana_storage: Some("S/4HANA compatibility view: actual storage is ACDOCA. Existing reports continue to work; new reports should query ACDOCA directly.".into()),
            },
            rows: vec![],
        },
        // ---- ACDOCA — Universal Journal (S/4HANA primary table) --------
        MockTable {
            structure: TableStructure {
                table: "ACDOCA".into(),
                description: "S/4HANA Universal Journal: single primary table for FI, CO, COPA, ML, FA, etc.".into(),
                key_fields: vec!["RCLNT".into(), "RLDNR".into(), "RBUKRS".into(), "GJAHR".into(), "BELNR".into(), "DOCLN".into()],
                fields: vec![
                    tf_key("RCLNT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("RLDNR", "RLDNR", "CHAR", 2, "Ledger"),
                    tf_key("RBUKRS", "BUKRS", "CHAR", 4, "Company code"),
                    tf_key("GJAHR", "GJAHR", "NUMC", 4, "Fiscal year"),
                    tf_key("BELNR", "BELNR_D", "CHAR", 10, "Document number"),
                    tf_key("DOCLN", "DOCLN6", "NUMC", 6, "Line item"),
                    tf("RACCT", "RACCT", "CHAR", 10, "G/L account"),
                    tf("KOSTL", "KOSTL", "CHAR", 10, "Cost centre"),
                    tf("PRCTR", "PRCTR", "CHAR", 10, "Profit centre"),
                    tf("HSL", "HSL", "CURR", 23, "Amount in local currency"),
                    tf("WSL", "WSL", "CURR", 23, "Amount in document currency"),
                    tf("RUNIT", "MEINS", "UNIT", 3, "Base unit of measure"),
                    tf("BUDAT", "BUDAT", "DATS", 8, "Posting date"),
                ],
                authorization_group: "FC".into(),
                s4hana_storage: Some("Universal Journal (ACDOCA): primary FI / CO line-item store in S/4HANA. Replaces BSEG, FAGLFLEXA, COEP, COSP, COSS, MLIT, MLPP, MLCD, ANEP, ANEK, ANLP at the storage layer; those remain queryable as compatibility views.".into()),
            },
            rows: vec![
                row(&[("RCLNT","100"),("RLDNR","0L"),("RBUKRS","1000"),("GJAHR","2026"),("BELNR","0100000123"),("DOCLN","000001"),("RACCT","0000400000"),("KOSTL","CC-FIN-100"),("PRCTR","PC-1000-FIN"),("HSL","1500.00"),("WSL","1500.00"),("BUDAT","20260315")]),
            ],
        },
        // ---- VBAK — Sales document header ------------------------------
        MockTable {
            structure: TableStructure {
                table: "VBAK".into(),
                description: "Sales Document: Header Data".into(),
                key_fields: vec!["MANDT".into(), "VBELN".into()],
                fields: vec![
                    tf_key("MANDT", "MANDT", "CLNT", 3, "Client"),
                    tf_key("VBELN", "VBELN_VA", "CHAR", 10, "Sales document"),
                    tf("ERDAT", "ERDAT", "DATS", 8, "Created on"),
                    tf("ERZET", "ERZET", "TIMS", 6, "Created at"),
                    tf("ERNAM", "ERNAM", "CHAR", 12, "Created by"),
                    tf("AUART", "AUART", "CHAR", 4, "Sales document type (OR, RE, ...)"),
                    tf("AUGRU", "AUGRU", "CHAR", 3, "Order reason"),
                    tf("KUNNR", "KUNNR", "CHAR", 10, "Sold-to party (in S/4HANA the master is Business Partner)"),
                    tf("VKORG", "VKORG", "CHAR", 4, "Sales organisation"),
                    tf("VTWEG", "VTWEG", "CHAR", 2, "Distribution channel"),
                    tf("NETWR", "NETWR", "CURR", 15, "Net value of order"),
                    tf("WAERK", "WAERK", "CUKY", 5, "Document currency"),
                ],
                authorization_group: "VC".into(),
                s4hana_storage: None,
            },
            rows: vec![
                row(&[("MANDT","100"),("VBELN","0000005001"),("ERDAT","20260112"),("AUART","OR"),("KUNNR","C-100"),("VKORG","1000"),("VTWEG","10"),("NETWR","12500.00"),("WAERK","USD")]),
                row(&[("MANDT","100"),("VBELN","0000005002"),("ERDAT","20260115"),("AUART","OR"),("KUNNR","C-100"),("VKORG","1000"),("VTWEG","10"),("NETWR","8990.00"),("WAERK","USD")]),
                row(&[("MANDT","100"),("VBELN","0000005003"),("ERDAT","20260120"),("AUART","RE"),("KUNNR","C-200"),("VKORG","2000"),("VTWEG","10"),("NETWR","-450.00"),("WAERK","EUR")]),
            ],
        },
        // ---- E070 — Transport request headers --------------------------
        MockTable {
            structure: TableStructure {
                table: "E070".into(),
                description: "Transport request header (TR / task)".into(),
                key_fields: vec!["TRKORR".into()],
                fields: vec![
                    tf_key("TRKORR", "TRKORR", "CHAR", 20, "Transport request"),
                    tf("TRFUNCTION", "TRFUNCTION", "CHAR", 1, "Type: K=customising, W=workbench, T=task, ..."),
                    tf("TRSTATUS", "TRSTATUS", "CHAR", 1, "Status: D=modifiable, L=locked, R=released"),
                    tf("TARSYSTEM", "TARSYSTEM", "CHAR", 10, "Target system"),
                    tf("AS4USER", "AS4USER", "CHAR", 12, "Owner"),
                    tf("AS4DATE", "AS4DATE", "DATS", 8, "Created on"),
                    tf("AS4TIME", "AS4TIME", "TIMS", 6, "Created at"),
                    tf("STRKORR", "STRKORR", "CHAR", 20, "Parent request (for sub-tasks)"),
                ],
                authorization_group: "&NC&".into(),
                s4hana_storage: None,
            },
            rows: vec![
                row(&[("TRKORR","ZTRA01K900123"),("TRFUNCTION","K"),("TRSTATUS","D"),("TARSYSTEM","QAA"),("AS4USER","DEV01"),("AS4DATE","20260318"),("STRKORR","")]),
            ],
        },
    ]
}

fn row(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs {
        m.insert((*k).into(), serde_json::Value::String((*v).into()));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Phase 7 SAP correctness invariants.  These tests enforce the rules
    // that hold across the standard SAP catalogue, so any drift in our
    // fixtures fails CI loudly.
    // -----------------------------------------------------------------

    #[test]
    fn every_write_bapi_has_bapiret2_in_tables() {
        for f in seed_functions() {
            if !f.read_only && !f.function.starts_with("BAPI_TRANSACTION_") && !f.function.starts_with("TMS_") {
                let has_return = f.parameters.iter().any(|p|
                    matches!(p.direction, RfcParamDirection::Tables)
                    && p.name == "RETURN"
                    && p.type_token.contains("BAPIRET2")
                );
                assert!(has_return,
                    "write BAPI {} must declare TABLES RETURN of type BAPIRET2 \
                     (SAP convention; agents need it to inspect error messages)",
                    f.function);
            }
        }
    }

    #[test]
    fn every_write_bapi_requires_commit() {
        // Per SAP standard contract: BAPIs do not auto-commit.  Caller
        // must follow up with BAPI_TRANSACTION_COMMIT.  Exceptions:
        // the commit / rollback BAPIs themselves, and TMS (committed by
        // the queue manager internally).
        for f in seed_functions() {
            if f.read_only { continue; }
            if f.function == "BAPI_TRANSACTION_COMMIT" || f.function == "BAPI_TRANSACTION_ROLLBACK" { continue; }
            if f.function.starts_with("TMS_") { continue; }
            assert!(f.commit_required,
                "write BAPI {} must have commit_required = true \
                 (the standard SAP convention)", f.function);
        }
    }

    #[test]
    fn every_rfc_has_at_least_one_authorization_entry() {
        for f in seed_functions() {
            assert!(!f.authorization.is_empty(),
                "RFC {} has no S_RFC authorization metadata", f.function);
            for a in &f.authorization {
                assert!(a.object == "S_RFC" || a.object.starts_with("S_"),
                    "RFC {} carries malformed authorization object '{}'", f.function, a.object);
            }
        }
    }

    #[test]
    fn every_table_has_client_as_first_key() {
        // SAP convention: every client-dependent table carries a client
        // column of type CLNT(3) as its first key.  The field is named
        // MANDT for classic tables and RCLNT for the new-G/L / Universal
        // Journal tables (FAGLFLEXA, ACDOCA, etc.).  Cross-client
        // tables (E070, E071) are explicit exceptions.
        let cross_client = ["E070", "E071", "T000"];
        for t in seed_tables() {
            let s = &t.structure;
            assert!(!s.fields.is_empty(), "table {} has no fields", s.table);
            if cross_client.contains(&s.table.as_str()) { continue; }
            let first = &s.fields[0];
            assert!(first.name == "MANDT" || first.name == "RCLNT",
                "table {} first field is {}, expected MANDT or RCLNT", s.table, first.name);
            assert_eq!(first.type_token, "CLNT",
                "table {} client field has type {}, expected CLNT", s.table, first.type_token);
            assert_eq!(first.length, 3,
                "table {} client field has length {}, expected 3", s.table, first.length);
            assert!(first.key, "table {} client field is not flagged as key", s.table);
            assert!(s.key_fields.first().map(|k| k == "MANDT" || k == "RCLNT").unwrap_or(false),
                "table {} key_fields does not start with MANDT or RCLNT", s.table);
        }
    }

    #[test]
    fn material_number_is_char_40_per_s4hana() {
        // The single most-cited DDIC change in S/4HANA: MATNR is
        // CHAR(40), not CHAR(18).  This regression test catches drift
        // from the standard S4 length.
        let mara = seed_tables().into_iter().find(|t| t.structure.table == "MARA").unwrap();
        let matnr = mara.structure.fields.iter().find(|f| f.name == "MATNR").unwrap();
        assert_eq!(matnr.length, 40,
            "MATNR length is {} in our fixture; S/4HANA uses CHAR(40)",
            matnr.length);
    }

    #[test]
    fn acdoca_is_present_and_marked_as_universal_journal() {
        let t = seed_tables().into_iter()
            .find(|t| t.structure.table == "ACDOCA")
            .expect("ACDOCA missing — Universal Journal is required for S/4HANA correctness");
        assert!(t.structure.s4hana_storage.as_deref().unwrap_or("").to_lowercase().contains("universal"));
    }

    #[test]
    fn compatibility_views_carry_s4hana_storage_note() {
        // BSEG and FAGLFLEXA are compatibility views in S/4HANA.  Both
        // must carry an s4hana_storage note pointing to ACDOCA, so
        // agents reading the metadata know where data really lives.
        for table_name in &["BSEG", "FAGLFLEXA"] {
            let t = seed_tables().into_iter()
                .find(|t| &t.structure.table == table_name)
                .unwrap_or_else(|| panic!("{} fixture missing", table_name));
            let note = t.structure.s4hana_storage.as_deref().unwrap_or("");
            assert!(note.to_lowercase().contains("acdoca"),
                "{} must note that S/4HANA storage is in ACDOCA; got: {note:?}",
                table_name);
        }
    }

    #[tokio::test]
    async fn system_info_returns_identity() {
        let c = MockSapClient::new(4, serde_json::json!({"user": "DEMO"}));
        let info = c.system_info().await.unwrap();
        assert_eq!(info.sid, "S4H");
        assert_eq!(info.client, "100");
    }

    #[tokio::test]
    async fn rfc_search_ranks_by_match() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let r = c.search_rfc("material master", 5).await.unwrap();
        assert!(!r.hits.is_empty());
        assert_eq!(r.hits[0].function, "BAPI_MATERIAL_GET_DETAIL");
    }

    #[tokio::test]
    async fn rfc_metadata_required_param_check() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let req = RfcCallRequest {
            function: "BAPI_MATERIAL_GET_DETAIL".into(),
            parameters: serde_json::json!({}),
            timeout_ms: 5000,
            require_read_only_safe: true,
        };
        let err = c.call_rfc(req, true).await.unwrap_err();
        assert!(matches!(err, RfcError::InvalidParameter { ref name, .. } if name == "MATERIAL"));
    }

    #[tokio::test]
    async fn rfc_call_read_only_mode_blocks_writes() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let req = RfcCallRequest {
            function: "BAPI_SALESORDER_CREATEFROMDAT2".into(),
            parameters: serde_json::json!({
                "ORDER_HEADER_IN": {},
                "ORDER_ITEMS_IN": []
            }),
            timeout_ms: 5000,
            require_read_only_safe: true,
        };
        let err = c.call_rfc(req, true).await.unwrap_err();
        assert!(matches!(err, RfcError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn read_table_filters_and_projects() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let rows = c.read_table(ReadTableRequest {
            table: "T001".into(),
            fields: vec!["BUKRS".into(), "BUTXT".into()],
            where_conditions: vec!["WAERS = 'EUR'".into()],
            max_rows: 10,
        }).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].values.get("BUKRS").unwrap(), "2000");
        assert!(rows[0].values.get("WAERS").is_none(), "field not projected");
    }

    #[tokio::test]
    async fn read_table_buffer_overflow() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let err = c.read_table(ReadTableRequest {
            table: "MARA".into(),
            fields: vec![],
            where_conditions: vec![],
            max_rows: 9999,
        }).await.unwrap_err();
        assert!(matches!(err, RfcError::TableBufferOverflow { .. }));
    }

    #[tokio::test]
    async fn bulk_metadata_reports_missing() {
        let c = MockSapClient::new(4, serde_json::json!({}));
        let r = c.bulk_rfc_metadata(
            &["RFC_SYSTEM_INFO".into(), "DOES_NOT_EXIST".into()],
            "EN",
        ).await.unwrap();
        assert_eq!(r.functions.len(), 1);
        assert_eq!(r.missing, vec!["DOES_NOT_EXIST".to_string()]);
    }
}
