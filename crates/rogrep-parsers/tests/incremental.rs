//! The golden invariant: `parse(full) == parse(prefix) ⊕ resume(tail)` at
//! every line boundary (and at arbitrary byte splits, where the partial
//! line is transient garbage that resolves once the file grows).

mod common;

use pretty_assertions::assert_eq;
use rogrep_model::{AgentKind, Turn};

fn line_boundaries(bytes: &[u8]) -> Vec<usize> {
    let mut out = vec![0];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            out.push(i + 1);
        }
    }
    if *out.last().unwrap() != bytes.len() {
        out.push(bytes.len());
    }
    out
}

fn merged_turns(prefix_turns: &[Turn], replace_from_2: u32, tail_turns: &[Turn]) -> Vec<Turn> {
    let mut merged: Vec<Turn> = prefix_turns
        .iter()
        .filter(|t| t.turn_index < replace_from_2)
        .cloned()
        .collect();
    merged.extend(tail_turns.iter().cloned());
    merged
}

fn check_all_splits(kind: AgentKind, rel: &str) {
    let bytes = std::fs::read(common::fixture_path(rel)).unwrap();
    let name = "split-test.jsonl";
    let full = common::parse_bytes(kind, name, &bytes, None);

    for &split in &line_boundaries(&bytes) {
        // Run 1: parse the prefix as if that's all that exists yet.
        let run1 = common::parse_bytes(kind, name, &bytes[..split], None);
        // Run 2: the file has grown to its final size; resume from run 1's
        // checkpoint.
        let run2 = common::parse_bytes(kind, name, &bytes, Some(run1.state.clone()));

        let merged = merged_turns(&run1.conversation.turns, run2.replace_from, &run2.conversation.turns);
        assert_eq!(
            full.conversation.turns, merged,
            "turn mismatch at split {split} for {rel}"
        );
        // Summary equality: run 2's summary reflects the whole file.
        let mut got = run2.conversation.clone();
        let mut want = full.conversation.clone();
        got.turns.clear();
        want.turns.clear();
        assert_eq!(want, got, "summary mismatch at split {split} for {rel}");
        // Checkpoint equivalence: resuming from either path lands on the
        // same watermark.
        assert_eq!(
            full.state, run2.state,
            "state mismatch at split {split} for {rel}"
        );
    }
}

#[test]
fn claude_basic_all_line_splits() {
    check_all_splits(AgentKind::Claude, "claude/basic_session.jsonl");
}

#[test]
fn claude_edge_cases_all_line_splits() {
    check_all_splits(AgentKind::Claude, "claude/edge_cases.jsonl");
}

#[test]
fn codex_all_line_splits() {
    check_all_splits(AgentKind::Codex, "codex/session.jsonl");
}

#[test]
fn generic_all_line_splits() {
    check_all_splits(AgentKind::Generic, "generic/openai_shape.jsonl");
}

#[test]
fn malformed_all_line_splits() {
    check_all_splits(AgentKind::Claude, "malformed/mixed_valid_invalid.jsonl");
}

/// Arbitrary byte splits: run 1 sees a torn final line; once the file grows,
/// resume must converge to the full parse.
#[test]
fn byte_splits_converge() {
    for rel in ["claude/basic_session.jsonl", "codex/session.jsonl"] {
        let bytes = std::fs::read(common::fixture_path(rel)).unwrap();
        let kind = if rel.starts_with("claude") {
            AgentKind::Claude
        } else {
            AgentKind::Codex
        };
        let full = common::parse_bytes(kind, "byte-split.jsonl", &bytes, None);
        let step = (bytes.len() / 23).max(1);
        for split in (1..bytes.len()).step_by(step) {
            let run1 = common::parse_bytes(kind, "byte-split.jsonl", &bytes[..split], None);
            let run2 = common::parse_bytes(kind, "byte-split.jsonl", &bytes, Some(run1.state.clone()));
            let merged = merged_turns(&run1.conversation.turns, run2.replace_from, &run2.conversation.turns);
            assert_eq!(full.conversation.turns, merged, "byte split {split} in {rel}");
            assert_eq!(
                full.conversation.malformed_lines, run2.conversation.malformed_lines,
                "malformed count at byte split {split} in {rel}"
            );
        }
    }
}

/// Three-stage growth: parse, extend, extend again.
#[test]
fn multi_stage_growth() {
    let bytes = std::fs::read(common::fixture_path("claude/basic_session.jsonl")).unwrap();
    let cuts = line_boundaries(&bytes);
    let a = cuts[cuts.len() / 3];
    let b = cuts[2 * cuts.len() / 3];
    let full = common::parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes, None);

    let r1 = common::parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes[..a], None);
    let r2 = common::parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes[..b], Some(r1.state.clone()));
    let r3 = common::parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes, Some(r2.state.clone()));

    let m12 = merged_turns(&r1.conversation.turns, r2.replace_from, &r2.conversation.turns);
    let m123 = merged_turns(&m12, r3.replace_from, &r3.conversation.turns);
    assert_eq!(full.conversation.turns, m123);
    assert_eq!(full.state, r3.state);
}
