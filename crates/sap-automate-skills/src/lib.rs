//! Agentic skill library.
//!
//! Convergent pattern from `SAP/mdk-mcp-server` (AGENTS.md), `fr0ster/mcp-abap-adt`
//! (handler dedup / role-based filtering), and the `marianfoo/sap-ai-mcp-servers`
//! catalogue (CAP Agentic Engineered Skills, ARC-1 SAP Skills, RAP Skills):
//!
//! A **skill** is a declarative workflow template that wraps tool composition
//! + prompt engineering for a specific SAP scenario.  Agents invoke skills
//! via MCP `prompts/get`.  Each skill ships as a markdown file with YAML
//! frontmatter:
//!
//! ```text
//! ---
//! name: sap.skill.period-close-investigation
//! description: Investigate root causes of an FI period-close delay
//! tags: [fi, period-close, investigation]
//! requires_tools: [sap.docs.search, sap.table.read, abap.adt.where_used]
//! arguments:
//!   - name: company_code
//!     description: BUKRS, e.g. "1000"
//!     required: true
//!   - name: fiscal_period
//!     description: e.g. "2026-M03"
//!     required: false
//! ---
//!
//! Investigate the FI period close for {{company_code}} ({{fiscal_period}}).
//!
//! Steps:
//! 1. Use sap.docs.search to find the official period-close procedure ...
//! ```
//!
//! At server start `SkillRegistry::scan_paths()` walks the configured
//! directories, parses each `*.md` file, and yields one `Skill` per file.
//! The server then exposes them as MCP prompts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed frontmatter in {path}: {reason}")]
    Frontmatter { path: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Declarative workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// MCP tools that the skill body references.  The server validates that
    /// they exist at registry time so a stale skill fails loudly.
    #[serde(default)]
    pub requires_tools: Vec<String>,
    #[serde(default)]
    pub arguments: Vec<SkillArgument>,
    /// Skill body (the markdown after the frontmatter).
    pub body: String,
    /// Source file path, useful for hot-reload and audit logs.
    #[serde(default)]
    pub source: Option<String>,
}

impl Skill {
    /// Substitute `{{arg_name}}` placeholders in the body with values from
    /// the supplied map.  Missing required arguments are flagged by leaving
    /// the placeholder visible (so the agent notices).
    pub fn render(&self, args: &HashMap<String, String>) -> String {
        let mut out = self.body.clone();
        for arg in &self.arguments {
            let placeholder = format!("{{{{{}}}}}", arg.name);
            let replacement = args.get(&arg.name)
                .cloned()
                .unwrap_or_else(|| if arg.required { format!("<MISSING {}>", arg.name) } else { String::new() });
            out = out.replace(&placeholder, &replacement);
        }
        out
    }
}

/// Discovers, parses, and caches skills.
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn skills(&self) -> impl Iterator<Item = &Skill> { self.skills.values() }

    pub fn get(&self, name: &str) -> Option<&Skill> { self.skills.get(name) }

    pub fn len(&self) -> usize { self.skills.len() }
    pub fn is_empty(&self) -> bool { self.skills.is_empty() }

    pub fn insert(&mut self, skill: Skill) { self.skills.insert(skill.name.clone(), skill); }

    /// Walk a directory recursively and load every `*.md` file as a skill.
    /// Idempotent — re-scan to hot-reload.
    pub async fn scan_paths(&mut self, paths: &[PathBuf]) -> Result<usize, SkillError> {
        let mut loaded = 0;
        for root in paths {
            if !root.exists() { continue; }
            let mut stack = vec![root.clone()];
            while let Some(p) = stack.pop() {
                let meta = match tokio::fs::metadata(&p).await { Ok(m) => m, Err(_) => continue };
                if meta.is_dir() {
                    let mut rd = match tokio::fs::read_dir(&p).await { Ok(r) => r, Err(_) => continue };
                    while let Ok(Some(entry)) = rd.next_entry().await {
                        stack.push(entry.path());
                    }
                } else if p.extension().map(|e| e == "md").unwrap_or(false) {
                    match parse_skill_file(&p).await {
                        Ok(skill) => {
                            debug!(name = %skill.name, path = %p.display(), "loaded skill");
                            self.skills.insert(skill.name.clone(), skill);
                            loaded += 1;
                        }
                        Err(e) => warn!(path = %p.display(), error = %e, "skipped skill"),
                    }
                }
            }
        }
        Ok(loaded)
    }
}

