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
//!
//! rogrep extension beyond agentpm: an unquoted `/pattern/` token is a
//! regular expression matched against full turn text (case-sensitive;
//! `(?i)` opts into case-insensitivity), and a facet value may be
//! `/pattern/` to regex-match the facet's indexed vocabulary.

use serde::Serialize;

/// Facet keys rogrep understands. Multi-tenant keys from agentpm (org,
/// owner, agent_id) are gone; date facets are handled separately below.
pub const KNOWN_FACET_KEYS: &[&str] = &[
    "is",
    "origin",
    "subagent",
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

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ParsedQuery {
    /// AND'ed bare terms (lowercased).
    pub terms: Vec<String>,
    /// Quoted phrases (must match in order).
    pub phrases: Vec<String>,
    /// key → values (values may contain */? wildcards).
    pub facets: Vec<(String, String)>,
    /// Date constraints (key, raw value) for the caller to resolve.
    pub dates: Vec<(String, String)>,
    /// AND'ed `/pattern/` regexes over full turn text (case preserved).
    pub regexes: Vec<String>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.phrases.is_empty()
            && self.facets.is_empty()
            && self.regexes.is_empty()
    }

    /// All text matchers for excerpt highlighting.
    pub fn highlight_terms(&self) -> Vec<String> {
        let mut out = self.terms.clone();
        out.extend(self.phrases.clone());
        out
    }

    /// Compile the `/pattern/` regexes. Size-limited so pathological
    /// patterns error instead of exhausting memory.
    pub fn compile_regexes(&self) -> anyhow::Result<Vec<regex::Regex>> {
        self.regexes
            .iter()
            .map(|pat| {
                regex::RegexBuilder::new(pat)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| anyhow::anyhow!("invalid regex /{pat}/: {e}"))
            })
            .collect()
    }
}

/// The inner pattern of a `/pattern/`-shaped token, if it is one.
pub fn regex_token(token: &str) -> Option<&str> {
    let body = token.strip_prefix('/')?.strip_suffix('/')?;
    (!body.is_empty()).then_some(body)
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
    let mut parsed = ParsedQuery::default();
    for token in split_query(query) {
        if token.quoted {
            let t = token.text.trim();
            if !t.is_empty() {
                parsed.phrases.push(t.to_lowercase());
            }
            continue;
        }
        if let Some(pat) = regex_token(&token.text) {
            parsed.regexes.push(pat.to_string());
            continue;
        }
        if let Some((key, value)) = token.text.split_once(':') {
            if let Some(canonical) = is_known_facet_key(key) {
                // Leading @ is user-name sugar in agentpm; strip everywhere.
                let value = value.trim();
                let value = value.strip_prefix('@').unwrap_or(value);
                if !value.is_empty() {
                    if DATE_FACET_KEYS.contains(&canonical) {
                        parsed.dates.push((canonical.to_string(), value.to_string()));
                    } else if regex_token(value).is_some() {
                        // Regex facet values stay verbatim — normalization
                        // would mangle the pattern.
                        parsed.facets.push((canonical.to_string(), value.to_string()));
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
        "tool_status" | "content" | "is" | "origin" | "subagent" | "provider" | "agent" | "role" => {
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
            c => re.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
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

    #[test]
    fn regex_tokens_extracted_case_preserved() {
        let q = parse_query("bug /Err.*Kind/ tool:bash");
        assert_eq!(q.terms, vec!["bug"]);
        assert_eq!(q.regexes, vec!["Err.*Kind"]);
        assert_eq!(q.facets, vec![("tool".into(), "bash".into())]);
        assert!(!q.is_empty());
    }

    #[test]
    fn pure_regex_query_is_not_empty() {
        let q = parse_query("/panic!/");
        assert_eq!(q.regexes, vec!["panic!"]);
        assert!(!q.is_empty());
    }

    #[test]
    fn escaped_slash_body_preserved() {
        let q = parse_query(r"/src\/lib/");
        assert_eq!(q.regexes, vec![r"src\/lib"]);
        assert!(q.compile_regexes().is_ok());
    }

    #[test]
    fn quoted_regex_stays_phrase() {
        let q = parse_query("\"/x.*y/\"");
        assert!(q.regexes.is_empty());
        assert_eq!(q.phrases, vec!["/x.*y/"]);
    }

    #[test]
    fn unterminated_or_empty_slashes_stay_terms() {
        let q = parse_query("/foo // bar/");
        assert!(q.regexes.is_empty());
        assert_eq!(q.terms, vec!["/foo", "//", "bar/"]);
    }

    #[test]
    fn invalid_regex_fails_at_compile_not_parse() {
        let q = parse_query("/te(st/");
        assert_eq!(q.regexes, vec!["te(st"]);
        let err = q.compile_regexes().unwrap_err();
        assert!(err.to_string().contains("invalid regex /te(st/"));
    }

    #[test]
    fn facet_regex_value_kept_verbatim() {
        // Normalization would turn `_` into `-`; regex values must not be
        // touched.
        let q = parse_query("tool_status:/fail_.*/ file:/.*_test\\.rs/");
        assert_eq!(
            q.facets,
            vec![
                ("tool_status".into(), "/fail_.*/".into()),
                ("file".into(), "/.*_test\\.rs/".into())
            ]
        );
    }

    #[test]
    fn at_prefix_stripped_from_facet_values() {
        let q = parse_query("project:@rogrep");
        assert_eq!(q.facets, vec![("project".into(), "rogrep".into())]);
    }

    #[test]
    fn subagent_values_normalize() {
        let q = parse_query("subagent:TRUE");
        assert_eq!(q.facets, vec![("subagent".into(), "true".into())]);
    }
}
