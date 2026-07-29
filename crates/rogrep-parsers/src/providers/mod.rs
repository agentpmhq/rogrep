//! Provider registry and path claiming.
//!
//! Registry order matters: more specific providers claim first (cowork
//! before claude; generic last as the catch-all for configured extra roots).

pub mod claude;
pub mod codex;
pub mod generic;

use crate::driver::Provider;
use rogrep_model::AgentKind;

static CLAUDE: claude::ClaudeProvider = claude::ClaudeProvider;
static CODEX: codex::CodexProvider = codex::CodexProvider;
static GENERIC: generic::GenericProvider = generic::GenericProvider;

static REGISTRY: [&dyn Provider; 3] = [&CLAUDE, &CODEX, &GENERIC];

/// All registered providers, in claim order.
pub fn registry() -> &'static [&'static dyn Provider] {
    &REGISTRY
}

/// Resolve the provider owning a path (excluding the generic catch-all —
/// generic only applies to explicitly configured extra roots).
pub fn provider_for_path(path: &str) -> Option<&'static dyn Provider> {
    registry()
        .iter()
        .copied()
        .filter(|p| p.kind() != AgentKind::Generic)
        .find(|p| p.claims_path(path))
}

pub fn provider_for_kind(kind: AgentKind) -> Option<&'static dyn Provider> {
    registry().iter().copied().find(|p| p.kind() == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_claims() {
        let p = provider_for_path("/home/u/.claude/projects/-home-u/x.jsonl").unwrap();
        assert_eq!(p.kind(), AgentKind::Claude);
        let p = provider_for_path("/home/u/.codex/sessions/2026/01/01/r.jsonl").unwrap();
        assert_eq!(p.kind(), AgentKind::Codex);
        assert!(provider_for_path("/home/u/.claude/projects/x/history.jsonl").is_none());
        assert!(provider_for_path("/home/u/random.jsonl").is_none());
    }

    #[test]
    fn claude_subagent_paths_claimed() {
        let p = provider_for_path("/h/.claude/projects/-p/sess-1/subagents/agent-a.jsonl").unwrap();
        assert_eq!(p.kind(), AgentKind::Claude);
        let info = p.source_info("/h/.claude/projects/-p/sess-1/subagents/agent-a.jsonl");
        let sub = info.subagent.expect("subagent link");
        assert_eq!(sub.subagent_id.as_deref(), Some("agent-a"));
        assert_eq!(sub.parent_source_path.as_deref(), Some("/h/.claude/projects/-p/sess-1.jsonl"));
    }
}
