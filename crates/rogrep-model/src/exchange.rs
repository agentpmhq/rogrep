//! The exchange: one real user prompt plus every agent action until the next
//! real user prompt. rogrep's first-class query unit (agentpm calls these
//! "notebook cards", conversations.go:5487).

use crate::special::SpecialTurn;
use crate::tokens::TokenCounts;
use crate::turn::{Role, ToolDirection, ToolStatus, Turn};
use crate::UnixMillis;
use serde::{Deserialize, Serialize};

/// Boolean outcome signals rolled up per exchange (queryable via `has:`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExchangeSignals {
    /// Any failed tool call.
    pub error: bool,
    /// Any rejected tool call (permission denied by user).
    pub rejected: bool,
    /// The user interrupted / the turn aborted.
    pub interrupted: bool,
    /// A context compaction happened inside this exchange.
    pub compacted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exchange {
    /// 0-based internally; user-facing ids (`CONV#eN`) are ordinal+1.
    pub ordinal: u32,
    /// Turn index of the opening real user prompt; None for a leading
    /// pre-user preamble exchange.
    pub user_turn_index: Option<u32>,
    /// Turn range `[start_turn, end_turn)` — contiguous, covers all turns.
    pub start_turn: u32,
    pub end_turn: u32,
    pub started_at: Option<UnixMillis>,
    pub ended_at: Option<UnixMillis>,
    pub assistant_turns: u32,
    pub tool_calls: u32,
    pub failed_tool_calls: u32,
    pub rejected_tool_calls: u32,
    pub tokens: TokenCounts,
    pub signals: ExchangeSignals,
    /// First ~200 chars of the user prompt, for list UIs.
    pub user_preview: String,
}

impl Exchange {
    pub fn duration_ms(&self) -> Option<i64> {
        match (self.started_at, self.ended_at) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }

    pub fn turn_count(&self) -> u32 {
        self.end_turn - self.start_turn
    }
}

/// Does this turn open a new exchange? Role must be user AND the turn must
/// not be a harness echo (task notification, scheduled prompt, compact
/// boundary) or injected/synthetic context.
pub fn is_real_user_prompt(turn: &Turn) -> bool {
    if turn.role != Role::User {
        return false;
    }
    if turn.synthetic_context {
        return false;
    }
    if let Some(special) = &turn.special {
        if special.suppresses_exchange_boundary() {
            return false;
        }
    }
    if crate::turn::is_injected_context_text(&turn.text) {
        return false;
    }
    true
}

const USER_PREVIEW_CHARS: usize = 200;

