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

fn seed_functions() -> Vec<RfcFunctionMeta> {
    vec![
        RfcFunctionMeta {
            function: "RFC_SYSTEM_INFO".into(),
            description: "Retrieve system identity (SID, client, release).".into(),
            function_group: "SUTL".into(),
            package: Some("SAPRFC".into()),
            parameters: vec![
                RfcParameter { name: "RFCSI_EXPORT".into(), direction: RfcParamDirection::Export, type_token: "STRUCT(RFCSI)".into(), optional: false, description: Some("System identity structure".into()), default_value: None },
            ],
            deprecated: false,
            read_only: true,
        },
        RfcFunctionMeta {
            function: "BAPI_MATERIAL_GET_DETAIL".into(),
            description: "Read material master detail for a given MATNR.".into(),
            function_group: "MGV3".into(),
            package: Some("MM".into()),
            parameters: vec![
                RfcParameter { name: "MATERIAL".into(), direction: RfcParamDirection::Import, type_token: "MATNR".into(), optional: false, description: Some("Material number".into()), default_value: None },
                RfcParameter { name: "PLANT".into(), direction: RfcParamDirection::Import, type_token: "WERKS_D".into(), optional: true, description: Some("Plant".into()), default_value: None },
                RfcParameter { name: "MATERIAL_GENERAL_DATA".into(), direction: RfcParamDirection::Export, type_token: "STRUCT(BAPIMATDOA)".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "RETURN".into(), direction: RfcParamDirection::Export, type_token: "STRUCT(BAPIRET2)".into(), optional: false, description: None, default_value: None },
            ],
            deprecated: false,
            read_only: true,
        },
        RfcFunctionMeta {
            function: "BAPI_ACC_DOCUMENT_POST".into(),
            description: "Post an accounting document (FI journal entry).".into(),
            function_group: "FBAS".into(),
            package: Some("FI".into()),
            parameters: vec![
                RfcParameter { name: "DOCUMENTHEADER".into(), direction: RfcParamDirection::Import, type_token: "STRUCT(BAPIACHE09)".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "OBJ_TYPE".into(), direction: RfcParamDirection::Export, type_token: "AWTYP".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "OBJ_KEY".into(), direction: RfcParamDirection::Export, type_token: "AWKEY".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "RETURN".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(BAPIRET2)".into(), optional: false, description: None, default_value: None },
            ],
            deprecated: false,
            read_only: false, // writes!
        },
        RfcFunctionMeta {
            function: "BAPI_SALESORDER_CREATEFROMDAT2".into(),
            description: "Create a sales order from external data.".into(),
            function_group: "2032".into(),
            package: Some("SD".into()),
            parameters: vec![
                RfcParameter { name: "SALESDOCUMENT".into(), direction: RfcParamDirection::Export, type_token: "VBELN".into(), optional: false, description: Some("Resulting sales order number".into()), default_value: None },
                RfcParameter { name: "ORDER_HEADER_IN".into(), direction: RfcParamDirection::Import, type_token: "STRUCT(BAPISDHD1)".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "ORDER_ITEMS_IN".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(BAPISDITM)".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "RETURN".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(BAPIRET2)".into(), optional: false, description: None, default_value: None },
            ],
            deprecated: false,
            read_only: false,
        },
        RfcFunctionMeta {
            function: "RFC_READ_TABLE".into(),
            description: "Generic table read (DDIC, with field filter and WHERE clause).".into(),
            function_group: "SDTX".into(),
            package: Some("SAPRFC".into()),
            parameters: vec![
                RfcParameter { name: "QUERY_TABLE".into(), direction: RfcParamDirection::Import, type_token: "DDOBJNAME".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "DELIMITER".into(), direction: RfcParamDirection::Import, type_token: "CHAR(1)".into(), optional: true, description: None, default_value: Some(";".into()) },
                RfcParameter { name: "ROWCOUNT".into(), direction: RfcParamDirection::Import, type_token: "INT4".into(), optional: true, description: None, default_value: Some("100".into()) },
                RfcParameter { name: "OPTIONS".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(RFC_DB_OPT)".into(), optional: true, description: Some("WHERE clauses".into()), default_value: None },
                RfcParameter { name: "FIELDS".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(RFC_DB_FLD)".into(), optional: true, description: Some("Field projection".into()), default_value: None },
                RfcParameter { name: "DATA".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(TAB512)".into(), optional: false, description: None, default_value: None },
            ],
            deprecated: false,
            read_only: true,
        },
        RfcFunctionMeta {
            function: "DDIF_FIELDINFO_GET".into(),
            description: "Retrieve DDIC structure information for a table or structure.".into(),
            function_group: "SDIC".into(),
            package: Some("SDIC".into()),
            parameters: vec![
                RfcParameter { name: "TABNAME".into(), direction: RfcParamDirection::Import, type_token: "DDOBJNAME".into(), optional: false, description: None, default_value: None },
                RfcParameter { name: "FIELDNAME".into(), direction: RfcParamDirection::Import, type_token: "DFIES-FIELDNAME".into(), optional: true, description: None, default_value: None },
                RfcParameter { name: "LANGU".into(), direction: RfcParamDirection::Import, type_token: "LANGU".into(), optional: true, description: None, default_value: Some("EN".into()) },
                RfcParameter { name: "DFIES_TAB".into(), direction: RfcParamDirection::Tables, type_token: "STRUCT(DFIES)".into(), optional: false, description: None, default_value: None },
            ],
            deprecated: false,
            read_only: true,
        },
    ]
}

fn seed_tables() -> Vec<MockTable> {
    vec![
        MockTable {
            structure: TableStructure {
                table: "MARA".into(),
                description: "General Material Data".into(),
                key_fields: vec!["MATNR".into()],
                fields: vec![
                    TableField { name: "MATNR".into(), data_element: "MATNR".into(), type_token: "CHAR".into(), length: 40, description: Some("Material number".into()), key: true },
                    TableField { name: "MTART".into(), data_element: "MTART".into(), type_token: "CHAR".into(), length: 4, description: Some("Material type".into()), key: false },
                    TableField { name: "MEINS".into(), data_element: "MEINS".into(), type_token: "UNIT".into(), length: 3, description: Some("Base unit of measure".into()), key: false },
                    TableField { name: "MBRSH".into(), data_element: "MBRSH".into(), type_token: "CHAR".into(), length: 1, description: Some("Industry sector".into()), key: false },
                ],
            },
            rows: vec![
                row(&[("MATNR", "FIN-RAW-001"), ("MTART", "ROH"), ("MEINS", "KG"), ("MBRSH", "M")]),
                row(&[("MATNR", "FIN-FERT-77"), ("MTART", "FERT"), ("MEINS", "PC"), ("MBRSH", "M")]),
                row(&[("MATNR", "TRADE-HAWA-12"), ("MTART", "HAWA"), ("MEINS", "PC"), ("MBRSH", "H")]),
            ],
        },
        MockTable {
            structure: TableStructure {
                table: "T001".into(),
                description: "Company Codes".into(),
                key_fields: vec!["BUKRS".into()],
                fields: vec![
                    TableField { name: "BUKRS".into(), data_element: "BUKRS".into(), type_token: "CHAR".into(), length: 4, description: Some("Company code".into()), key: true },
                    TableField { name: "BUTXT".into(), data_element: "BUTXT".into(), type_token: "CHAR".into(), length: 25, description: Some("Company code name".into()), key: false },
                    TableField { name: "ORT01".into(), data_element: "ORT01".into(), type_token: "CHAR".into(), length: 25, description: Some("City".into()), key: false },
                    TableField { name: "WAERS".into(), data_element: "WAERS".into(), type_token: "CUKY".into(), length: 5, description: Some("Currency".into()), key: false },
                ],
            },
            rows: vec![
                row(&[("BUKRS", "1000"), ("BUTXT", "Acme Global HQ"), ("ORT01", "New York"), ("WAERS", "USD")]),
                row(&[("BUKRS", "2000"), ("BUTXT", "Acme EMEA"), ("ORT01", "Berlin"), ("WAERS", "EUR")]),
                row(&[("BUKRS", "3000"), ("BUTXT", "Acme APAC"), ("ORT01", "Singapore"), ("WAERS", "SGD")]),
            ],
        },
        MockTable {
            structure: TableStructure {
                table: "VBAK".into(),
                description: "Sales Document: Header Data".into(),
                key_fields: vec!["VBELN".into()],
                fields: vec![
                    TableField { name: "VBELN".into(), data_element: "VBELN_VA".into(), type_token: "CHAR".into(), length: 10, description: Some("Sales document".into()), key: true },
                    TableField { name: "ERDAT".into(), data_element: "ERDAT".into(), type_token: "DATS".into(), length: 8, description: Some("Created on".into()), key: false },
                    TableField { name: "AUART".into(), data_element: "AUART".into(), type_token: "CHAR".into(), length: 4, description: Some("Sales document type".into()), key: false },
                    TableField { name: "KUNNR".into(), data_element: "KUNNR".into(), type_token: "CHAR".into(), length: 10, description: Some("Sold-to party".into()), key: false },
                    TableField { name: "NETWR".into(), data_element: "NETWR".into(), type_token: "CURR".into(), length: 15, description: Some("Net value of order".into()), key: false },
                ],
            },
            rows: vec![
                row(&[("VBELN", "0000005001"), ("ERDAT", "20260112"), ("AUART", "OR"), ("KUNNR", "C-100"), ("NETWR", "12500.00")]),
                row(&[("VBELN", "0000005002"), ("ERDAT", "20260115"), ("AUART", "OR"), ("KUNNR", "C-100"), ("NETWR", "8990.00")]),
                row(&[("VBELN", "0000005003"), ("ERDAT", "20260120"), ("AUART", "RE"), ("KUNNR", "C-200"), ("NETWR", "-450.00")]),
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
