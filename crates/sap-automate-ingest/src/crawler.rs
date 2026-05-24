//! SAP Help Portal crawler.
//!
//! Phase 1A focuses on a single source family (Help Portal HTML pages).  The
//! crawler is split into two surfaces:
//!   1. `parse_help_portal_html` — pure function that turns one HTML page
//!      into a `ParsedPage`.  Has a dedicated test suite that runs nightly
//!      against a snapshot corpus (paper §X-N risk-1).
//!   2. `HelpPortalCrawler` — orchestrator that walks a directory or an
//!      HTTP root, calls the parser, and yields `Document`s.
//!
//! Real production crawling against help.sap.com requires user agent +
//! rate-limit + ETag handling; the trait surface accommodates all three but
//! Phase 1A ships only the local-filesystem driver so CI / offline test runs
//! work without network access.

use sap_automate_kb::{Document, Domain};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum CrawlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("parse: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedPage {
    pub title: String,
    pub breadcrumbs: Vec<String>,
    pub body: String,
    pub module: Option<String>,
}

/// Pure HTML → ParsedPage conversion.  Used both online (live HTML) and
/// offline (snapshot corpus).
pub fn parse_help_portal_html(raw: &str) -> Result<ParsedPage, CrawlError> {
    let doc = Html::parse_document(raw);

    let title = pick_text(&doc, &["h1", "title"])
        .unwrap_or_default()
        .trim()
        .to_string();
    if title.is_empty() {
        return Err(CrawlError::Parse("missing title".into()));
    }

    let breadcrumbs = collect_text(&doc, "nav.breadcrumb a, .breadcrumb li, [data-breadcrumb] a");
    let breadcrumbs = breadcrumbs
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Primary content area.  Try paper §VI-C suggested selectors in order.
    let body = pick_text(&doc, &["main", "article", ".content", "#content", "body"])
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if body.is_empty() {
        return Err(CrawlError::Parse("empty body".into()));
    }

    let module = pick_meta(&doc, "module");

    Ok(ParsedPage { title, breadcrumbs, body, module })
}

fn pick_text(doc: &Html, selectors: &[&str]) -> Option<String> {
    for raw in selectors {
        let Ok(sel) = Selector::parse(raw) else { continue };
        if let Some(el) = doc.select(&sel).next() {
            let text: String = el.text().collect::<Vec<_>>().join(" ");
            let trimmed = text.trim();
            if !trimmed.is_empty() { return Some(trimmed.to_string()); }
        }
    }
    None
}

fn collect_text(doc: &Html, raw: &str) -> Vec<String> {
    let Ok(sel) = Selector::parse(raw) else { return Vec::new() };
    doc.select(&sel)
        .map(|el| el.text().collect::<Vec<_>>().join(" "))
        .collect()
}

