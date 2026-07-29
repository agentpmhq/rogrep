//! Codex session parser.
//!
//! Format: `~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl`. Records:
//! `session_meta` (cwd, originator, subagent source), `turn_context`
//! (turn_id, cwd, model), `response_item` (payload.type: message,
//! function_call, function_call_output, web_search_call, reasoning),
//! `event_msg` (payload.type: error, tool_rejected, turn_aborted,
//! token_count, context_compacted, user_message/agent_message echoes),
//! `compacted`.
//!
//! `token_count` events are CUMULATIVE; the parser deltas them against the
//! previous totals and attaches the delta to the most recent model-output
//! turn inside the open exchange (pending-carry when none exists yet).

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{compact_json, string_value, u64_value, RawRecord};
use rogrep_model::{
    AgentKind, Origin, Role, SpecialTurn, TokenCounts, ToolDirection, ToolInfo, ToolStatus, Turn,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CODEX_PARSER_VERSION: u32 = 1;

pub struct CodexProvider;

impl Provider for CodexProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn parser_version(&self) -> u32 {
        CODEX_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        let p = path.replace('\\', "/");
        p.contains("/.codex/sessions/") && p.ends_with(".jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        SourceInfo {
            source_path: path.to_string(),
            project: "home".to_string(), // repaired via cwd by normalized_project
            cwd_seed: None,
            subagent: None,
        }
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(CodexParser::default())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct CodexState {
    /// Previous cumulative totals from token_count records.
    prev_totals: TokenCounts,
    /// Usage delta awaiting an eligible model turn.
    pending: TokenCounts,
    /// Current turn id from turn_context, stamped onto emitted turns.
    current_turn_id: Option<String>,
}

#[derive(Default)]
struct CodexParser {
    state: CodexState,
}

impl RolloutParser for CodexParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let rtype = rec.record_type();
        let payload = rec.object("payload");
        match rtype.as_str() {
            "session_meta" => {
                if let Some(p) = payload {
                    let cwd = string_value(p.get("cwd"));
                    if cwd.starts_with('/') {
                        ctx.set_cwd(cwd);
                    }
                    if p.get("source")
                        .and_then(Value::as_object)
                        .is_some_and(|s| s.contains_key("subagent"))
                        || string_value(p.get("thread_source")) == "subagent"
                    {
                        ctx.set_origin(Origin::Subagent);
                    }
                }
            }
            "turn_context" => {
                if let Some(p) = payload {
                    let cwd = string_value(p.get("cwd"));
                    if cwd.starts_with('/') {
                        ctx.set_cwd(cwd);
                    }
                    let model = model_from_payload(p);
                    if !model.is_empty() {
                        ctx.set_model(model);
                    }
                    let turn_id = string_value(p.get("turn_id"));
                    if !turn_id.is_empty() {
                        self.state.current_turn_id = Some(turn_id);
                    }
                }
            }
            "response_item" => {
                if let Some(p) = payload {
                    let model = model_from_payload(p);
                    if !model.is_empty() {
                        ctx.set_model(model);
                    }
                    for mut turn in self.turns_from_response_item(p) {
                        self.stamp_turn_id(&mut turn);
                        // A pending usage delta attaches to the next
                        // assistant turn.
                        if turn.role == Role::Assistant && !self.state.pending.is_zero() {
                            turn.tokens.add(&self.state.pending.clone());
                            self.state.pending = TokenCounts::default();
                        }
                        ctx.emit(turn);
                    }
                }
            }
            "event_msg" => {
                if let Some(p) = payload {
                    self.process_event(p, ctx);
                }
            }
            "compacted" => {
                ctx.emit(Turn {
                    role: Role::System,
                    speaker: "compaction".into(),
                    text: "Conversation compacted".into(),
                    special: Some(SpecialTurn::CompactBoundary),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    fn export_state(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }

    fn import_state(&mut self, state: &Value) {
        if !state.is_null() {
            if let Ok(s) = serde_json::from_value::<CodexState>(state.clone()) {
                self.state = s;
            }
        }
    }
}

impl CodexParser {
    fn stamp_turn_id(&self, turn: &mut Turn) {
        if let Some(id) = &self.state.current_turn_id {
            turn.provider_meta
                .insert("codex_turn_id".into(), Value::String(id.clone()));
        }
    }

    fn turns_from_response_item(&mut self, p: &Map<String, Value>) -> Vec<Turn> {
        let item_type = string_value(p.get("type"));
        match item_type.as_str() {
            "message" => {
                let role = crate::record::normalize_role(&string_value(p.get("role")));
                let base_role = Role::parse(&role).unwrap_or(Role::Event);
                let text = crate::record::content_text(p.get("content").unwrap_or(&Value::Null));
                if text.trim().is_empty() {
                    return vec![];
                }
                vec![Turn {
                    role: base_role,
                    speaker: role,
                    text,
                    ..Default::default()
                }]
            }
            "function_call" | "tool_call" | "custom_tool_call" | "web_search_call" => {
                let mut name = string_value(p.get("name"));
                if name.is_empty() {
                    name = if item_type == "web_search_call" {
                        "web_search".into()
                    } else {
                        item_type.clone()
                    };
                }
                let mut text = string_value(p.get("arguments"));
                if text.is_empty() {
                    text = string_value(p.get("input"));
                }
                if text.is_empty() {
                    if let Some(action) = p.get("action") {
                        text = compact_json(action);
                    }
                }
                if text.is_empty() {
                    text = compact_json(&Value::Object(p.clone()));
                }
                let mut input_fields = std::collections::BTreeMap::new();
                if let Ok(Value::Object(args)) =
                    serde_json::from_str::<Value>(&string_value(p.get("arguments")))
                {
                    for key in ["cmd", "command", "workdir", "path", "file_path", "pattern", "query"] {
                        if let Some(v) = args.get(key) {
                            input_fields.insert(key.to_string(), v.clone());
                        }
                    }
                }
                let mut turn = Turn {
                    role: Role::Tool,
                    speaker: name.clone(),
                    text,
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Use),
                        name,
                        pair_id: non_empty(string_value(p.get("call_id"))),
                        status: ToolStatus::Unknown,
                        input_fields,
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                if let Some(meta) = p.get("metadata").and_then(Value::as_object) {
                    let tid = string_value(meta.get("turn_id"));
                    if !tid.is_empty() {
                        turn.provider_meta
                            .insert("codex_turn_id".into(), Value::String(tid));
                    }
                }
                vec![turn]
            }
            "function_call_output" | "tool_call_output" | "custom_tool_call_output" => {
                let mut text = string_value(p.get("output"));
                if text.is_empty() {
                    text = crate::record::content_text(p.get("content").unwrap_or(&Value::Null));
                }
                if text.is_empty() {
                    text = compact_json(&Value::Object(p.clone()));
                }
                let (status, exit_code) = codex_output_status(p, &text);
                vec![Turn {
                    role: Role::Tool,
                    speaker: "tool_result".into(),
                    text: text.clone(),
                    tokens: TokenCounts {
                        estimated: rogrep_model::tokens::estimate_text_tokens(&text),
                        ..Default::default()
                    },
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Output),
                        name: "tool_result".into(),
                        pair_id: non_empty(string_value(p.get("call_id"))),
                        status,
                        exit_code,
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            }
            // reasoning payloads are dropped (matches agentpm).
            _ => vec![],
        }
    }

    fn process_event(&mut self, p: &Map<String, Value>, ctx: &mut ParseCtx<'_>) {
        let event_type = string_value(p.get("type"));
        match event_type.as_str() {
            "error" => {
                let text = {
                    let m = string_value(p.get("message"));
                    if m.is_empty() {
                        compact_json(&Value::Object(p.clone()))
                    } else {
                        m
                    }
                };
                ctx.emit(Turn {
                    role: Role::System,
                    speaker: "error".into(),
                    text,
                    ..Default::default()
                });
            }
            "tool_rejected" => {
                let call_id = {
                    let a = string_value(p.get("call_id"));
                    if a.is_empty() {
                        string_value(p.get("tool_call_id"))
                    } else {
                        a
                    }
                };
                // Flip the paired tool call inside the open exchange.
                for t in ctx.amendable().iter_mut().rev() {
                    if let Some(tool) = &mut t.tool {
                        if !call_id.is_empty() && tool.pair_id.as_deref() == Some(call_id.as_str()) {
                            tool.status = ToolStatus::Rejected;
                            break;
                        }
                    }
                }
                let text = {
                    let m = string_value(p.get("message"));
                    if m.is_empty() {
                        "Tool rejected".to_string()
                    } else {
                        m
                    }
                };
                ctx.emit(Turn {
                    role: Role::System,
                    speaker: "tool_rejected".into(),
                    text,
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Output),
                        name: "tool_rejected".into(),
                        pair_id: non_empty(call_id),
                        status: ToolStatus::Rejected,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            "turn_aborted" => {
                let reason = {
                    let r = string_value(p.get("reason"));
                    if r.is_empty() {
                        "aborted".to_string()
                    } else {
                        r
                    }
                };
                let turn_id = string_value(p.get("turn_id"));
                let aborted_user_turn = ctx.amendable().iter().find_map(|t| {
                    let matches_id = turn_id.is_empty()
                        || t.provider_meta
                            .get("codex_turn_id")
                            .and_then(Value::as_str)
                            == Some(turn_id.as_str());
                    (t.role == Role::User && matches_id).then_some(t.turn_index)
                });
                ctx.emit(Turn {
                    role: Role::System,
                    speaker: "turn_aborted".into(),
                    text: format!("Turn aborted: {reason}"),
                    special: Some(SpecialTurn::TurnAborted {
                        reason,
                        aborted_user_turn,
                    }),
                    ..Default::default()
                });
            }
            "token_count" => {
                let Some(info) = p.get("info").and_then(Value::as_object) else {
                    return;
                };
                let totals = info
                    .get("total_token_usage")
                    .and_then(Value::as_object)
                    .map(usage_from_codex)
                    .unwrap_or_default();
                let delta = totals.saturating_delta(&self.state.prev_totals);
                self.state.prev_totals = totals;
                if delta.is_zero() {
                    return;
                }
                // Attach to the most recent assistant turn in the open
                // exchange; otherwise carry as pending.
                let mut attached = false;
                for t in ctx.amendable().iter_mut().rev() {
                    if t.role == Role::Assistant {
                        t.tokens.add(&delta);
                        attached = true;
                        break;
                    }
                }
                if !attached {
                    self.state.pending.add(&delta);
                }
            }
            "context_compacted" => {
                ctx.emit(Turn {
                    role: Role::System,
                    speaker: "compaction".into(),
                    text: "Conversation compacted".into(),
                    special: Some(SpecialTurn::CompactBoundary),
                    ..Default::default()
                });
            }
            // user_message / agent_message / task_started / task_complete are
            // echoes of response_item records; dropped.
            _ => {}
        }
    }
}

fn usage_from_codex(u: &Map<String, Value>) -> TokenCounts {
    let cached = u64_value(u.get("cached_input_tokens"));
    let input_raw = u64_value(u.get("input_tokens"));
    TokenCounts {
        // codex input_tokens includes cached; split them out.
        input: input_raw.saturating_sub(cached),
        output: u64_value(u.get("output_tokens")),
        cache_read: cached,
        cache_creation: 0,
        reasoning_output: u64_value(u.get("reasoning_output_tokens")),
        estimated: 0,
    }
}

fn model_from_payload(p: &Map<String, Value>) -> String {
    let m = string_value(p.get("model"));
    if !m.is_empty() {
        return m;
    }
    if let Some(settings) = p.get("settings").and_then(Value::as_object) {
        let m = string_value(settings.get("model"));
        if !m.is_empty() {
            return m;
        }
    }
    if let Some(collab) = p.get("collaboration_mode").and_then(Value::as_object) {
        if let Some(settings) = collab.get("settings").and_then(Value::as_object) {
            let m = string_value(settings.get("model"));
            if !m.is_empty() {
                return m;
            }
        }
    }
    String::new()
}

/// Status cascade for codex tool outputs: structured metadata first, then
/// mined from the output text (`Process exited with code N`, bwrap
/// failures).
fn codex_output_status(p: &Map<String, Value>, text: &str) -> (ToolStatus, Option<i64>) {
    let mut exit_code: Option<i64> = None;
    let mut status_str = String::new();
    let mut scan = |source: &Map<String, Value>| {
        if exit_code.is_none() {
            exit_code = crate::record::int_value(source.get("exit_code"));
        }
        if status_str.is_empty() {
            status_str = string_value(source.get("status"));
        }
        if status_str.is_empty() {
            status_str = string_value(source.get("tool_status"));
        }
    };
    if let Some(meta) = p.get("metadata").and_then(Value::as_object) {
        scan(meta);
    }
    if let Ok(Value::Object(envelope)) = serde_json::from_str::<Value>(text) {
        if let Some(meta) = envelope.get("metadata").and_then(Value::as_object) {
            scan(meta);
        }
        scan(&envelope);
    }
    if exit_code.is_none() {
        // "Process exited with code N" mined from the head of the output.
        let head: String = text.chars().take(400).collect();
        if let Some(idx) = head.find("Process exited with code ") {
            let rest = &head[idx + "Process exited with code ".len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
            exit_code = digits.parse::<i64>().ok();
        }
    }
    let status = match (&status_str[..], exit_code) {
        ("rejected", _) => ToolStatus::Rejected,
        ("failed" | "error", _) => ToolStatus::Failed,
        ("completed" | "success" | "succeeded" | "ok", None) => ToolStatus::Succeeded,
        (_, Some(0)) => ToolStatus::Succeeded,
        (_, Some(_)) => ToolStatus::Failed,
        ("", None) => ToolStatus::Unknown,
        (_, None) => ToolStatus::Unknown,
    };
    (status, exit_code)
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}
