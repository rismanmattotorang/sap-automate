//! Minimal robots.txt parser + per-host cache.
//!
//! Implements the subset of RFC 9309 that real crawlers actually rely on:
//!   - `User-agent:` group selection with the most-specific match winning.
//!   - `Allow:` / `Disallow:` rules, longest-prefix-wins semantics.
//!   - `Crawl-delay:` for the token-bucket rate limiter in `rate_limit.rs`.
//!
//! Not implemented (because we do not need them for the SAP Help Portal +
//! Signavio + LeanIX sources we target): sitemap links, wildcard `*` /
//! end-anchor `$` patterns.  Easy to add behind a feature flag if a future
//! source demands it.
//!
//! Convergent with Crawl4AI's "stealth + respect" stance: production
//! crawlers must respect robots before doing anything else.

use std::collections::HashMap;
use std::time::Duration;

/// Result of `RobotsTxt::is_allowed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Disallowed,
}

/// One parsed robots.txt for one origin.
#[derive(Debug, Clone, Default)]
pub struct RobotsTxt {
    /// Groups keyed by user-agent string (lower-cased).  `*` is the wildcard.
    groups: HashMap<String, Group>,
}

#[derive(Debug, Clone, Default)]
struct Group {
    allow: Vec<String>,
    disallow: Vec<String>,
    crawl_delay: Option<Duration>,
}

impl RobotsTxt {
    /// Parse the body of a robots.txt response.  Tolerant: unknown
    /// directives and malformed lines are skipped, never errored, matching
    /// real-world parser behaviour.
    pub fn parse(body: &str) -> Self {
        let mut out = RobotsTxt::default();
        let mut current_agents: Vec<String> = Vec::new();
        let mut just_saw_directive = false;

        for line in body.lines() {
            let line = strip_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = match line.split_once(':') {
                Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim()),
                None => continue,
            };
            match key.as_str() {
                "user-agent" => {
                    // Adjacent user-agent lines form a group; if the
                    // previous line was a rule (Allow/Disallow/Crawl-delay),
                    // start a fresh group.
                    if just_saw_directive {
                        current_agents.clear();
                    }
                    current_agents.push(value.to_ascii_lowercase());
                    just_saw_directive = false;
                }
                "allow" | "disallow" | "crawl-delay" => {
                    for agent in &current_agents {
                        let g = out.groups.entry(agent.clone()).or_default();
                        match key.as_str() {
                            "allow" if !value.is_empty() => g.allow.push(value.to_string()),
                            "disallow" if !value.is_empty() => g.disallow.push(value.to_string()),
                            "crawl-delay" => {
                                if let Ok(secs) = value.parse::<f64>() {
                                    if secs.is_finite() && secs >= 0.0 {
                                        g.crawl_delay = Some(Duration::from_millis((secs * 1000.0) as u64));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    just_saw_directive = true;
                }
                _ => {} // Unknown directive — ignore.
            }
        }
        out
    }

    /// Decide whether `user_agent` may fetch `path`.
    pub fn is_allowed(&self, user_agent: &str, path: &str) -> Decision {
        let group = self.matching_group(user_agent);
        let allow_match = longest_prefix_match(group.map(|g| &g.allow), path);
        let disallow_match = longest_prefix_match(group.map(|g| &g.disallow), path);
        match (allow_match, disallow_match) {
            (None, None) => Decision::Allowed,
            (Some(_), None) => Decision::Allowed,
            (None, Some(_)) => Decision::Disallowed,
            // RFC 9309 §2.2.2: longest-prefix wins; tie → Allow.
            (Some(a), Some(d)) if a >= d => Decision::Allowed,
            (Some(_), Some(_)) => Decision::Disallowed,
        }
    }

    /// Get the crawl-delay (if any) for `user_agent`.  Used by the
    /// rate-limiter to honour the source's published cadence.
    pub fn crawl_delay(&self, user_agent: &str) -> Option<Duration> {
        self.matching_group(user_agent).and_then(|g| g.crawl_delay)
    }

    fn matching_group(&self, user_agent: &str) -> Option<&Group> {
        let ua = user_agent.to_ascii_lowercase();
        // Most-specific match: prefer an exact agent line, fall back to
        // any agent string that's a prefix of `ua` (longest wins), then
        // fall back to `*`.
        if let Some(g) = self.groups.get(&ua) {
            return Some(g);
        }
        let mut best: Option<(usize, &Group)> = None;
        for (agent, g) in &self.groups {
            if agent == "*" {
                continue;
            }
            if ua.starts_with(agent)
                && best.map(|(len, _)| agent.len() > len).unwrap_or(true)
            {
                best = Some((agent.len(), g));
            }
        }
        if let Some((_, g)) = best {
            return Some(g);
        }
        self.groups.get("*")
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn longest_prefix_match(rules: Option<&Vec<String>>, path: &str) -> Option<usize> {
    let rules = rules?;
    rules.iter()
        .filter(|r| path.starts_with(r.as_str()))
        .map(|r| r.len())
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# Sample robots.txt
User-agent: *
Disallow: /private/
Allow: /private/public/
Crawl-delay: 2

User-agent: sap-automate
Disallow: /admin/
Crawl-delay: 0.5
"#;

    #[test]
    fn wildcard_disallow_blocks_private() {
        let r = RobotsTxt::parse(SAMPLE);
        assert_eq!(r.is_allowed("OtherBot", "/private/foo"), Decision::Disallowed);
    }

    #[test]
    fn longest_prefix_allow_wins_over_disallow() {
        let r = RobotsTxt::parse(SAMPLE);
        assert_eq!(r.is_allowed("OtherBot", "/private/public/index.html"), Decision::Allowed);
    }

    #[test]
    fn specific_agent_group_overrides_wildcard() {
        let r = RobotsTxt::parse(SAMPLE);
        // sap-automate's group disallows /admin/, doesn't disallow /private/.
        // RFC 9309: once the more-specific group is selected, the * group
        // is ignored entirely.
        assert_eq!(r.is_allowed("sap-automate", "/private/foo"), Decision::Allowed);
        assert_eq!(r.is_allowed("sap-automate", "/admin/x"), Decision::Disallowed);
    }

    #[test]
    fn crawl_delay_is_parsed_per_agent() {
        let r = RobotsTxt::parse(SAMPLE);
        assert_eq!(r.crawl_delay("OtherBot"), Some(Duration::from_secs(2)));
        assert_eq!(r.crawl_delay("sap-automate"), Some(Duration::from_millis(500)));
    }

    #[test]
    fn unrelated_directives_are_ignored() {
        let body = "User-agent: *\nSitemap: https://example.com/sitemap.xml\nDisallow: /x\n";
        let r = RobotsTxt::parse(body);
        assert_eq!(r.is_allowed("Bot", "/x"), Decision::Disallowed);
        assert_eq!(r.is_allowed("Bot", "/y"), Decision::Allowed);
    }

    #[test]
    fn empty_robots_means_allow_all() {
        let r = RobotsTxt::parse("");
        assert_eq!(r.is_allowed("any", "/anything"), Decision::Allowed);
    }

    #[test]
    fn comments_are_stripped() {
        let body = "User-agent: * # match all\nDisallow: /x # private\n";
        let r = RobotsTxt::parse(body);
        assert_eq!(r.is_allowed("a", "/x"), Decision::Disallowed);
    }
}
