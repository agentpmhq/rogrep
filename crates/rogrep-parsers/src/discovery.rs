//! Source-file discovery: walk the fixed provider roots under a home
//! directory (plus configured extra roots) and classify each JSONL file.

use crate::providers;
use rogrep_model::AgentKind;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub path: PathBuf,
    pub kind: AgentKind,
    pub size: u64,
    /// Modification time in unix nanoseconds.
    pub mtime_ns: i128,
}

/// Provider roots relative to a home directory.
pub fn provider_roots(home: &Path) -> Vec<(PathBuf, AgentKind)> {
    let mut roots = vec![
        (home.join(".claude/projects"), AgentKind::Claude),
        (home.join(".codex/sessions"), AgentKind::Codex),
        (home.join(".cursor/projects"), AgentKind::Cursor),
        (home.join(".grok/sessions"), AgentKind::Grok),
        (home.join(".grok/logs"), AgentKind::Grok),
    ];
    if cfg!(target_os = "macos") {
        roots.push((
            home.join("Library/Application Support/Claude/local-agent-mode-sessions"),
            AgentKind::ClaudeCowork,
        ));
    }
    roots
}

/// Walk all provider roots and extra generic roots. Files are classified by
/// the provider registry; unclaimed files under provider roots are skipped,
/// files under extra roots fall back to the generic parser.
pub fn discover_files(home: &Path, extra_roots: &[PathBuf]) -> Vec<DiscoveredFile> {
    let mut out = Vec::new();
    for (root, _kind) in provider_roots(home) {
        walk(&root, &mut |path, size, mtime_ns| {
            let s = path.to_string_lossy();
            if !s.ends_with(".jsonl") || should_skip(&s) {
                return;
            }
            if let Some(provider) = providers::provider_for_path(&s) {
                out.push(DiscoveredFile {
                    path: path.to_path_buf(),
                    kind: provider.kind(),
                    size,
                    mtime_ns,
                });
            }
        });
    }
    for root in extra_roots {
        walk(root, &mut |path, size, mtime_ns| {
            let s = path.to_string_lossy();
            if !s.ends_with(".jsonl") {
                return;
            }
            let kind = providers::provider_for_path(&s)
                .map(|p| p.kind())
                .unwrap_or(AgentKind::Generic);
            out.push(DiscoveredFile {
                path: path.to_path_buf(),
                kind,
                size,
                mtime_ns,
            });
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Skip rules (agentpm parity): history.jsonl everywhere; cursor keeps only
/// agent-transcripts; grok logs only top-level (enforced by depth guard in
/// walk callers is overkill here — path check suffices); cowork audit logs.
fn should_skip(path: &str) -> bool {
    if path.ends_with("/history.jsonl") || path.ends_with("/audit.jsonl") {
        return true;
    }
    if path.contains("/.cursor/projects/") && !path.contains("/agent-transcripts/") {
        return true;
    }
    false
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path, u64, i128)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk(&path, f);
        } else if meta.is_file() {
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(0);
            f(&path, meta.len(), mtime_ns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovery_walks_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let proj = home.join(".claude/projects/-home-u-src-x");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("a.jsonl"), "{}\n").unwrap();
        fs::write(proj.join("history.jsonl"), "{}\n").unwrap();
        fs::write(proj.join("notes.txt"), "x").unwrap();
        let codex = home.join(".codex/sessions/2026/01/02");
        fs::create_dir_all(&codex).unwrap();
        fs::write(codex.join("rollout-x.jsonl"), "{}\n").unwrap();

        let found = discover_files(home, &[]);
        let names: Vec<String> = found
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.jsonl".to_string()));
        assert!(names.contains(&"rollout-x.jsonl".to_string()));
        assert!(!names.contains(&"history.jsonl".to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
    }

    #[test]
    fn extra_roots_use_generic() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tmp.path().join("mylogs");
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("x.jsonl"), "{}\n").unwrap();
        let found = discover_files(tmp.path(), &[extra]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, AgentKind::Generic);
    }
}