fn pick_meta(doc: &Html, name: &str) -> Option<String> {
    let raw = format!(r#"meta[name="{name}"]"#);
    let sel = Selector::parse(&raw).ok()?;
    let el = doc.select(&sel).next()?;
    el.value().attr("content").map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Crawler driver
// ---------------------------------------------------------------------------

pub struct HelpPortalCrawler {
    /// Domain assigned to all documents produced by this crawler.  Defaults
    /// to `SapHelp` but is configurable so the same parser can drive ABAP
    /// HTML dumps or Signavio exports.
    pub domain: Domain,
}

impl HelpPortalCrawler {
    pub fn new() -> Self { Self { domain: Domain::SapHelp } }

    pub fn with_domain(mut self, domain: Domain) -> Self {
        self.domain = domain;
        self
    }

    /// Walk a local directory of `*.html` files (test / offline mode).
    pub async fn crawl_directory(&self, root: impl AsRef<Path>) -> Result<Vec<Document>, CrawlError> {
        let mut docs = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.as_ref().to_path_buf()];
        while let Some(p) = stack.pop() {
            let meta = tokio::fs::metadata(&p).await?;
            if meta.is_dir() {
                let mut rd = tokio::fs::read_dir(&p).await?;
                while let Some(entry) = rd.next_entry().await? {
                    stack.push(entry.path());
                }
            } else if p.extension().map(|e| e == "html").unwrap_or(false) {
                match self.crawl_file(&p).await {
                    Ok(d) => docs.push(d),
                    Err(e) => warn!(path = %p.display(), error = %e, "skipping page"),
                }
            }
        }
        info!(count = docs.len(), root = %root.as_ref().display(), "directory crawl complete");
        Ok(docs)
    }

    async fn crawl_file(&self, path: &Path) -> Result<Document, CrawlError> {
        let raw = tokio::fs::read_to_string(path).await?;
        let page = parse_help_portal_html(&raw)?;
        let rel = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
        let id = format!("{}:{}", self.domain.collection(), rel);
        let uri = format!("file://{}", path.display());

        let mut doc = Document::new(id, self.domain, uri, page.title, page.body);
        doc.breadcrumbs = page.breadcrumbs;
        if let Some(module) = page.module {
            doc.metadata.insert("module".into(), module);
        }
        debug!(id = %doc.id, "parsed help portal page");
        Ok(doc)
    }

    /// HTTP fetch with ETag / If-None-Match (paper §VI-C).  Returns `None`
    /// when the server responds 304.
    pub async fn fetch_url(
        &self,
        http: &reqwest::Client,
        url: &str,
        previous_etag: Option<&str>,
    ) -> Result<Option<(Document, Option<String>)>, CrawlError> {
        let mut req = http.get(url);
        if let Some(etag) = previous_etag {
            req = req.header("If-None-Match", etag);
        }
        let resp = req.send().await.map_err(|e| CrawlError::Http(e.to_string()))?;
        if resp.status().as_u16() == 304 {
            debug!(url, "304 Not Modified");
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(CrawlError::Http(format!("{} -> {}", url, resp.status())));
        }
        let etag = resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok().map(String::from));
        let body = resp.text().await.map_err(|e| CrawlError::Http(e.to_string()))?;
        let page = parse_help_portal_html(&body)?;
        let id = derive_id_from_url(url, self.domain);
        let mut doc = Document::new(id, self.domain, url, page.title, page.body);
        doc.breadcrumbs = page.breadcrumbs;
        doc.etag = etag.clone();
        if let Some(module) = page.module {
            doc.metadata.insert("module".into(), module);
        }
        Ok(Some((doc, etag)))
    }
}

impl Default for HelpPortalCrawler {
    fn default() -> Self { Self::new() }
}

fn derive_id_from_url(url: &str, domain: Domain) -> String {
    let parsed = url::Url::parse(url);
    let suffix = parsed
        .as_ref()
        .ok()
        .map(|u| u.path().trim_start_matches('/').replace('/', "_"))
        .unwrap_or_else(|| sha_short(url));
    format!("{}:{}", domain.collection(), suffix)
}

fn sha_short(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(&h.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
<!doctype html>
<html>
<head>
  <title>Period-End Close in SAP FI - SAP Help Portal</title>
  <meta name="module" content="FI"/>
</head>
<body>
  <nav class="breadcrumb">
    <a>Finance</a> &gt; <a>General Ledger</a> &gt; <a>Period Close</a>
  </nav>
  <h1>Period-End Close in SAP FI</h1>
  <main>
    <p>Open and close posting periods via T001B.</p>
    <p>Execute foreign-currency revaluation.</p>
    <p>Post accruals and deferrals; run BSEG -> FAGLFLEXA reconciliation.</p>
  </main>
</body>
</html>
"#;

    #[test]
    fn parses_title_breadcrumb_body() {
        let p = parse_help_portal_html(SAMPLE).unwrap();
        assert_eq!(p.title, "Period-End Close in SAP FI");
        assert_eq!(p.breadcrumbs, vec!["Finance", "General Ledger", "Period Close"]);
        assert!(p.body.contains("T001B"));
        assert!(p.body.contains("FAGLFLEXA"));
        assert_eq!(p.module.as_deref(), Some("FI"));
    }

    #[test]
    fn rejects_empty_title() {
        let html = "<html><body><p>no title here</p></body></html>";
        let err = parse_help_portal_html(html).unwrap_err();
        match err {
            CrawlError::Parse(m) => assert!(m.contains("title")),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}
