//! Seed corpus: small, illustrative documents across the four SAP domains.
//!
//! Phase 1A: runs the documents through the chunker and embedder so the
//! KnowledgeStore exposes the same chunk-level surface as a real ingestion
//! pipeline does.

use sap_automate_ingest::{chunk_document, ChunkerConfig, EmbeddingClient};
use sap_automate_kb::{Document, Domain, KnowledgeStore, UpsertBatch};

pub async fn populate_with_embeddings(
    store: &std::sync::Arc<dyn KnowledgeStore>,
    embedder: &dyn EmbeddingClient,
) -> anyhow::Result<()> {
    let docs = seed_documents();
    let chunker = ChunkerConfig::default();

    for doc in docs {
        let mut chunks = chunk_document(&doc, &chunker);
        if chunks.is_empty() { continue; }
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let vectors = embedder.embed(&texts).await?;
        for (chunk, vec) in chunks.iter_mut().zip(vectors.into_iter()) {
            chunk.embedding = Some(vec);
        }
        store.upsert(UpsertBatch { documents: vec![doc], chunks }).await?;
    }
    Ok(())
}

fn seed_documents() -> Vec<Document> {
    let mut out = Vec::new();

    out.push({
        let mut d = Document::new(
            "abap:ZFIN/ZFIN_POST_JE", Domain::Abap, "abap-obj://ZFIN/ZFIN_POST_JE",
            "ZFIN_POST_JE",
            "ABAP report ZFIN_POST_JE posts financial journal entries via BAPI_ACC_DOCUMENT_POST. \
             Validates cost-centre against table CSKS, blocks postings to closed periods (T001B), \
             and routes inter-company entries through BAPI_ACC_GL_POSTING_CHECK before commit.",
        );
        d.metadata.insert("package".into(), "ZFIN".into());
        d.metadata.insert("type".into(), "REPORT".into());
        d
    });

    out.push({
        let mut d = Document::new(
            "abap:ZMM/ZMM_GRN_CHECK", Domain::Abap, "abap-obj://ZMM/ZMM_GRN_CHECK",
            "ZMM_GRN_CHECK",
            "Function module ZMM_GRN_CHECK reconciles goods receipt with purchase order \
             quantities; triggers tolerance check, batch management, and material valuation flow. \
             Calls BAPI_GOODSMVT_CREATE on success.",
        );
        d.metadata.insert("package".into(), "ZMM".into());
        d.metadata.insert("type".into(), "FUNCTION".into());
        d
    });

    out.push({
        let mut d = Document::new(
            "bpmn:core/P2P-001", Domain::Bpmn, "bpmn-proc://core/P2P-001",
            "Procure-to-Pay (P2P)",
            "Signavio BPMN process P2P-001: purchase requisition into PO approval into goods receipt into \
             invoice verification into payment release. Mined throughput drops 18% at PO approval \
             due to manager-cost-centre coverage gaps.",
        );
        d.breadcrumbs = vec!["core".into()];
        d
    });

    out.push({
        let mut d = Document::new(
            "bpmn:core/O2C-002", Domain::Bpmn, "bpmn-proc://core/O2C-002",
            "Order-to-Cash (O2C)",
            "Signavio BPMN process O2C-002: sales order entry into ATP check into delivery into \
             billing into cash application. Mining shows 12% rework loop between billing and \
             delivery, primarily caused by incomplete delivery addresses.",
        );
        d.breadcrumbs = vec!["core".into()];
        d
    });

    out.push(Document::new(
        "leanix:FS-12001", Domain::Leanix, "leanix-fs://FS-12001",
        "S/4HANA Finance",
        "LeanIX application fact sheet for S/4HANA Finance (FS-12001). Lifecycle: active. \
         Business capabilities: general ledger, accounts payable, accounts receivable, asset \
         accounting. Integrations: ZFIN_POST_JE, Concur, Datasphere. EOL: 2031-12.",
    ));

    out.push(Document::new(
        "leanix:FS-08823", Domain::Leanix, "leanix-fs://FS-08823",
        "Legacy Billing Engine",
        "LeanIX application fact sheet for Legacy Billing Engine (FS-08823). Lifecycle: \
         phase-out. Integrations: ZSD_BILL_GEN, mainframe. EOL: 2026-09. Replacement: S/4HANA \
         SD billing.",
    ));

    out.push({
        let mut d = Document::new(
            "sap_help:FI/period-close", Domain::SapHelp, "sap-help://FI/period-close",
            "Period-End Close in SAP FI",
            "SAP Help Portal page on FI period-end close: open and close posting periods via T001B, \
             execute foreign-currency revaluation, post accruals and deferrals, run BSEG to FAGLFLEXA \
             reconciliation, and generate balance audit trail.",
        );
        d.breadcrumbs = vec!["Finance".into(), "General Ledger".into()];
        d.metadata.insert("module".into(), "FI".into());
        d
    });

    out.push({
        let mut d = Document::new(
            "sap_help:MM/goods-movement", Domain::SapHelp, "sap-help://MM/goods-movement",
            "Goods Movement Posting",
            "SAP Help Portal page describing goods movement postings (transaction MIGO). \
             Movement types 101 (GR for PO), 102 (reversal), 122 (return delivery). \
             Updates MSEG, MKPF, and material valuation.",
        );
        d.breadcrumbs = vec!["Logistics".into(), "Inventory Management".into()];
        d.metadata.insert("module".into(), "MM".into());
        d
    });

    out
}
