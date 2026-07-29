//! Claude Cowork (macOS local agent mode): claude-format transcripts inside
//! a sandboxed session directory. Same parser as claude; the sandbox cwd
//! (`…/sessions/<id>/mnt/Project`) is repaired to a stable per-project key.

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::RawRecord;
use rogrep_model::AgentKind;

pub const COWORK_PARSER_VERSION: u32 = 1;

pub struct CoworkProvider;

impl Provider for CoworkProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCowork
    }

    fn parser_version(&self) -> u32 {
        COWORK_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        let p = path.replace('\\', "/");
        p.contains("/local-agent-mode-sessions/") && p.ends_with(".jsonl") && !p.ends_with("/audit.jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        let mut info = super::claude::source_info_for_claude_path(path);
        info.cwd_seed = info.cwd_seed.map(|c| repair_sandbox_cwd(&c));
        info
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(CoworkParser {
            inner: super::claude::ClaudeProvider.new_parser(),
        })
    }
}

struct CoworkParser {
    inner: Box<dyn RolloutParser>,
}

impl RolloutParser for CoworkParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        self.inner.process(rec, ctx);
        ctx.map_cwd(repair_sandbox_cwd);
    }

    fn finish(&mut self, amendable: &mut [rogrep_model::Turn]) {
        self.inner.finish(amendable);
    }

    fn export_state(&self) -> serde_json::Value {
        self.inner.export_state()
    }

    fn import_state(&mut self, state: &serde_json::Value) {
        self.inner.import_state(state);
    }
}

/// `/…/sessions/<uuid>/mnt/Project` → `/cowork/Project` — a stable
/// per-project key that groups the same Cowork project across sessions
/// (the real host path is not recoverable from inside the sandbox).
pub fn repair_sandbox_cwd(cwd: &str) -> String {
    let slash = cwd.replace('\\', "/");
    if let Some(idx) = slash.find("/sessions/") {
        let rest = &slash[idx + "/sessions/".len()..];
        if let Some(mnt) = rest.find("/mnt/") {
            let project_path = &rest[mnt + "/mnt/".len()..];
            if !project_path.is_empty() {
                return format!("/cowork/{project_path}");
            }
        }
    }
    cwd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_cwd_repair() {
        assert_eq!(
            repair_sandbox_cwd("/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/sessions/abc-123/mnt/MyProject"),
            "/cowork/MyProject"
        );
        assert_eq!(repair_sandbox_cwd("/home/u/src/x"), "/home/u/src/x");
    }
}
