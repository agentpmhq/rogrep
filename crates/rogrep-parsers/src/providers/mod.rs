//! Provider registry and path claiming.
//!
//! Registry order matters: more specific providers claim first (cowork
//! before claude — cowork transcripts contain `.claude/projects` in their
//! sandbox paths). Generic stays last as the catch-all for configured
//! extra roots.

pub mod claude;
pub mod codex;
pub mod cowork;
pub mod cursor;
pub mod generic;
pub mod grok;
pub mod hermes;
pub mod opencode;

use crate::driver::Provider;
use rogrep_model::AgentKind;

static COWORK: cowork::CoworkProvider = cowork::CoworkProvider;
static CLAUDE: claude::ClaudeProvider = claude::ClaudeProvider;
static CODEX: codex::CodexProvider = codex::CodexProvider;
static CURSOR: cursor::CursorProvider = cursor::CursorProvider;
static GROK: grok::GrokProvider = grok::GrokProvider;
static HERMES: hermes::HermesProvider = hermes::HermesProvider;
static OPENCODE: opencode::OpencodeProvider = opencode::OpencodeProvider;
static GENERIC: generic::GenericProvider = generic::GenericProvider;

static REGISTRY: [&dyn Provider; 8] = [
    &COWORK, &CLAUDE, &CODEX, &CURSOR, &GROK, &HERMES, &OPENCODE, &GENERIC,
];

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
        let cases = [
            ("/home/u/.claude/projects/-home-u/x.jsonl", AgentKind::Claude),
            ("/home/u/.codex/sessions/2026/01/01/r.jsonl", AgentKind::Codex),
            (
                "/home/u/.cursor/projects/home-u-src-x/agent-transcripts/a/a.jsonl",
                AgentKind::Cursor,
            ),
            (
                "/home/u/.grok/sessions/%2Fhome%2Fu/abc/chat_history.jsonl",
                AgentKind::Grok,
            ),
            (
                "/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/session/s1/.claude/projects/-p/x.jsonl",
                AgentKind::ClaudeCowork,
            ),
            ("/data/rogrep/spool/hermes/sess-1.jsonl", AgentKind::Hermes),
            ("/data/rogrep/spool/opencode/ses_x.jsonl", AgentKind::Opencode),
        ];
        for (path, kind) in cases {
            let p = provider_for_path(path).unwrap_or_else(|| panic!("no provider for {path}"));
            assert_eq!(p.kind(), kind, "{path}");
        }
        assert!(provider_for_path("/home/u/.claude/projects/x/history.jsonl").is_none());
        assert!(provider_for_path("/home/u/.cursor/projects/x/notes.jsonl").is_none());
        assert!(
            provider_for_path("/home/u/.grok/sessions/%2Fhome/prompt_history.jsonl").is_none(),
            "only chat_history.jsonl is a grok transcript"
        );
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
