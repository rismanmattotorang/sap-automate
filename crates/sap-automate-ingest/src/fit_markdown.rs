//! Crawl4AI-style "fit markdown" content filter.
//!
//! Crawl4AI ships two markdown flavours — `clean` (preserves structure) and
//! `fit` (removes boilerplate the LLM doesn't need).  Their `fit` flavour
//! uses BM25 against a query (typically the page title) to score blocks
//! and drop low-relevance ones — typical victims are navigation menus,
//! footers, "related articles" sidebars, and cookie banners.
//!
//! We adopt the *idea* without taking the dependency: a tiny pure-Rust
//! BM25-block-filter that scores paragraphs against a topic string and
//! returns the body with low-score paragraphs elided.
//!
//! Block boundary: a blank line (one or more consecutive `\n`s).  This
//! matches the way `parse_help_portal_html` joins extracted text and the
//! way most real-world HTML-to-text converters present their output.

use std::collections::HashMap;

/// Configuration for `fit_markdown_filter`.
#[derive(Debug, Clone)]
pub struct FitConfig {
    /// BM25 k1.  Defaults to 1.5, same as the store-level BM25.
    pub k1: f32,
    /// BM25 b.  Defaults to 0.75.
    pub b: f32,
    /// Minimum BM25 score for a block to survive.  Blocks scoring below
    /// this are dropped.  `0.0` means "drop only blocks that have zero
    /// term overlap with the topic".
    pub min_score: f32,
    /// Hard floor on block length in characters.  Very short blocks
    /// (1–2 words) are almost always nav / breadcrumb artefacts; drop
    /// them unconditionally.
    pub min_block_chars: usize,
    /// Always keep blocks longer than this — they carry the real content
    /// even if topical overlap is low.
    pub keep_block_chars: usize,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self {
            k1: 1.5,
            b: 0.75,
            min_score: 0.0,
            min_block_chars: 40,
            keep_block_chars: 600,
        }
    }
}

/// Telemetry from one fit-filter pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct FitStats {
    pub blocks_in: usize,
    pub blocks_out: usize,
    pub chars_in: usize,
    pub chars_out: usize,
}

impl FitStats {
    pub fn block_retention(&self) -> f32 {
        if self.blocks_in == 0 { 1.0 } else { self.blocks_out as f32 / self.blocks_in as f32 }
    }
    pub fn char_retention(&self) -> f32 {
        if self.chars_in == 0 { 1.0 } else { self.chars_out as f32 / self.chars_in as f32 }
    }
}

/// Drop low-relevance blocks from `body` using BM25 against `topic`.
/// Returns `(filtered_body, stats)`.  Blocks shorter than
/// `cfg.min_block_chars` are always dropped; blocks longer than
/// `cfg.keep_block_chars` are always kept; everything in between is
/// scored.
pub fn fit_markdown_filter(body: &str, topic: &str, cfg: &FitConfig) -> (String, FitStats) {
    let blocks: Vec<&str> = body
        .split("\n\n")
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .collect();
    let chars_in = body.len();
    let blocks_in = blocks.len();

    if blocks.is_empty() {
        return (
            String::new(),
            FitStats { blocks_in: 0, blocks_out: 0, chars_in, chars_out: 0 },
        );
    }

    let topic_terms = tokens(topic);
    let scored = bm25_score(&blocks, &topic_terms, cfg.k1, cfg.b);
    let mut kept: Vec<&str> = Vec::with_capacity(blocks.len());
    for (block, score) in blocks.iter().zip(scored.iter()) {
        if block.len() >= cfg.keep_block_chars {
            kept.push(block);
            continue;
        }
        if block.len() < cfg.min_block_chars {
            continue;
        }
        if *score >= cfg.min_score && *score > 0.0 {
            kept.push(block);
        }
    }

    let out = kept.join("\n\n");
    let stats = FitStats {
        blocks_in,
        blocks_out: kept.len(),
        chars_in,
        chars_out: out.len(),
    };
    (out, stats)
}

