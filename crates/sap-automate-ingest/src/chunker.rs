//! Document chunker.
//!
//! Splits a document body into bounded text windows while preserving (a)
//! sentence boundaries where feasible and (b) the breadcrumb context that
//! disambiguates similar Help pages (paper §VI-E "contextual breadcrumb").

use sap_automate_kb::{Chunk, Document};

#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// Approximate target chunk size in characters.  Defaults to 1600
    /// (~400 tokens), which is the operating point identified for the
    /// SAP Help Portal corpus in §VI-E.
    pub target_chars: usize,
    /// Overlap between adjacent chunks, in characters.
    pub overlap_chars: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self { target_chars: 1600, overlap_chars: 200 }
    }
}

/// Split a document into chunks.  Each chunk carries the breadcrumb +
/// document title as a contextual prefix prepended to the embedded text.
pub fn chunk_document(doc: &Document, cfg: &ChunkerConfig) -> Vec<Chunk> {
    let breadcrumb = if doc.breadcrumbs.is_empty() {
        String::new()
    } else {
        format!("{} > ", doc.breadcrumbs.join(" > "))
    };
    let prefix = format!("{}{}\n", breadcrumb, doc.title);

    let body = doc.body.trim();
    if body.is_empty() {
        return Vec::new();
    }

    let segments = split_text(body, cfg.target_chars, cfg.overlap_chars);
    segments
        .into_iter()
        .enumerate()
        .map(|(idx, seg)| Chunk {
            id: format!("{}#chunk-{idx}", doc.id),
            document_id: doc.id.clone(),
            domain: doc.domain,
            ordinal: idx as u32,
            text: format!("{prefix}{seg}"),
            embedding: None,
            breadcrumbs: doc.breadcrumbs.clone(),
            title: doc.title.clone(),
            uri: doc.uri.clone(),
        })
        .collect()
}

/// Greedy sentence-boundary chunker.
///
/// 1. Walk the body, accumulating until the running length reaches
///    `target_chars`.
/// 2. Try to back up to the nearest sentence end (`. `, `? `, `! `) so the
///    chunk does not split mid-sentence.
/// 3. Advance the cursor by `chunk_len - overlap` so each chunk overlaps the
///    previous one for context preservation.
fn split_text(body: &str, target: usize, overlap: usize) -> Vec<String> {
    let bytes = body.as_bytes();
    let n = body.len();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;

    while start < n {
        let mut end = (start + target).min(n);
        if end < n {
            // Look backwards within the last 200 bytes for a sentence end.
            let lookback = end.saturating_sub(200).max(start + target / 2);
            if let Some(boundary) = find_sentence_end(bytes, lookback, end) {
                end = boundary;
            } else if let Some(space) = find_space(bytes, end) {
                end = space;
            }
        }
        // Ensure char boundary.
        while end < n && !body.is_char_boundary(end) { end += 1; }

        let slice = &body[start..end];
        let trimmed = slice.trim();
        if !trimmed.is_empty() { out.push(trimmed.to_string()); }

        if end >= n { break; }
        let advance = (end - start).saturating_sub(overlap).max(1);
        start += advance;
        while start < n && !body.is_char_boundary(start) { start += 1; }
    }
    out
}

fn find_sentence_end(bytes: &[u8], from: usize, to: usize) -> Option<usize> {
    let to = to.min(bytes.len());
    let from = from.min(to);
    let mut i = to;
    while i > from + 1 {
        let c = bytes[i - 1];
        let p = bytes[i - 2];
        if (p == b'.' || p == b'?' || p == b'!') && (c == b' ' || c == b'\n') {
            return Some(i);
        }
        i -= 1;
    }
    None
}

fn find_space(bytes: &[u8], to: usize) -> Option<usize> {
    let to = to.min(bytes.len());
    let mut i = to;
    while i > 0 {
        let c = bytes[i - 1];
        if c == b' ' || c == b'\n' { return Some(i); }
        i -= 1;
        if to - i > 200 { break; }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use sap_automate_kb::Domain;

    #[test]
    fn breadcrumb_is_prepended() {
        let mut doc = Document::new(
            "sap_help:FI/period-close",
            Domain::SapHelp,
            "sap-help://FI/period-close",
            "Period-End Close",
            "Open posting periods via T001B. Execute foreign-currency revaluation.",
        );
        doc.breadcrumbs = vec!["Finance".into(), "General Ledger".into()];
        let chunks = chunk_document(&doc, &ChunkerConfig::default());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.starts_with("Finance > General Ledger > Period-End Close\n"));
    }

    #[test]
    fn long_body_splits_into_multiple_chunks() {
        let body = "Sentence one. ".repeat(200); // ~2800 chars
        let doc = Document::new("d:1", Domain::SapHelp, "u", "T", body);
        let chunks = chunk_document(&doc, &ChunkerConfig::default());
        assert!(chunks.len() >= 2, "expected at least 2 chunks, got {}", chunks.len());
        // Ordinals are dense.
        for (i, c) in chunks.iter().enumerate() { assert_eq!(c.ordinal as usize, i); }
    }
}
