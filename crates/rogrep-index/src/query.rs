//! The query grammar — a direct port of agentpm's tokenizer semantics
//! (conversations.go:5795-5990):
//!
//! - whitespace tokenization honoring "double quoted phrases";
//! - `key:value` is a facet only when `key` is in the known set — anything
//!   else with a colon (URLs, `data:` URIs, timecodes) is a literal term;
//! - quoted tokens are NEVER facets;
//! - `*`/`?` wildcards allowed in facet values;
//! - unterminated quotes flush as a quoted term so nothing is lost;
//! - bare text terms are lowercased, trimmed of punctuation, and dropped
//!   when shorter than 2 chars; quoted phrases survive whole.

use serde::Serialize;

/// Facet keys rogrep understands. Multi-tenant keys from agentpm (org,
/// owner, agent_id) are gone; date facets are handled separately below.
pub const KNOWN_FACET_KEYS: &[&str] = &[
    "is",
    "provider",
    "agent",
    "model",
    "project",
    "cwd",
    "file",
    "content",
    "role",
    "tool",
    "skill",
    "mcp",
    "tool_cmd",
    "tool_status",
    "tool_mutating",
    "git_cmd",
    "git_pr",
    "git_pr_num",
    "git_commit",
    "git_branch",
    "git_remote",
];

/// Date-range pseudo-facets resolved to a ts range, not term queries.
pub const DATE_FACET_KEYS: &[&str] = &["before", "after", "since", "until", "when"];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParsedQuery {
    /// AND'ed bare terms (lowercased).
    pub terms: Vec<String>,
    /// Quoted phrases (must match in order).
    pub phrases: Vec<String>,
    /// key → values (values may contain */? wildcards).
    pub facets: Vec<(String, String)>,
    /// Date constraints (key, raw value) for the caller to resolve.
    pub dates: Vec<(String, String)>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.phrases.is_empty() && self.facets.is_empty()
    }

    /// All text matchers for excerpt highlighting.
    pub fn highlight_terms(&self) -> Vec<String> {
        let mut out = self.terms.clone();
        out.extend(self.phrases.clone());
        out
    }
}

struct RawToken {
    text: String,
    quoted: bool,
}

/// Whitespace split honoring double quotes. Unterminated quote → flushed as
/// quoted.
fn split_query(query: &str) -> Vec<RawToken> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let flush = |current: &mut String, quoted: bool, out: &mut Vec<RawToken>| {
        if !current.is_empty() {
            out.push(RawToken {
                text: std::mem::take(current),
                quoted,
            });
        }
    };
    for c in query.chars() {
        match c {
            '"' => {
                if quoted {
                    flush(&mut current, true, &mut out);
                    quoted = false;
                } else {
                    flush(&mut current, false, &mut out);
                    quoted = true;
                }
            }
            c if c.is_whitespace() && !quoted => flush(&mut current, false, &mut out),
            c => current.push(c),
        }
    }
    flush(&mut current, quoted, &mut out);
    out
}

fn is_known_facet_key(key: &str) -> Option<&'static str> {
    let normalized = key.trim().to_lowercase().replace('-', "_");
    KNOWN_FACET_KEYS
        .iter()
        .chain(DATE_FACET_KEYS.iter())
        .find(|k| **k == normalized)
        .copied()
}

/// Trim leading/trailing punctuation from a bare term (agentpm textTerms).
fn trim_term(term: &str) -> String {
    term.trim_matches(|c: char| !c.is_alphanumeric() && c != '*' && c != '?' && c != '/' && c != '.' && c != '_' && c != '-')
        .to_lowercase()
}

