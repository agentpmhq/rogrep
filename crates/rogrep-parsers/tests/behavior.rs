//! Semantic assertions on parsed fixtures — the provider-trap checklist.

mod common;

use rogrep_model::{
    build_exchanges, AgentKind, Role, SpecialTurn, ToolDirection, ToolStatus,
};

#[test]
fn claude_tool_pairing_and_error_status() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/basic_session.jsonl");
    let turns = &out.conversation.turns;
    let tool_use = turns
        .iter()
        .find(|t| t.tool.as_ref().is_some_and(|i| i.direction == Some(ToolDirection::Use)))
        .expect("tool use turn");
    let tool_result = turns
        .iter()
        .find(|t| t.tool.as_ref().is_some_and(|i| i.direction == Some(ToolDirection::Output)))
        .expect("tool result turn");
    assert_eq!(
        tool_use.tool.as_ref().unwrap().pair_id,
        tool_result.tool.as_ref().unwrap().pair_id
    );
    assert_eq!(tool_result.tool.as_ref().unwrap().status, ToolStatus::Failed);
    assert_eq!(tool_use.speaker, "Bash");
}

#[test]
fn claude_usage_counted_once_per_message_id() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/basic_session.jsonl");
    // msg_01 spans two records (text block + tool_use block) with identical
    // usage; it must be counted once: totals = msg_01 + msg_02 + msg_03.
    let t = out.conversation.tokens;
    assert_eq!(t.input, 10 + 20 + 5);
    assert_eq!(t.output, 50 + 30 + 12);
    assert_eq!(t.cache_read, 1000 + 1400 + 1500);
    assert_eq!(t.cache_creation, 200);
}

#[test]
fn claude_ai_title_wins() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/basic_session.jsonl");
    assert_eq!(out.conversation.title.as_deref(), Some("Fix flaky reader offset test"));
}

#[test]
fn claude_cwd_first_record_wins_and_drift_ignored() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    // Conversation cwd from the first record cwd; the later
    // /home/u/src/other cd drift must not move it.
    assert_eq!(out.conversation.cwd.as_deref(), Some("/home/u/src/proj"));
    // But per-turn cwd tracks the drift.
    let last = out.conversation.turns.last().unwrap();
    assert_eq!(last.cwd.as_deref(), Some("/home/u/src/other"));
}

#[test]
fn claude_model_first_wins_but_turns_track() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    assert_eq!(out.conversation.model.as_deref(), Some("claude-fable-5"));
    let last = out.conversation.turns.last().unwrap();
    assert_eq!(last.model.as_deref(), Some("claude-opus-5"));
}

#[test]
fn claude_attachments_synthetic_and_bookkeeping_dropped() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    let turns = &out.conversation.turns;
    let ide = turns.iter().find(|t| t.speaker == "ide selection").expect("ide attachment");
    assert!(ide.synthetic_context);
    assert!(ide.text.contains("src/lib.rs:10-20"));
    // file-history-snapshot attachment must NOT produce a turn.
    assert!(!turns.iter().any(|t| t.text.contains("snapshotId")));
}

#[test]
fn task_notification_queued_and_delivered_distinct() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    let notifs: Vec<&SpecialTurn> = out
        .conversation
        .turns
        .iter()
        .filter_map(|t| t.special.as_ref())
        .filter(|s| matches!(s, SpecialTurn::TaskNotification { .. }))
        .collect();
    assert_eq!(notifs.len(), 2);
    let queued: Vec<bool> = notifs
        .iter()
        .map(|s| match s {
            SpecialTurn::TaskNotification { queued, .. } => *queued,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(queued, vec![true, false]);
}

#[test]
fn exchange_boundaries_respect_specials() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    let exchanges = build_exchanges(&out.conversation.turns);
    // Real prompts: "start the work" and "now change directory work".
    // The injected reminder opens the leading orphan card; task
    // notifications / meta caveat / interrupt / compaction never open one.
    let with_user: Vec<_> = exchanges.iter().filter(|e| e.user_turn_index.is_some()).collect();
    assert_eq!(with_user.len(), 2, "exchanges: {exchanges:#?}");
    assert!(with_user[0].user_preview.starts_with("start the work"));
    assert!(with_user[0].signals.interrupted);
    assert!(with_user[0].signals.compacted);
}

