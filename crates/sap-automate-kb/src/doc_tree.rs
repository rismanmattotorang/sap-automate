//! Hierarchical document tree — the OpenKB + PageIndex convergent pattern.
//!
//! [`VectifyAI/OpenKB`](https://github.com/VectifyAI/OpenKB) compiles long
//! documents into a persistent wiki rather than re-discovering knowledge per
//! query.  Its long-document indexer is
//! [`VectifyAI/PageIndex`](https://github.com/VectifyAI/PageIndex), which
//! builds a *table-of-contents*-shaped tree from a document so an agent can
//! navigate sections by name instead of similarity-blind.
//!
//! For SAP-Automate we adopt the *data structure* — a typed tree with
//! `title`, extractive `summary`, byte-range `[start_index, end_index)`, and
//! children — without taking on the LLM-reasoning-at-build-time dependency.
//! The agent (which lives outside this crate) can still reason over the
//! tree at query time via the `sap.kb.navigate` MCP tool that consumes it.
//!
//! **What this is.**  A deterministic, pure-Rust tree builder that detects
//! heading levels in a document body and produces a navigable hierarchy.
//! Supports two heading dialects out of the box:
//!   - Markdown ATX headings (`#`, `##`, `###`, …) — the canonical PageIndex input.
//!   - Plain-text section markers (`SECTION:`, `1.`, `1.1.`) common in
//!     extracted SAP Help pages.
//!
//! **What this isn't.**  No vector store dependency.  No LLM call.  No
//! page-range tracking (we work on byte-ranges of the post-extraction body).
//!
//! Skipping the LLM at build time costs us automatic-summary fidelity.  We
//! compensate with an extractive 2-sentence summary per node, plus the
//! agent's own reasoning at query time — same separation OpenKB uses
//! between *compile-time tree* and *query-time reasoning*.

use crate::schema::Document;
use serde::{Deserialize, Serialize};

/// One node in a document tree.  Maps directly to a section / sub-section in
/// the source.  Byte ranges are inclusive-start, exclusive-end into the
/// parent `Document::body`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocTreeNode {
    /// Dotted path within the tree (e.g. `1.2.3`).  Empty for the root.
    pub path: String,
    /// Heading depth.  Root is 0; first-level headings are 1.
    pub depth: u32,
    /// Section title — verbatim heading text minus the leading marker.
    pub title: String,
    /// Extractive 2-sentence summary derived from the first sentences of the
    /// section body.  Bounded to ~280 chars (the same operating point the
    /// chunker uses for contextual enrichment).
    pub summary: String,
    /// Byte range into `Document::body`.  Useful for citation rendering.
    pub start_index: usize,
    pub end_index: usize,
    /// Approximate token count — body length divided by 4 (the
    /// universally-cited rough English byte/token ratio).  Cheap to
    /// compute, useful for top-K budgeting at the LLM layer.
    pub approx_tokens: u32,
    /// Direct child nodes.  Ordered as they appear in the source.
    #[serde(default)]
    pub children: Vec<DocTreeNode>,
}

impl DocTreeNode {
    /// Total node count (self + all descendants).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(|c| c.count()).sum::<usize>()
    }

    /// Depth-first walk in source order.
    pub fn visit<'a, F: FnMut(&'a DocTreeNode)>(&'a self, f: &mut F) {
        f(self);
        for c in &self.children {
            c.visit(f);
        }
    }

    /// Find a node by its dotted path (e.g. `"1.2"`).
    pub fn find(&self, path: &str) -> Option<&DocTreeNode> {
        if self.path == path {
            return Some(self);
        }
        for c in &self.children {
            if let Some(n) = c.find(path) {
                return Some(n);
            }
        }
        None
    }
}

/// The whole tree for a document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentTree {
    pub document_id: String,
    pub root: DocTreeNode,
    /// Total leaf count (sections that have no children).
    pub leaf_count: usize,
    /// Max depth reached during construction.
    pub max_depth: u32,
}

impl DocumentTree {
    pub fn node_count(&self) -> usize {
        self.root.count()
    }