/// Parse one skill file: YAML-style frontmatter wrapped in `---` lines,
/// followed by the body.
pub async fn parse_skill_file(path: &Path) -> Result<Skill, SkillError> {
    let raw = tokio::fs::read_to_string(path).await?;
    let (frontmatter, body) = split_frontmatter(&raw)
        .ok_or_else(|| SkillError::Frontmatter {
            path: path.display().to_string(),
            reason: "missing --- frontmatter".into(),
        })?;
    let mut skill = parse_frontmatter(frontmatter)
        .map_err(|e| SkillError::Frontmatter { path: path.display().to_string(), reason: e })?;
    skill.body = body.trim().to_string();
    skill.source = Some(path.display().to_string());
    Ok(skill)
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let trimmed = raw.trim_start();
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let body_start = end + 4; // past "\n---"
    let body = &rest[body_start..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some((frontmatter, body))
}

/// Minimal YAML subset parser.  Supports scalar fields, list of scalars,
/// and a list of mappings (used by `arguments`).  Sufficient for the
/// skill schema; switching to full `serde_yaml` is a one-line dependency
/// change if richer skills demand it.
fn parse_frontmatter(text: &str) -> Result<Skill, String> {
    let mut name = String::new();
    let mut description = String::new();
    let mut tags: Vec<String> = Vec::new();
    let mut requires_tools: Vec<String> = Vec::new();
    let mut arguments: Vec<SkillArgument> = Vec::new();

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') { i += 1; continue; }

        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = rest.trim().trim_matches('"').to_string();
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            description = rest.trim().trim_matches('"').to_string();
        } else if trimmed.starts_with("tags:") {
            tags = parse_inline_list(trimmed.trim_start_matches("tags:").trim());
        } else if trimmed.starts_with("requires_tools:") {
            requires_tools = parse_inline_list(trimmed.trim_start_matches("requires_tools:").trim());
        } else if trimmed == "arguments:" {
            // Parse a list of mappings until we hit a non-indented line.
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if !next.starts_with(' ') && !next.starts_with('\t') && !next.is_empty() { break; }
                let next_t = next.trim();
                if next_t.is_empty() { i += 1; continue; }
                if let Some(after) = next_t.strip_prefix("- name:") {
                    let arg_name = after.trim().trim_matches('"').to_string();
                    let mut arg = SkillArgument { name: arg_name, description: None, required: false };
                    i += 1;
                    while i < lines.len() {
                        let inner = lines[i];
                        if !inner.starts_with("    ") && !inner.starts_with("\t\t") { break; }
                        let inner_t = inner.trim();
                        if let Some(d) = inner_t.strip_prefix("description:") {
                            arg.description = Some(d.trim().trim_matches('"').to_string());
                        } else if let Some(r) = inner_t.strip_prefix("required:") {
                            arg.required = r.trim() == "true";
                        }
                        i += 1;
                    }
                    arguments.push(arg);
                    continue;
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    if name.is_empty() { return Err("missing `name:` field".into()); }
    if description.is_empty() { return Err("missing `description:` field".into()); }

    Ok(Skill { name, description, tags, requires_tools, arguments, body: String::new(), source: None })
}

fn parse_inline_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.trim_start_matches('[').trim_end_matches(']');
    s.split(',')
        .map(|t| t.trim().trim_matches('"').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_frontmatter_and_body() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.md");
        let content = r#"---
name: sap.skill.test
description: A test skill
tags: [test, demo]
requires_tools: [sap.docs.search]
arguments:
  - name: company_code
    description: BUKRS
    required: true
  - name: fiscal_period
    required: false
---

Investigate period close for {{company_code}}.
"#;
        tokio::fs::write(&path, content).await.unwrap();
        let skill = parse_skill_file(&path).await.unwrap();
        assert_eq!(skill.name, "sap.skill.test");
        assert_eq!(skill.tags, vec!["test", "demo"]);
        assert_eq!(skill.requires_tools, vec!["sap.docs.search"]);
        assert_eq!(skill.arguments.len(), 2);
        assert!(skill.arguments[0].required);
        assert!(skill.body.contains("Investigate period close"));
    }

    #[tokio::test]
    async fn scan_directory_loads_multiple_skills() {
        let tmp = tempfile::tempdir().unwrap();
        for (slug, body) in [("a", "A skill"), ("b", "B skill")] {
            let path = tmp.path().join(format!("{slug}.md"));
            let content = format!("---\nname: sap.skill.{slug}\ndescription: {body}\n---\n\nBody {slug}.\n");
            tokio::fs::write(&path, content).await.unwrap();
        }
        let mut reg = SkillRegistry::new();
        let n = reg.scan_paths(&[tmp.path().to_path_buf()]).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn render_substitutes_arguments() {
        let s = Skill {
            name: "x".into(), description: "y".into(),
            tags: vec![], requires_tools: vec![],
            arguments: vec![
                SkillArgument { name: "company_code".into(), description: None, required: true },
                SkillArgument { name: "optional".into(), description: None, required: false },
            ],
            body: "Code: {{company_code}}, Opt: {{optional}}".into(),
            source: None,
        };
        let mut args = HashMap::new();
        args.insert("company_code".into(), "1000".into());
        let out = s.render(&args);
        assert_eq!(out, "Code: 1000, Opt: ");
    }

    #[test]
    fn render_flags_missing_required() {
        let s = Skill {
            name: "x".into(), description: "y".into(),
            tags: vec![], requires_tools: vec![],
            arguments: vec![
                SkillArgument { name: "company_code".into(), description: None, required: true },
            ],
            body: "Code: {{company_code}}".into(),
            source: None,
        };
        let out = s.render(&HashMap::new());
        assert!(out.contains("<MISSING company_code>"));
    }
}
