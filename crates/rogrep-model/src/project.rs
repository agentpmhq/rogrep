//! Cross-provider project/cwd reconciliation.
//!
//! claude encodes cwds on disk as dash slugs (`/home/u/src/x` →
//! `-home-u-src-x`); codex/grok/hermes report a literal "home" fallback;
//! cursor's slug is missing its leading dash. `normalized_project` collapses
//! every agent working in the same directory to one key. The decode→encode
//! roundtrip is identity for well-formed claude slugs (a literal dash inside a
//! path segment like `ir-study` survives), so claude grouping never changes —
//! only the other providers get repaired. Port of agentpm
//! internal/agentserver/conversations.go:6517-6660.

/// Decode a dash-encoded project slug to a slash path. Lossy on purpose
/// (literal dashes decode to slashes) — only used where the roundtrip
/// property makes that safe. Returns "" when the slug is not decodable.
pub fn slash_path_from_dash_project(project: &str) -> String {
    let mut project = project.trim().trim_end_matches(".jsonl").to_string();
    if project.is_empty() || project == "home" {
        return String::new();
    }
    // cursor drops the leading dash on common absolute roots.
    if project.starts_with("home-")
        || project.starts_with("Users-")
        || project.starts_with("private-var-")
        || project.starts_with("var-")
    {
        project = format!("-{project}");
    }
    if !project.starts_with('-') {
        return String::new();
    }
    let parts: Vec<&str> = project.split('-').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    format!("/{}", parts.join("/"))
}

/// Encode an absolute cwd using claude's on-disk convention.
pub fn dash_project_from_cwd(cwd: &str) -> String {
    let cwd = cwd.trim();
    if cwd.is_empty() || !cwd.starts_with('/') {
        return String::new();
    }
    cwd.replace('/', "-")
}

/// The cross-agent project key.
pub fn normalized_project(project: &str, cwd: &str, agent: crate::AgentKind) -> String {
    use crate::AgentKind::*;
    let project = project.trim();
    if project.starts_with('-') && matches!(agent, Claude | ClaudeCowork) {
        return project.to_string();
    }
    if !project.is_empty() && project != "home" {
        let decoded = slash_path_from_dash_project(project);
        if !decoded.is_empty() {
            let enc = dash_project_from_cwd(&decoded);
            if !enc.is_empty() {
                return enc;
            }
        }
        if project.starts_with('-') {
            return project.to_string();
        }
    }
    let cwd = cwd.trim();
    if !cwd.is_empty() {
        let enc = dash_project_from_cwd(&clean_path(cwd));
        if !enc.is_empty() {
            return enc;
        }
    }
    "home".to_string()
}

/// Minimal lexical path clean (collapse `//`, drop trailing `/`, resolve `.`).
fn clean_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    if p.starts_with('/') {
        format!("/{}", out.join("/"))
    } else {
        out.join("/")
    }
}

/// Scrape `<cwd>…</cwd>` from injected context blocks (case-insensitive tags).
pub fn cwd_from_text(text: &str) -> Option<String> {
    let stripped = text.trim();
    let lower = stripped.to_lowercase();
    let start = lower.find("<cwd>")?;
    let body_start = start + "<cwd>".len();
    let end = lower[body_start..].find("</cwd>")?;
    let cwd = stripped[body_start..body_start + end].trim();
    if cwd.is_empty() {
        None
    } else {
        Some(cwd.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentKind;

    #[test]
    fn dash_roundtrip_is_identity_for_claude_slugs() {
        // Literal dash inside a segment survives the decode→encode roundtrip.
        let slug = "-home-ssorkin-src-ir-study";
        let decoded = slash_path_from_dash_project(slug);
        assert_eq!(decoded, "/home/ssorkin/src/ir/study"); // lossy decode…
        assert_eq!(dash_project_from_cwd(&decoded), slug); // …identity roundtrip
    }

    #[test]
    fn claude_slug_is_authoritative() {
        assert_eq!(
            normalized_project("-home-u-src-x", "/somewhere/else", AgentKind::Claude),
            "-home-u-src-x"
        );
    }

    #[test]
    fn home_fallback_repaired_from_cwd() {
        assert_eq!(
            normalized_project("home", "/home/u/src/x", AgentKind::Codex),
            "-home-u-src-x"
        );
        assert_eq!(
            normalized_project("", "/home/u/src/x/", AgentKind::Grok),
            "-home-u-src-x"
        );
    }

    #[test]
    fn cursor_missing_leading_dash_repaired() {
        assert_eq!(
            normalized_project("home-u-src-x", "", AgentKind::Cursor),
            "-home-u-src-x"
        );
    }

    #[test]
    fn all_agents_in_same_dir_collapse() {
        let cwd = "/home/u/src/proj";
        let claude = normalized_project("-home-u-src-proj", cwd, AgentKind::Claude);
        let codex = normalized_project("home", cwd, AgentKind::Codex);
        let hermes = normalized_project("home", cwd, AgentKind::Hermes);
        let cursor = normalized_project("home-u-src-proj", "", AgentKind::Cursor);
        assert_eq!(claude, codex);
        assert_eq!(claude, hermes);
        assert_eq!(claude, cursor);
    }

    #[test]
    fn no_information_yields_home() {
        assert_eq!(normalized_project("", "", AgentKind::Codex), "home");
        assert_eq!(normalized_project("home", "relative/x", AgentKind::Codex), "home");
    }

    #[test]
    fn cwd_scrape() {
        assert_eq!(
            cwd_from_text("<environment_context>\n<cwd>/home/u/x</cwd>\n</environment_context>"),
            Some("/home/u/x".to_string())
        );
        assert_eq!(cwd_from_text("no tags"), None);
        assert_eq!(cwd_from_text("<CWD>/a</CWD>"), Some("/a".to_string()));
    }
}