#[test]
fn claude_interrupt_points_at_open_exchange_prompt() {
    let out = common::parse_fixture(AgentKind::Claude, "claude/edge_cases.jsonl");
    let abort = out
        .conversation
        .turns
        .iter()
        .find_map(|t| match &t.special {
            Some(SpecialTurn::TurnAborted { aborted_user_turn, .. }) => Some(*aborted_user_turn),
            _ => None,
        })
        .expect("abort turn present");
    let user_turn = abort.expect("abort resolved to a user turn");
    assert!(out.conversation.turns[user_turn as usize].text.starts_with("start the work"));
}

#[test]
fn codex_cumulative_usage_becomes_deltas() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    let t = out.conversation.tokens;
    // Final cumulative totals: input 1500 (incl. 1000 cached) → 500 raw
    // input + 1000 cache_read, output 140, reasoning 55.
    assert_eq!(t.input, 500);
    assert_eq!(t.cache_read, 1000);
    assert_eq!(t.output, 140);
    assert_eq!(t.reasoning_output, 55);
    // Deltas attached to assistant turns, split across the two events.
    let assistant_totals: u64 = out
        .conversation
        .turns
        .iter()
        .filter(|x| x.role == Role::Assistant)
        .map(|x| x.tokens.output)
        .sum();
    assert_eq!(assistant_totals, 140);
}

#[test]
fn codex_exit_code_mined_from_output() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    let result = out
        .conversation
        .turns
        .iter()
        .find(|t| t.tool.as_ref().is_some_and(|i| i.direction == Some(ToolDirection::Output) && i.name == "tool_result"))
        .expect("tool result");
    let info = result.tool.as_ref().unwrap();
    assert_eq!(info.exit_code, Some(1));
    assert_eq!(info.status, ToolStatus::Failed);
}

#[test]
fn codex_rejection_flips_paired_call() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    let call2 = out
        .conversation
        .turns
        .iter()
        .find(|t| {
            t.tool
                .as_ref()
                .is_some_and(|i| i.pair_id.as_deref() == Some("call_2") && i.direction == Some(ToolDirection::Use))
        })
        .expect("call_2");
    assert_eq!(call2.tool.as_ref().unwrap().status, ToolStatus::Rejected);
}

#[test]
fn codex_abort_resolves_user_turn_by_turn_id() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    let abort = out
        .conversation
        .turns
        .iter()
        .find_map(|t| match &t.special {
            Some(SpecialTurn::TurnAborted { aborted_user_turn, reason }) => {
                Some((*aborted_user_turn, reason.clone()))
            }
            _ => None,
        })
        .expect("abort");
    assert_eq!(abort.1, "user_interrupt");
    let idx = abort.0.expect("resolved");
    assert_eq!(out.conversation.turns[idx as usize].text, "also run the tests");
}

#[test]
fn codex_reasoning_dropped_and_injected_context_stays_system() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    assert!(!out.conversation.turns.iter().any(|t| t.text.contains("internal reasoning")));
}

#[test]
fn cross_provider_project_collapse() {
    let claude = common::parse_fixture(AgentKind::Claude, "claude/basic_session.jsonl");
    let codex = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    assert_eq!(claude.conversation.normalized_project, "-home-u-src-proj");
    assert_eq!(codex.conversation.normalized_project, "-home-u-src-proj");
}

#[test]
fn malformed_lines_counted_and_offsets_exact() {
    let out = common::parse_fixture(AgentKind::Claude, "malformed/mixed_valid_invalid.jsonl");
    // Three bad lines: plain garbage, top-level array, truncated json.
    assert_eq!(out.conversation.malformed_lines, 3);
    assert_eq!(out.conversation.turns.len(), 4);
    // Source spans still point at the right lines.
    let second_prompt = out
        .conversation
        .turns
        .iter()
        .find(|t| t.text.contains("second prompt"))
        .unwrap();
    assert_eq!(second_prompt.source.line, 6);
}

#[test]
fn generic_fallback_extracts_roles() {
    let out = common::parse_fixture(AgentKind::Generic, "generic/openai_shape.jsonl");
    let roles: Vec<Role> = out.conversation.turns.iter().map(|t| t.role).collect();
    assert_eq!(roles, vec![Role::User, Role::Assistant, Role::Event]);
    assert!(out.conversation.turns[2].ts.is_some(), "numeric ts parsed");
}

#[test]
fn codex_auto_review_classified_auxiliary() {
    let out = common::parse_fixture(AgentKind::Codex, "codex/auto_review.jsonl");
    assert_eq!(out.conversation.origin, rogrep_model::Origin::Auxiliary);
    // A genuine session stays interactive.
    let normal = common::parse_fixture(AgentKind::Codex, "codex/session.jsonl");
    assert_eq!(normal.conversation.origin, rogrep_model::Origin::Interactive);
}
