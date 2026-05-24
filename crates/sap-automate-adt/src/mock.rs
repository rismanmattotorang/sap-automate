//! Offline mock ADT client.
//!
//! Seeded with realistic FI/MM/SD fixtures:
//!   - Programs: ZFIN_POST_JE, ZMM_GRN_CHECK
//!   - Classes:  ZCL_FIN_POSTER, ZCL_MM_GRN_VALIDATOR
//!   - Interfaces: ZIF_FIN_POSTABLE
//!   - Includes: ZFIN_TOP, ZFIN_F01
//!   - Function modules: Z_FIN_VALIDATE_BUKRS in group ZFIN_UTIL
//!   - CDS views: Z_C_SALES_ORDER_KPI
//!   - Packages: ZFIN, ZMM
//!   - Where-used data wired between the above so impact analysis is
//!     meaningful in demos.

use crate::client::{AdtCallContext, AdtClient};
use crate::destination::AdtDestination;
use crate::error::{AdtError, AdtResult};
use crate::types::{
    AbapObjectKind, ActivationOutcome, ActivationRequest, AdtSearchHit, AdtSearchRequest,
    CdsView, PackageContents, PackageMember, ProgramSource, TableRow, WhereUsedHit,
    WhereUsedRequest, MAX_TABLE_ROWS,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

pub struct MockAdtClient {
    destination: AdtDestination,
    programs: HashMap<String, ProgramSource>,
    classes: HashMap<String, ProgramSource>,
    interfaces: HashMap<String, ProgramSource>,
    includes: HashMap<String, ProgramSource>,
    function_modules: HashMap<(String, String), ProgramSource>,
    cds_views: HashMap<String, CdsView>,
    packages: HashMap<String, PackageContents>,
    where_used: HashMap<(String, AbapObjectKind), Vec<WhereUsedHit>>,
    tables: HashMap<String, Vec<TableRow>>,
}

impl MockAdtClient {
    pub fn new(destination: AdtDestination) -> Arc<Self> {
        let mut s = Self {
            destination,
            programs: HashMap::new(),
            classes: HashMap::new(),
            interfaces: HashMap::new(),
            includes: HashMap::new(),
            function_modules: HashMap::new(),
            cds_views: HashMap::new(),
            packages: HashMap::new(),
            where_used: HashMap::new(),
            tables: HashMap::new(),
        };
        s.seed();
        Arc::new(s)
    }

    fn seed(&mut self) {
        // Programs
        self.programs.insert("ZFIN_POST_JE".into(), prog(
            "ZFIN_POST_JE", AbapObjectKind::Program, "ZFIN", "Post FI journal entries",
            "REPORT zfin_post_je.\n\nINCLUDE zfin_top.\nINCLUDE zfin_f01.\n\nSTART-OF-SELECTION.\n  PERFORM validate.\n  CALL FUNCTION 'BAPI_ACC_DOCUMENT_POST'\n    EXPORTING documentheader = ls_header.\n  CALL FUNCTION 'BAPI_TRANSACTION_COMMIT'.\n",
        ));
        self.programs.insert("ZMM_GRN_CHECK".into(), prog(
            "ZMM_GRN_CHECK", AbapObjectKind::Program, "ZMM", "Goods receipt reconciliation",
            "REPORT zmm_grn_check.\n\nSTART-OF-SELECTION.\n  CALL FUNCTION 'Z_MM_GRN_VALIDATE'.\n  CALL FUNCTION 'BAPI_GOODSMVT_CREATE'.\n",
        ));

        // Classes
        self.classes.insert("ZCL_FIN_POSTER".into(), prog(
            "ZCL_FIN_POSTER", AbapObjectKind::Class, "ZFIN", "FI posting helper class",
            "CLASS zcl_fin_poster DEFINITION PUBLIC FINAL.\n  PUBLIC SECTION.\n    INTERFACES zif_fin_postable.\n    METHODS post_document\n      IMPORTING is_header TYPE bapiache09\n      EXPORTING ev_obj_key TYPE awkey.\nENDCLASS.\n\nCLASS zcl_fin_poster IMPLEMENTATION.\n  METHOD zif_fin_postable~validate.\n    \" cost-centre check\n  ENDMETHOD.\n  METHOD post_document.\n    CALL FUNCTION 'BAPI_ACC_DOCUMENT_POST'\n      EXPORTING documentheader = is_header.\n  ENDMETHOD.\nENDCLASS.\n",
        ));
        self.classes.insert("ZCL_MM_GRN_VALIDATOR".into(), prog(
            "ZCL_MM_GRN_VALIDATOR", AbapObjectKind::Class, "ZMM", "Goods receipt validator",
            "CLASS zcl_mm_grn_validator DEFINITION PUBLIC FINAL.\n  PUBLIC SECTION.\n    METHODS validate IMPORTING is_grn TYPE mseg RETURNING VALUE(rv_ok) TYPE abap_bool.\nENDCLASS.\n",
        ));

        // Interfaces
        self.interfaces.insert("ZIF_FIN_POSTABLE".into(), prog(
            "ZIF_FIN_POSTABLE", AbapObjectKind::Interface, "ZFIN", "Postable contract",
            "INTERFACE zif_fin_postable PUBLIC.\n  METHODS validate IMPORTING is_header TYPE bapiache09\n                    RETURNING VALUE(rv_ok) TYPE abap_bool.\nENDINTERFACE.\n",
        ));

        // Includes
        self.includes.insert("ZFIN_TOP".into(), prog(
            "ZFIN_TOP", AbapObjectKind::Include, "ZFIN", "ZFIN top include",
            "* Global data definitions for ZFIN_POST_JE\nTYPES: BEGIN OF ty_line,\n         bukrs TYPE bukrs,\n         hkont TYPE hkont,\n         wrbtr TYPE wrbtr,\n       END OF ty_line.\nDATA: gt_lines TYPE STANDARD TABLE OF ty_line.\n",
        ));
        self.includes.insert("ZFIN_F01".into(), prog(
            "ZFIN_F01", AbapObjectKind::Include, "ZFIN", "ZFIN form routines",
            "FORM validate.\n  IF gt_lines IS INITIAL.\n    MESSAGE 'No lines to post' TYPE 'E'.\n  ENDIF.\nENDFORM.\n",
        ));

        // Function modules
        self.function_modules.insert(("ZFIN_UTIL".into(), "Z_FIN_VALIDATE_BUKRS".into()), prog(
            "Z_FIN_VALIDATE_BUKRS", AbapObjectKind::FunctionModule, "ZFIN", "Validate company code",
            "FUNCTION z_fin_validate_bukrs.\n*\"--------------------------------------------------------------\n*\"*\"Local Interface:\n*\"  IMPORTING\n*\"     VALUE(IV_BUKRS) TYPE  BUKRS\n*\"  EXPORTING\n*\"     VALUE(EV_OK) TYPE  ABAP_BOOL\n*\"--------------------------------------------------------------\n  SELECT SINGLE bukrs FROM t001 INTO @DATA(lv_bukrs) WHERE bukrs = @iv_bukrs.\n  ev_ok = COND #( WHEN sy-subrc = 0 THEN abap_true ELSE abap_false ).\nENDFUNCTION.\n",
        ));

        // CDS views
        self.cds_views.insert("Z_C_SALES_ORDER_KPI".into(), CdsView {
            name: "Z_C_SALES_ORDER_KPI".into(),
            root_entity: "Z_C_SalesOrderKpi".into(),
            annotations: serde_json::json!({
                "AbapCatalog.sqlViewName": "ZSO_KPI",
                "AccessControl.authorizationCheck": "#NOT_REQUIRED",
                "EndUserText.label": "Sales order KPIs"
            }),
            source: "@AbapCatalog.sqlViewName: 'ZSO_KPI'\n@AccessControl.authorizationCheck: #NOT_REQUIRED\n@EndUserText.label: 'Sales order KPIs'\ndefine view Z_C_SalesOrderKpi as select from vbak\n  inner join vbap on vbak.vbeln = vbap.vbeln\n{\n  key vbak.vbeln,\n      vbak.erdat,\n      vbak.auart,\n      vbak.kunnr,\n      sum( vbap.netwr ) as total_net_value\n}\ngroup by vbak.vbeln, vbak.erdat, vbak.auart, vbak.kunnr;\n".into(),
            line_count: 12,
        });

        // Packages
        self.packages.insert("ZFIN".into(), PackageContents {
            package: "ZFIN".into(),
            description: Some("Finance customisations".into()),
            members: vec![
                PackageMember { name: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program, description: Some("Post FI journal entries".into()) },
                PackageMember { name: "ZCL_FIN_POSTER".into(), kind: AbapObjectKind::Class, description: Some("FI posting helper".into()) },
                PackageMember { name: "ZIF_FIN_POSTABLE".into(), kind: AbapObjectKind::Interface, description: Some("Postable contract".into()) },
                PackageMember { name: "ZFIN_TOP".into(), kind: AbapObjectKind::Include, description: Some("ZFIN top include".into()) },
                PackageMember { name: "ZFIN_F01".into(), kind: AbapObjectKind::Include, description: Some("ZFIN form routines".into()) },
                PackageMember { name: "ZFIN_UTIL".into(), kind: AbapObjectKind::FunctionGroup, description: Some("FI utility functions".into()) },
                PackageMember { name: "Z_C_SALES_ORDER_KPI".into(), kind: AbapObjectKind::CdsView, description: Some("Sales order KPIs".into()) },
            ],
        });
        self.packages.insert("ZMM".into(), PackageContents {
            package: "ZMM".into(),
            description: Some("Materials Management customisations".into()),
            members: vec![
                PackageMember { name: "ZMM_GRN_CHECK".into(), kind: AbapObjectKind::Program, description: Some("Goods receipt reconciliation".into()) },
                PackageMember { name: "ZCL_MM_GRN_VALIDATOR".into(), kind: AbapObjectKind::Class, description: Some("Goods receipt validator".into()) },
            ],
        });

        // Where-used links — the value of impact analysis at demo time.
        self.where_used.insert(("ZIF_FIN_POSTABLE".into(), AbapObjectKind::Interface), vec![
            WhereUsedHit { object: "ZCL_FIN_POSTER".into(), kind: AbapObjectKind::Class, location: "DEFINITION line 3".into(), usage: "implements".into() },
        ]);
        self.where_used.insert(("ZCL_FIN_POSTER".into(), AbapObjectKind::Class), vec![
            WhereUsedHit { object: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program, location: "INCLUDE zfin_f01 line 8".into(), usage: "call method".into() },
        ]);
        self.where_used.insert(("ZFIN_TOP".into(), AbapObjectKind::Include), vec![
            WhereUsedHit { object: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program, location: "main line 3".into(), usage: "include".into() },
        ]);

        // Tables for ADT-side data preview
        self.tables.insert("T001".into(), vec![
            row(&[("BUKRS", "1000"), ("BUTXT", "Acme Global HQ"), ("WAERS", "USD")]),
            row(&[("BUKRS", "2000"), ("BUTXT", "Acme EMEA"), ("WAERS", "EUR")]),
        ]);
    }
}

fn prog(name: &str, kind: AbapObjectKind, package: &str, description: &str, source: &str) -> ProgramSource {
    let line_count = source.lines().count();
    ProgramSource {
        name: name.into(),
        kind,
        package: Some(package.into()),
        description: Some(description.into()),
        source: source.into(),
        active: true,
        line_count,
    }
}

fn row(pairs: &[(&str, &str)]) -> TableRow {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs { m.insert((*k).into(), serde_json::Value::String((*v).into())); }
    TableRow { values: m }
}

#[async_trait]
impl AdtClient for MockAdtClient {
    fn destination(&self) -> &AdtDestination { &self.destination }

    async fn get_program(&self, name: &str) -> AdtResult<ProgramSource> {
        get_object(&self.programs, name, AbapObjectKind::Program)
    }
    async fn get_class(&self, name: &str) -> AdtResult<ProgramSource> {
        get_object(&self.classes, name, AbapObjectKind::Class)
    }
    async fn get_interface(&self, name: &str) -> AdtResult<ProgramSource> {
        get_object(&self.interfaces, name, AbapObjectKind::Interface)
    }
    async fn get_include(&self, name: &str) -> AdtResult<ProgramSource> {
        get_object(&self.includes, name, AbapObjectKind::Include)
    }
    async fn get_function_module(&self, group: &str, name: &str) -> AdtResult<ProgramSource> {
        self.function_modules
            .get(&(group.to_uppercase(), name.to_uppercase()))
            .cloned()
            .ok_or_else(|| AdtError::NotFound { kind: "FunctionModule".into(), name: format!("{group}/{name}") })
    }
    async fn get_package_contents(&self, package: &str) -> AdtResult<PackageContents> {
        self.packages.get(&package.to_uppercase()).cloned()
            .ok_or_else(|| AdtError::NotFound { kind: "Package".into(), name: package.into() })
    }
    async fn get_cds_view(&self, name: &str) -> AdtResult<CdsView> {
        self.cds_views.get(&name.to_uppercase()).cloned()
            .ok_or_else(|| AdtError::NotFound { kind: "CdsView".into(), name: name.into() })
    }

    async fn search(&self, request: AdtSearchRequest) -> AdtResult<Vec<AdtSearchHit>> {
        let q = request.query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<AdtSearchHit> = Vec::new();

        let kind_match = |k: AbapObjectKind| request.kind.map(|wanted| wanted == k).unwrap_or(true);
        let mut push = |name: &str, kind: AbapObjectKind, desc: Option<&str>, pkg: Option<&str>, score: usize| {
            if kind_match(kind) && score > 0 {
                hits.push(AdtSearchHit {
                    name: name.into(), kind,
                    description: desc.map(String::from),
                    package: pkg.map(String::from),
                    score: score as f32,
                });
            }
        };
        let score_of = |hay: &str| -> usize {
            let hay_lc = hay.to_lowercase();
            terms.iter().map(|t| hay_lc.matches(t).count()).sum()
        };

        for (n, p) in &self.programs {
            push(n, p.kind, p.description.as_deref(), p.package.as_deref(),
                 score_of(&format!("{n} {} {}", p.description.as_deref().unwrap_or(""), p.package.as_deref().unwrap_or(""))));
        }
        for (n, p) in &self.classes {
            push(n, p.kind, p.description.as_deref(), p.package.as_deref(),
                 score_of(&format!("{n} {} {}", p.description.as_deref().unwrap_or(""), p.package.as_deref().unwrap_or(""))));
        }
        for (n, p) in &self.interfaces {
            push(n, p.kind, p.description.as_deref(), p.package.as_deref(),
                 score_of(&format!("{n} {}", p.description.as_deref().unwrap_or(""))));
        }
        for ((_g, n), p) in &self.function_modules {
            push(n, p.kind, p.description.as_deref(), p.package.as_deref(),
                 score_of(&format!("{n} {}", p.description.as_deref().unwrap_or(""))));
        }
        for (n, v) in &self.cds_views {
            push(n, AbapObjectKind::CdsView, None, None,
                 score_of(&format!("{n} {}", v.root_entity)));
        }

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(request.max_results.max(1));
        Ok(hits)
    }

    async fn where_used(&self, request: WhereUsedRequest) -> AdtResult<Vec<WhereUsedHit>> {
        Ok(self.where_used
            .get(&(request.name.to_uppercase(), request.kind))
            .cloned()
            .unwrap_or_default())
    }

    async fn get_table_contents(&self, table: &str, max_rows: usize) -> AdtResult<Vec<TableRow>> {
        if max_rows == 0 || max_rows > MAX_TABLE_ROWS {
            return Err(AdtError::InvalidObjectName(format!("max_rows must be in 1..={MAX_TABLE_ROWS}, got {max_rows}")));
        }
        // Simulate the BTP backend block path on a labelled table so demos
        // exercise the fallback advice.
        if table.eq_ignore_ascii_case("BSEG") {
            return Err(AdtError::DataPreviewBlocked(format!(
                "table {table} is blocked from ADT data preview; fall back to sap.table.read (RFC) or RFC_READ_TABLE",
            )));
        }
        let rows = self.tables.get(&table.to_uppercase()).cloned()
            .ok_or_else(|| AdtError::NotFound { kind: "Table".into(), name: table.into() })?;
        let mut out = rows;
        out.truncate(max_rows);
        Ok(out)
    }

    async fn activate(&self, request: ActivationRequest, ctx: AdtCallContext) -> AdtResult<ActivationOutcome> {
        if ctx.read_only {
            return Err(AdtError::PermissionDenied(format!(
                "activate({} {}) blocked: read-only mode",
                request.kind.label(), request.name,
            )));
        }
        // Acknowledge activation; in real ADT this triggers the activation
        // queue and may produce warnings.
        Ok(ActivationOutcome {
            name: request.name.clone(),
            kind: request.kind,
            activated: true,
            messages: vec![format!("{} {} activated (mock)", request.kind.label(), request.name)],
        })
    }
}

fn get_object(
    map: &HashMap<String, ProgramSource>,
    name: &str,
    kind: AbapObjectKind,
) -> AdtResult<ProgramSource> {
    map.get(&name.to_uppercase()).cloned()
        .ok_or_else(|| AdtError::NotFound { kind: kind.label().into(), name: name.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Arc<MockAdtClient> {
        MockAdtClient::new(AdtDestination::mock("dev"))
    }

    #[tokio::test]
    async fn get_program_returns_source() {
        let c = client();
        let p = c.get_program("zfin_post_je").await.unwrap();
        assert_eq!(p.name, "ZFIN_POST_JE");
        assert!(p.source.contains("BAPI_ACC_DOCUMENT_POST"));
        assert!(p.line_count > 0);
    }

    #[tokio::test]
    async fn search_filters_by_kind() {
        let c = client();
        let hits = c.search(AdtSearchRequest {
            query: "fin".into(),
            kind: Some(AbapObjectKind::Class),
            max_results: 20,
        }).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.kind == AbapObjectKind::Class));
        assert!(hits.iter().any(|h| h.name == "ZCL_FIN_POSTER"));
    }

    #[tokio::test]
    async fn where_used_traces_dependency_chain() {
        let c = client();
        // The interface should report ZCL_FIN_POSTER implementing it.
        let hits = c.where_used(WhereUsedRequest {
            name: "ZIF_FIN_POSTABLE".into(),
            kind: AbapObjectKind::Interface,
        }).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, "ZCL_FIN_POSTER");
        assert_eq!(hits[0].usage, "implements");
    }

    #[tokio::test]
    async fn data_preview_block_is_surfaced() {
        let c = client();
        let err = c.get_table_contents("BSEG", 10).await.unwrap_err();
        assert!(matches!(err, AdtError::DataPreviewBlocked(_)));
    }

    #[tokio::test]
    async fn activate_blocked_in_read_only() {
        let c = client();
        let err = c.activate(
            ActivationRequest { name: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program },
            AdtCallContext { read_only: true },
        ).await.unwrap_err();
        assert!(matches!(err, AdtError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn activate_allowed_when_writes_enabled() {
        let c = client();
        let outcome = c.activate(
            ActivationRequest { name: "ZFIN_POST_JE".into(), kind: AbapObjectKind::Program },
            AdtCallContext { read_only: false },
        ).await.unwrap();
        assert!(outcome.activated);
    }

    #[tokio::test]
    async fn package_contents_includes_seeded_objects() {
        let c = client();
        let pkg = c.get_package_contents("ZFIN").await.unwrap();
        assert!(pkg.members.iter().any(|m| m.name == "ZCL_FIN_POSTER"));
        assert!(pkg.members.iter().any(|m| m.name == "Z_C_SALES_ORDER_KPI"));
    }

    #[tokio::test]
    async fn function_module_lookup_uses_group_namespace() {
        let c = client();
        let fm = c.get_function_module("ZFIN_UTIL", "Z_FIN_VALIDATE_BUKRS").await.unwrap();
        assert_eq!(fm.name, "Z_FIN_VALIDATE_BUKRS");
        assert!(fm.source.contains("FROM t001"));
    }
}