/// Score each block with BM25 against the given query-term vector.  Returns
/// a parallel `Vec<f32>` of the same length as `blocks`.
fn bm25_score(blocks: &[&str], query_terms: &[String], k1: f32, b: f32) -> Vec<f32> {
    let n = blocks.len();
    if n == 0 || query_terms.is_empty() {
        return vec![0.0; n];
    }

    let mut term_counts: Vec<HashMap<String, u32>> = Vec::with_capacity(n);
    let mut lens: Vec<u32> = Vec::with_capacity(n);
    for b in blocks {
        let mut c = HashMap::new();
        let mut l = 0u32;
        for t in tokens(b) {
            *c.entry(t).or_insert(0) += 1;
            l += 1;
        }
        term_counts.push(c);
        lens.push(l);
    }
    let avg_dl = lens.iter().copied().sum::<u32>() as f32 / n as f32;
    if avg_dl == 0.0 {
        return vec![0.0; n];
    }

    let df: HashMap<&String, usize> = query_terms.iter().map(|t| {
        let c = term_counts.iter().filter(|c| c.contains_key(t)).count();
        (t, c)
    }).collect();

    (0..n).map(|i| {
        let dl = lens[i] as f32;
        let mut score = 0.0f32;
        for t in query_terms {
            let f = *term_counts[i].get(t).unwrap_or(&0) as f32;
            if f == 0.0 { continue; }
            let df_t = *df.get(t).unwrap_or(&0) as f32;
            let idf = ((n as f32 - df_t + 0.5) / (df_t + 0.5) + 1.0).ln();
            let denom = f + k1 * (1.0 - b + b * dl / avg_dl);
            score += idf * (f * (k1 + 1.0)) / denom;
        }
        score
    }).collect()
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            if cur.len() >= 2 { out.push(std::mem::take(&mut cur)); }
            else { cur.clear(); }
        }
    }
    if cur.len() >= 2 { out.push(cur); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_short_navigation_blocks_keeps_content() {
        let body = "\
Home > Finance > Period Close

This page explains foreign currency revaluation for SAP FI. The procedure runs at month-end and produces postings against FAGLFLEXA. The tcode is F.05 and the underlying program is SAPF100.

Cookie banner.\n\nNext\n\nPrevious\n\nThe SAP standard procedure operates on company code BUKRS and currency type WAERS, with the resulting clearing document written through BAPI_ACC_DOCUMENT_POST. Operators should verify open items via FBL3N before running the revaluation, otherwise reversals get tangled.";
        let cfg = FitConfig::default();
        let (out, stats) = fit_markdown_filter(body, "foreign currency revaluation period close", &cfg);
        // The content blocks should remain.
        assert!(out.contains("foreign currency revaluation"));
        assert!(out.contains("BAPI_ACC_DOCUMENT_POST"));
        // The "Next" / "Previous" / "Cookie banner" / breadcrumb blocks should be dropped.
        assert!(!out.contains("Cookie banner"));
        assert!(!out.contains("Next"));
        assert!(stats.blocks_in > stats.blocks_out);
    }

    #[test]
    fn long_block_is_always_kept_even_with_no_topical_overlap() {
        let long = "lorem ipsum ".repeat(80); // ~960 chars
        let body = format!("Home\n\n{long}");
        let cfg = FitConfig::default();
        let (out, stats) = fit_markdown_filter(&body, "completely unrelated topic words", &cfg);
        assert!(out.contains("lorem"));
        // The 4-char "Home" block dropped; long block kept.
        assert_eq!(stats.blocks_in, 2);
        assert_eq!(stats.blocks_out, 1);
    }

    #[test]
    fn empty_body_returns_empty() {
        let cfg = FitConfig::default();
        let (out, stats) = fit_markdown_filter("", "x", &cfg);
        assert!(out.is_empty());
        assert_eq!(stats.blocks_in, 0);
        assert_eq!(stats.blocks_out, 0);
    }

    #[test]
    fn retention_ratios_make_sense() {
        let body = "short\n\nshort\n\nshort\n\n".to_string()
            + &"This is a medium-length sentence about period close and FBL3N. ".repeat(8);
        let cfg = FitConfig::default();
        let (_out, stats) = fit_markdown_filter(&body, "period close", &cfg);
        assert!(stats.block_retention() <= 1.0);
        assert!(stats.char_retention() <= 1.0);
    }
}