pub fn parse_query(query: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery {
        terms: Vec::new(),
        phrases: Vec::new(),
        facets: Vec::new(),
        dates: Vec::new(),
    };
    for token in split_query(query) {
        if token.quoted {
            let t = token.text.trim();
            if !t.is_empty() {
                parsed.phrases.push(t.to_lowercase());
            }
            continue;
        }
        if let Some((key, value)) = token.text.split_once(':') {
            if let Some(canonical) = is_known_facet_key(key) {
                let value = value.trim();
                if !value.is_empty() {
                    if DATE_FACET_KEYS.contains(&canonical) {
                        parsed.dates.push((canonical.to_string(), value.to_string()));
                    } else {
                        parsed
                            .facets
                            .push((canonical.to_string(), normalize_facet_value(canonical, value)));
                    }
                    continue;
                }
            }
            // Unknown key (or empty value): the whole token is a literal term.
        }
        let t = trim_term(&token.text);
        if t.chars().count() >= 2 {
            parsed.terms.push(t);
        }
    }
    parsed
}

/// Facet value normalization: `_`→`-` for status-like values, short-sha
/// truncation for commits (matches the indexed vocabulary).
fn normalize_facet_value(key: &str, value: &str) -> String {
    let v = value.trim();
    match key {
        "git_commit" => {
            let v = v.to_lowercase();
            if v.len() > 7 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                v[..7].to_string()
            } else {
                v
            }
        }
        "tool" | "tool_cmd" | "skill" | "mcp" => v.to_lowercase(),
        "tool_status" | "content" | "is" | "provider" | "agent" | "role" => {
            v.to_lowercase().replace('_', "-").replace("--", "-")
        }
        _ => v.to_string(),
    }
}

/// Compile a facet value with `*`/`?` wildcards to an anchored regex, or
/// None when it has no wildcards (exact term match).
pub fn facet_glob_regex(value: &str) -> Option<String> {
    if !value.contains('*') && !value.contains('?') {
        return None;
    }
    let mut re = String::from("(?s)");
    for c in value.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c if "\\.+()[]{}|^$".contains(c) => {
                re.push('\\');
                re.push(c);
            }
            c => re.push(c),
        }
    }
    Some(re)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_terms_and_facets() {
        let q = parse_query("flaky test tool:bash tool_status:failed");
        assert_eq!(q.terms, vec!["flaky", "test"]);
        assert_eq!(
            q.facets,
            vec![
                ("tool".into(), "bash".into()),
                ("tool_status".into(), "failed".into())
            ]
        );
    }

    #[test]
    fn unknown_colon_token_is_literal() {
        let q = parse_query("https://github.com/x/y/pull/48 data:image/png");
        assert!(q.facets.is_empty());
        assert_eq!(q.terms.len(), 2);
        assert!(q.terms[0].contains("github.com"));
    }

    #[test]
    fn quoted_never_facet() {
        let q = parse_query("\"tool:bash\" \"exact phrase here\"");
        assert!(q.facets.is_empty());
        assert_eq!(q.phrases, vec!["tool:bash", "exact phrase here"]);
    }

    #[test]
    fn unterminated_quote_flushes() {
        let q = parse_query("start \"never closed phrase");
        assert_eq!(q.terms, vec!["start"]);
        assert_eq!(q.phrases, vec!["never closed phrase"]);
    }

    #[test]
    fn short_terms_dropped_punctuation_trimmed() {
        let q = parse_query("a (rust) x!");
        assert_eq!(q.terms, vec!["rust"]);
    }

    #[test]
    fn date_facets_split_out() {
        let q = parse_query("bug since:2026-07-01 before:2026-08-01");
        assert_eq!(q.terms, vec!["bug"]);
        assert_eq!(q.dates.len(), 2);
        assert!(q.facets.is_empty());
    }

    #[test]
    fn dash_keys_normalize() {
        let q = parse_query("tool-status:failed");
        assert_eq!(q.facets, vec![("tool_status".into(), "failed".into())]);
    }

    #[test]
    fn commit_shas_shorten() {
        let q = parse_query("git_commit:0123456789abcdef");
        assert_eq!(q.facets, vec![("git_commit".into(), "0123456".into())]);
    }

    #[test]
    fn glob_compilation() {
        assert_eq!(facet_glob_regex("plain"), None);
        assert_eq!(facet_glob_regex("*.rs"), Some("(?s).*\\.rs".into()));
        assert_eq!(facet_glob_regex("a?c"), Some("(?s)a.c".into()));
    }
}
