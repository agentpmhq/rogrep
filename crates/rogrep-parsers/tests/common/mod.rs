#![allow(dead_code)] // shared across test binaries; not all use every helper

use rogrep_parsers::driver::{parse_from, DriverOutput, Provider};
use rogrep_parsers::state::ParseState;
use rogrep_model::AgentKind;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn fixture_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(rel)
}

/// Canonical fake source path per provider so ids/projects are stable in
/// snapshots regardless of where the checkout lives.
pub fn canonical_path(kind: AgentKind, name: &str) -> String {
    match kind {
        AgentKind::Claude => format!("/home/u/.claude/projects/-home-u-src-proj/{name}"),
        AgentKind::Codex => format!("/home/u/.codex/sessions/2026/07/03/{name}"),
        AgentKind::Cursor => {
            format!("/home/u/.cursor/projects/home-u-src-proj/agent-transcripts/{name}")
        }
        AgentKind::Grok => {
            format!("/home/u/.grok/sessions/%2Fhome%2Fu%2Fsrc%2Fproj/sess-1/{name}")
        }
        AgentKind::Hermes => format!("/data/rogrep/spool/hermes/{name}"),
        AgentKind::Opencode => format!("/data/rogrep/spool/opencode/{name}"),
        AgentKind::ClaudeCowork => format!(
            "/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/session/s1/.claude/projects/-p/{name}"
        ),
        _ => format!("/home/u/logs/{name}"),
    }
}

pub fn provider_for(kind: AgentKind) -> &'static dyn Provider {
    rogrep_parsers::provider_for_kind(kind).expect("provider registered")
}

/// Parse arbitrary bytes as if they lived at the canonical path.
pub fn parse_bytes(
    kind: AgentKind,
    name: &str,
    bytes: &[u8],
    seed: Option<ParseState>,
) -> DriverOutput {
    let provider = provider_for(kind);
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(bytes).unwrap();
    tmp.flush().unwrap();
    let info = provider.source_info(&canonical_path(kind, name));
    let mut file = tmp.reopen().unwrap();
    let state = match seed {
        Some(s) => s,
        None => ParseState::fresh(provider.parser_version()),
    };
    parse_from(provider, &info, &mut file, state).unwrap()
}

pub fn parse_fixture(kind: AgentKind, rel: &str) -> DriverOutput {
    let bytes = std::fs::read(fixture_path(rel)).unwrap();
    let name = Path::new(rel).file_name().unwrap().to_string_lossy().to_string();
    parse_bytes(kind, &name, &bytes, None)
}