fn preview(text: &str) -> String {
    let line = text.trim();
    let mut s: String = line.chars().take(USER_PREVIEW_CHARS).collect();
    if line.chars().count() > USER_PREVIEW_CHARS {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// Build exchanges over a full (or partial) turn slice. Pure and
/// deterministic. Appending turns can only extend the final exchange or add
/// new ones, so incremental sync recomputes from the last stored exchange's
/// start_turn — `build_exchanges` on the tail slice with `base_ordinal` /
/// turn offsets handled by the caller.
pub fn build_exchanges(turns: &[Turn]) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    for turn in turns {
        let opens = is_real_user_prompt(turn);
        if opens || out.is_empty() {
            let ordinal = out.len() as u32;
            out.push(Exchange {
                ordinal,
                user_turn_index: opens.then_some(turn.turn_index),
                start_turn: turn.turn_index,
                end_turn: turn.turn_index,
                started_at: turn.ts,
                user_preview: if opens { preview(&turn.text) } else { String::new() },
                ..Default::default()
            });
        }
        let ex = out.last_mut().expect("just ensured non-empty");
        ex.end_turn = turn.turn_index + 1;
        if ex.started_at.is_none() {
            ex.started_at = turn.ts;
        }
        if turn.ts.is_some() {
            ex.ended_at = turn.ts;
        }
        ex.tokens.add(&turn.tokens);
        match turn.role {
            Role::Assistant => ex.assistant_turns += 1,
            Role::Tool => {
                if let Some(tool) = &turn.tool {
                    match tool.direction {
                        Some(ToolDirection::Use) => ex.tool_calls += 1,
                        Some(ToolDirection::Output) => match tool.status {
                            ToolStatus::Failed => {
                                ex.failed_tool_calls += 1;
                                ex.signals.error = true;
                            }
                            ToolStatus::Rejected => {
                                ex.rejected_tool_calls += 1;
                                ex.signals.rejected = true;
                            }
                            _ => {}
                        },
                        None => {}
                    }
                }
            }
            _ => {}
        }
        match &turn.special {
            Some(SpecialTurn::TurnAborted { .. }) => ex.signals.interrupted = true,
            Some(SpecialTurn::CompactBoundary) => ex.signals.compacted = true,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::ToolInfo;

    fn user(i: u32, text: &str) -> Turn {
        Turn {
            turn_index: i,
            role: Role::User,
            text: text.into(),
            ts: Some(1000 + i as i64 * 100),
            ..Default::default()
        }
    }

    fn assistant(i: u32) -> Turn {
        Turn {
            turn_index: i,
            role: Role::Assistant,
            text: "ok".into(),
            ts: Some(1000 + i as i64 * 100),
            ..Default::default()
        }
    }

    fn tool_output(i: u32, status: ToolStatus) -> Turn {
        Turn {
            turn_index: i,
            role: Role::Tool,
            text: "out".into(),
            tool: Some(ToolInfo {
                direction: Some(ToolDirection::Output),
                name: "Bash".into(),
                status,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn basic_grouping() {
        let turns = vec![user(0, "first"), assistant(1), user(2, "second"), assistant(3)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex.len(), 2);
        assert_eq!(ex[0].start_turn..ex[0].end_turn, 0..2);
        assert_eq!(ex[1].start_turn..ex[1].end_turn, 2..4);
        assert_eq!(ex[0].user_turn_index, Some(0));
        assert_eq!(ex[0].user_preview, "first");
    }

    #[test]
    fn leading_orphan_card() {
        let turns = vec![assistant(0), user(1, "hi"), assistant(2)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex.len(), 2);
        assert_eq!(ex[0].user_turn_index, None);
        assert_eq!(ex[0].start_turn..ex[0].end_turn, 0..1);
        assert_eq!(ex[1].user_turn_index, Some(1));
    }

    #[test]
    fn task_notification_does_not_open_exchange() {
        let mut notif = user(2, "task finished");
        notif.special = Some(SpecialTurn::TaskNotification {
            queued: false,
            status: None,
            summary: "done".into(),
            signature: "sig".into(),
        });
        let turns = vec![user(0, "go"), assistant(1), notif, assistant(3)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex.len(), 1, "notification must stay inside the exchange");
        assert_eq!(ex[0].end_turn, 4);
    }

    #[test]
    fn injected_context_does_not_open_exchange() {
        let turns = vec![user(0, "go"), user(1, "<system-reminder>noise")];
        let ex = build_exchanges(&turns);
        assert_eq!(ex.len(), 1);
    }

    #[test]
    fn failure_signals_rollup() {
        let turns = vec![user(0, "go"), tool_output(1, ToolStatus::Failed), tool_output(2, ToolStatus::Rejected)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex[0].failed_tool_calls, 1);
        assert_eq!(ex[0].rejected_tool_calls, 1);
        assert!(ex[0].signals.error);
        assert!(ex[0].signals.rejected);
    }

    #[test]
    fn contiguous_cover() {
        let turns = vec![user(0, "a"), assistant(1), user(2, "b"), assistant(3), assistant(4)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex.first().unwrap().start_turn, 0);
        assert_eq!(ex.last().unwrap().end_turn, 5);
        for w in ex.windows(2) {
            assert_eq!(w[0].end_turn, w[1].start_turn);
        }
    }

    #[test]
    fn duration_from_timestamps() {
        let turns = vec![user(0, "a"), assistant(1), assistant(2)];
        let ex = build_exchanges(&turns);
        assert_eq!(ex[0].duration_ms(), Some(200));
    }
}