    pub fn flat_titles(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        self.root.visit(&mut |n| {
            if !n.path.is_empty() {
                out.push((n.path.clone(), n.title.clone()));
            }
        });
        out
    }
}

/// Build a [`DocumentTree`] from a [`Document`] by detecting heading levels
/// in `body`.  Tries each dialect in order and uses the first one that
/// produces at least one heading; if nothing matches the result is a
/// single-leaf tree (the whole body as one node).
pub fn build_document_tree(doc: &Document) -> DocumentTree {
    let body = doc.body.as_str();
    let headings = detect_headings(body);

    if headings.is_empty() {
        let summary = extract_summary(body);
        let approx_tokens = (body.len() / 4) as u32;
        let root = DocTreeNode {
            path: String::new(),
            depth: 0,
            title: doc.title.clone(),
            summary,
            start_index: 0,
            end_index: body.len(),
            approx_tokens,
            children: Vec::new(),
        };
        return DocumentTree {
            document_id: doc.id.clone(),
            root,
            leaf_count: 1,
            max_depth: 0,
        };
    }

    // Build flat sections (heading-driven), then assemble into a tree by
    // walking the headings list and using `depth` as the stack key.
    let mut sections: Vec<Heading> = headings;
    // Append a sentinel that closes the last section at body end.
    let body_len = body.len();
    sections.push(Heading {
        depth: 0,
        title: String::new(),
        marker_start: body_len,
        body_start: body_len,
    });

    // Pre-compute end-of-body indices for every real section.
    let mut prepared: Vec<PreparedSection> = Vec::with_capacity(sections.len() - 1);
    for w in sections.windows(2) {
        let cur = &w[0];
        let next = &w[1];
        if cur.depth == 0 {
            continue;
        }
        prepared.push(PreparedSection {
            depth: cur.depth,
            title: cur.title.clone(),
            start: cur.marker_start,
            body_start: cur.body_start,
            end: next.marker_start,
        });
    }

    // Walk prepared sections and assemble into a tree.  Use a stack of
    // (depth, index-into-out-list) so each new section knows its parent.
    let mut roots: Vec<DocTreeNode> = Vec::new();
    let mut stack: Vec<(u32, *mut DocTreeNode)> = Vec::new();
    let mut max_depth = 0u32;

    for ps in &prepared {
        max_depth = max_depth.max(ps.depth);
        let body_slice = body.get(ps.body_start..ps.end).unwrap_or("");
        let summary = extract_summary(body_slice);
        let approx_tokens = (body_slice.len() / 4) as u32;
        let node = DocTreeNode {
            path: String::new(), // assigned below
            depth: ps.depth,
            title: ps.title.clone(),
            summary,
            start_index: ps.start,
            end_index: ps.end,
            approx_tokens,
            children: Vec::new(),
        };

        // Pop the stack until we find a strict-ancestor (lower depth) — or
        // empty the stack to make this a new root.
        while let Some(&(d, _)) = stack.last() {
            if d >= ps.depth {
                stack.pop();
            } else {
                break;
            }
        }

        if let Some(&(_, parent_ptr)) = stack.last() {
            // SAFETY: `parent_ptr` is a node we own in `roots` (or in some
            // ancestor's `children`).  We only append; we never realloc the
            // `roots` vector during this loop because we push to children,
            // not to roots, after the first iteration.  The pointer remains
            // valid for the duration of the borrow.
            let parent = unsafe { &mut *parent_ptr };
            parent.children.push(node);
            let new_child = parent.children.last_mut().unwrap();
            let child_ptr = new_child as *mut _;
            stack.push((ps.depth, child_ptr));
        } else {
            roots.push(node);
            let root_ptr = roots.last_mut().unwrap() as *mut _;
            stack.push((ps.depth, root_ptr));
        }
    }

    // Assign dotted paths.
    for (i, node) in roots.iter_mut().enumerate() {
        assign_paths(node, &format!("{}", i + 1));
    }

    // The document's overall root wraps the heading roots.
    let body_summary = extract_summary(body);
    let approx_tokens = (body.len() / 4) as u32;
    let root = DocTreeNode {
        path: String::new(),
        depth: 0,
        title: doc.title.clone(),
        summary: body_summary,
        start_index: 0,
        end_index: body_len,
        approx_tokens,
        children: roots,
    };
    let leaf_count = count_leaves_total(&root);
    DocumentTree {
        document_id: doc.id.clone(),
        root,
        leaf_count,
        max_depth,
    }
}

