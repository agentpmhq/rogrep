//! Golden snapshots: every fixture parses to a stable normalized
//! Conversation. Review changes with `cargo insta review`.

mod common;

use rogrep_model::AgentKind;

fn snap(kind: AgentKind, rel: &str) {
    let out = common::parse_fixture(kind, rel);
    let name = rel.replace('/', "__").replace(".jsonl", "");
    insta::assert_yaml_snapshot!(name, out.conversation);
}

#[test]
fn claude_basic_session() {
    snap(AgentKind::Claude, "claude/basic_session.jsonl");
}

#[test]
fn claude_edge_cases() {
    snap(AgentKind::Claude, "claude/edge_cases.jsonl");
}

#[test]
fn codex_session() {
    snap(AgentKind::Codex, "codex/session.jsonl");
}

#[test]
fn generic_openai_shape() {
    snap(AgentKind::Generic, "generic/openai_shape.jsonl");
}

#[test]
fn malformed_mixed() {
    snap(AgentKind::Claude, "malformed/mixed_valid_invalid.jsonl");
}
