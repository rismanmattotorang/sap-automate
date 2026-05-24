//! Seed corpus: small, illustrative documents across the four SAP domains.
//!
//! Replaced wholesale in Phase 1A by the Help Portal / ABAP / Signavio /
//! LeanIX crawlers.  Kept here so the Phase 1 sample apps demonstrate real
//! retrieval over realistic content shapes.

use sap_automate_kb::{Document, Domain, InMemoryKb};
use std::collections::HashMap;

pub fn populate(kb: &InMemoryKb) {
    let docs = vec![
        doc(
            "abap.zfin_post_je",
            Domain::Abap,
            "abap-obj://ZFIN/ZFIN_POST_JE",
            "ZFIN_POST_JE",
            "ABAP report ZFIN_POST_JE posts financial journal entries via BAPI_ACC_DOCUMENT_POST. \
             Validates cost-centre against table CSKS, blocks postings to closed periods (T001B), \
             and routes inter-company entries through BAPI_ACC_GL_POSTING_CHECK before commit.",
            &[("package", "ZFIN"), ("type", "REPORT")],
        ),
        doc(
            "abap.zmm_grn_check",
            Domain::Abap,
            "abap-obj://ZMM/ZMM_GRN_CHECK",
            "ZMM_GRN_CHECK",
            "Function module ZMM_GRN_CHECK reconciles goods receipt with purchase order \
             quantities; triggers tolerance check, batch management, and material valuation flow. \
             Calls BAPI_GOODSMVT_CREATE on success.",
            &[("package", "ZMM"), ("type", "FUNCTION")],
        ),
        doc(
            "bpmn.procure_to_pay",
            Domain::Bpmn,
            "bpmn-proc://core/P2P-001",
            "Procure-to-Pay (P2P)",
            "Signavio BPMN process P2P-001: purchase requisition → PO approval → goods receipt → \
             invoice verification → payment release. Mined throughput drops 18% at PO approval \
             due to manager-cost-centre coverage gaps.",
            &[("workspace", "core"), ("model_revision", "v7.3")],
        ),
        doc(
            "bpmn.order_to_cash",
            Domain::Bpmn,
            "bpmn-proc://core/O2C-002",
            "Order-to-Cash (O2C)",
            "Signavio BPMN process O2C-002: sales order entry → ATP check → delivery → \
             billing → cash application. Mining shows 12% rework loop between billing and \
             delivery, primarily caused by incomplete delivery addresses.",
            &[("workspace", "core"), ("model_revision", "v9.1")],
        ),
        doc(
            "leanix.app.s4_finance",
            Domain::Leanix,
            "leanix-fs://FS-12001",
            "S/4HANA Finance",
            "LeanIX application fact sheet for S/4HANA Finance (FS-12001). Lifecycle: active. \
             Business capabilities: general ledger, accounts payable, accounts receivable, \
             asset accounting. Integrations: ZFIN_POST_JE, Concur, Datasphere. EOL: 2031-12.",
            &[("lifecycle", "active"), ("eol", "2031-12")],
        ),
        doc(
            "leanix.app.legacy_billing",
            Domain::Leanix,
            "leanix-fs://FS-08823",
            "Legacy Billing Engine",
            "LeanIX application fact sheet for Legacy Billing Engine (FS-08823). Lifecycle: \
             phase-out. Integrations: ZSD_BILL_GEN, mainframe. EOL: 2026-09. Replacement: \
             S/4HANA SD billing.",
            &[("lifecycle", "phase_out"), ("eol", "2026-09")],
        ),
        doc(
            "help.fi.period_close",
            Domain::SapHelp,
            "sap-help://FI/period-close",
            "Period-End Close in SAP FI",
            "SAP Help Portal page on FI period-end close: open/close posting periods via T001B, \
             execute foreign-currency revaluation, post accruals and deferrals, run BSEG → \
             FAGLFLEXA reconciliation, and generate balance audit trail.",
            &[("module", "FI"), ("breadcrumb", "Finance > General Ledger")],
        ),
        doc(
            "help.mm.goods_movement",
            Domain::SapHelp,
            "sap-help://MM/goods-movement",
            "Goods Movement Posting",
            "SAP Help Portal page describing goods movement postings (transaction MIGO). \
             Movement types 101 (GR for PO), 102 (reversal), 122 (return delivery). \
             Updates MSEG, MKPF, and material valuation.",
            &[("module", "MM"), ("breadcrumb", "Logistics > Inventory Management")],
        ),
    ];

    for d in docs { kb.insert(d); }
}

fn doc(id: &str, domain: Domain, uri: &str, title: &str, body: &str, meta: &[(&str, &str)]) -> Document {
    let mut metadata = HashMap::new();
    for (k, v) in meta { metadata.insert((*k).into(), (*v).into()); }
    Document {
        id: id.into(),
        domain,
        uri: uri.into(),
        title: title.into(),
        body: body.into(),
        breadcrumbs: Vec::new(),
        metadata,
    }
}