fn assign_paths(node: &mut DocTreeNode, path: &str) {
    node.path = path.to_string();
    for (i, child) in node.children.iter_mut().enumerate() {
        let cp = format!("{}.{}", path, i + 1);
        assign_paths(child, &cp);
    }
}

fn count_leaves(node: &DocTreeNode, out: &mut usize) {
    if node.children.is_empty() {
        *out += 1;
    } else {
        for c in &node.children {
            count_leaves(c, out);
        }
    }
}

fn count_leaves_total(node: &DocTreeNode) -> usize {
    let mut n = 0;
    count_leaves(node, &mut n);
    n.max(1)
}

struct Heading {
    depth: u32,
    title: String,
    /// Byte offset of the heading marker itself (i.e. the `#` or `1.`).
    marker_start: usize,
    /// Byte offset of the heading's body content (just past the newline
    /// after the marker line).
    body_start: usize,
}

struct PreparedSection {
    depth: u32,
    title: String,
    start: usize,
    body_start: usize,
    end: usize,
}

/// Try detection dialects in order; return the first non-empty result.
fn detect_headings(body: &str) -> Vec<Heading> {
    let md = detect_atx_headings(body);
    if !md.is_empty() {
        return md;
    }
    detect_section_markers(body)
}

/// Markdown ATX headings: `# Title`, `## Title`, `### Title`, …  Stops at
/// depth 6 (the markdown spec maximum).  Headings must start at the
/// beginning of a line.
fn detect_atx_headings(body: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let n = bytes.len();
    while i < n {
        let line_start = i;
        // Find end of line.
        let mut j = i;
        while j < n && bytes[j] != b'\n' {
            j += 1;
        }
        let line = &body[line_start..j];
        if let Some(h) = parse_atx_line(line, line_start) {
            // body_start is just past the newline.
            let body_start = (j + 1).min(n);
            out.push(Heading {
                depth: h.depth,
                title: h.title,
                marker_start: line_start,
                body_start,
            });
        }
        i = j + 1;
    }
    out
}

struct ParsedAtx {
    depth: u32,
    title: String,
}

fn parse_atx_line(line: &str, _line_start: usize) -> Option<ParsedAtx> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let mut depth = 0u32;
    for ch in trimmed.chars() {
        if ch == '#' {
            depth += 1;
            if depth > 6 {
                return None;
            }
        } else {
            break;
        }
    }
    if depth == 0 || depth > 6 {
        return None;
    }
    // After the hashes there must be at least one space.
    let rest = &trimmed[depth as usize..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some(ParsedAtx { depth, title })
}

/// Plain-text section markers used in extracted SAP Help pages and ABAP
/// source comments.  We accept:
///   - `SECTION: <Title>` at the start of a line.
///   - Numbered headings of the form `1.`, `1.1.`, `1.1.1.` — depth is the
///     dot count.
fn detect_section_markers(body: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        let line_start = i;
        let mut j = i;
        while j < n && bytes[j] != b'\n' {
            j += 1;
        }
        let line = &body[line_start..j];
        let trimmed = line.trim_start();
        let body_start = (j + 1).min(n);

        if let Some(rest) = trimmed.strip_prefix("SECTION:") {
            let title = rest.trim().to_string();
            if !title.is_empty() {
                out.push(Heading {
                    depth: 1,
                    title,
                    marker_start: line_start,
                    body_start,
                });
            }
        } else if let Some(parsed) = parse_numbered(trimmed) {
            out.push(Heading {
                depth: parsed.depth,
                title: parsed.title,
                marker_start: line_start,
                body_start,
            });
        }
        i = j + 1;
    }
    out
}

fn parse_numbered(line: &str) -> Option<ParsedAtx> {
    // Walk leading "digits + dot" groups; at least one required.
    let mut depth = 0u32;
    let mut idx = 0usize;
    let bytes = line.as_bytes();
    loop {
        let saw_digits_at = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == saw_digits_at {
            break;
        }
        if idx < bytes.len() && bytes[idx] == b'.' {
            depth += 1;
            idx += 1;
        } else {
            return None;
        }
    }
    if depth == 0 {
        return None;
    }
    // After the numbered prefix there must be a space and a title.
    if idx >= bytes.len() {
        return None;
    }
    if bytes[idx] != b' ' && bytes[idx] != b'\t' {
        return None;
    }
    let title = line[idx..].trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some(ParsedAtx { depth, title })
}

/// Up to 2 sentences (or 280 chars), trimmed.  Mirrors the chunker's
/// extractive summary heuristic so callers see consistent shape across
/// `Chunk` and `DocTreeNode`.
fn extract_summary(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut sentence_count = 0u8;
    for ch in body.chars() {
        out.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            sentence_count += 1;
        }
        if sentence_count >= 2 || out.len() >= 280 {
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Domain;

    fn mk(body: &str) -> Document {
        Document::new("d:1", Domain::SapHelp, "u://1", "Title", body)
    }

    #[test]
    fn flat_doc_becomes_single_root() {
        let tree = build_document_tree(&mk("Just one paragraph. No headings here."));
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.leaf_count, 1);
        assert_eq!(tree.max_depth, 0);
        assert_eq!(tree.root.title, "Title");
    }

    #[test]
    fn markdown_atx_headings_build_a_two_level_tree() {
        let body = "\
# Overview
First sentence. Second sentence.
## Closing
Closing intro.
## Revaluation
Revaluation body.
### Logic
Logic detail.
";
        let tree = build_document_tree(&mk(body));
        // Root + Overview + Closing + Revaluation + Logic = 5 nodes.
        assert_eq!(tree.node_count(), 5);
        assert_eq!(tree.max_depth, 3);
        assert_eq!(tree.root.children.len(), 1);
        let overview = &tree.root.children[0];
        assert_eq!(overview.title, "Overview");
        assert_eq!(overview.path, "1");
        assert_eq!(overview.children.len(), 2, "Closing + Revaluation");
        let reval = &overview.children[1];
        assert_eq!(reval.title, "Revaluation");
        assert_eq!(reval.path, "1.2");
        assert_eq!(reval.children.len(), 1);
        assert_eq!(reval.children[0].title, "Logic");
        assert_eq!(reval.children[0].path, "1.2.1");
    }

    #[test]
    fn numbered_section_markers_build_a_tree() {
        let body = "\
1. Period close
Open posting periods via T001B.
1.1. Foreign currency
Run FX revaluation.
1.2. Reconciliation
BSEG to FAGLFLEXA reconciliation.
";
        let tree = build_document_tree(&mk(body));
        assert!(tree.max_depth >= 2);
        let period = &tree.root.children[0];
        assert_eq!(period.title, "Period close");
        assert_eq!(period.children.len(), 2);
    }

    #[test]
    fn section_keyword_marker() {
        let body = "\
SECTION: Setup
Initial steps.
SECTION: Execution
Run the report.
";
        let tree = build_document_tree(&mk(body));
        assert_eq!(tree.root.children.len(), 2);
        assert_eq!(tree.root.children[0].title, "Setup");
    }

    #[test]
    fn summary_is_bounded_to_two_sentences() {
        let body = "First. Second. Third. Fourth.";
        let s = extract_summary(body);
        assert!(s.contains("First"));
        assert!(s.contains("Second"));
        assert!(!s.contains("Third"));
    }

    #[test]
    fn find_resolves_dotted_paths() {
        let body = "# A\nbody\n## B\nbody\n### C\nbody\n";
        let tree = build_document_tree(&mk(body));
        let c = tree.root.find("1.1.1").expect("1.1.1");
        assert_eq!(c.title, "C");
        assert!(tree.root.find("9.9").is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let body = "# A\ntext\n## B\nmore\n";
        let tree = build_document_tree(&mk(body));
        let s = serde_json::to_string(&tree).unwrap();
        let back: DocumentTree = serde_json::from_str(&s).unwrap();
        assert_eq!(back, tree);
    }
}
