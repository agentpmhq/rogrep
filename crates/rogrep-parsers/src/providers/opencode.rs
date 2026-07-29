//! opencode spool parser (records materialized by spool/opencode_db.rs).
//!
//! A `part` of type "tool" merges the call AND its result: it becomes a
//! tool_use turn plus a tool_output turn. `step-finish` parts carry
//! per-step token accounting, attached to the most recent assistant turn.

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{string_value, RawRecord};
use rogrep_model::{
    AgentKind, Origin, Role, TokenCounts, ToolDirection, ToolInfo, ToolStatus, Turn,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const OPENCODE_PARSER_VERSION: u32 = 1;

pub struct OpencodeProvider;

impl Provider for OpencodeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Opencode
    }

    fn parser_version(&self) -> u32 {
        OPENCODE_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        path.replace('\\', "/").contains("/spool/opencode/") && path.ends_with(".jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        SourceInfo {
            source_path: path.to_string(),
            project: "home".to_string(),
            cwd_seed: None,
            subagent: None,
            default_ts: None,
        }
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(OpencodeParser)
    }
}

struct OpencodeParser;

impl RolloutParser for OpencodeParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        match rec.record_type().as_str() {
            "session_meta" => {
                let cwd = rec.str_field("cwd");
                if cwd.starts_with('/') {
                    ctx.set_cwd(cwd);
                }
                let title = rec.str_field("title");
                if !title.is_empty() {
                    ctx.set_title(title);
                }
                let model = rec.str_field("model");
                if !model.is_empty() {
                    ctx.set_model(model);
                }
                if !rec.str_field("parent_session_id").is_empty() {
                    ctx.set_origin(Origin::Subagent);
                }
            }
            "part" => self.process_part(rec, ctx),
            _ => {}
        }
    }
}

impl OpencodeParser {
    fn process_part(&self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let Some(data) = rec.object("data") else { return };
        let part_type = string_value(data.get("type"));
        let message_role = rec.str_field("message_role");
        let model = rec.str_field("model");
        if !model.is_empty() {
            ctx.set_model(model);
        }
        match part_type.as_str() {
            "text" => {
                let text = string_value(data.get("text"));
                if text.is_empty() {
                    return;
                }
                let role = match message_role.as_str() {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    _ => Role::Event,
                };
                ctx.emit(Turn {
                    role,
                    speaker: message_role,
                    text,
                    ..Default::default()
                });
            }
            "tool" => {
                let tool = string_value(data.get("tool"));
                let call_id = non_empty(string_value(data.get("callID")));
                let state = data.get("state").and_then(Value::as_object);
                let (input, output, status, exit_code) = match state {
                    Some(s) => {
                        let input = s.get("input").cloned().unwrap_or(Value::Null);
                        let output = string_value(s.get("output"));
                        let status_str = string_value(s.get("status"));
                        let exit = s
                            .get("metadata")
                            .and_then(Value::as_object)
                            .and_then(|m| m.get("exit"))
                            .and_then(Value::as_i64);
                        let status = match (status_str.as_str(), exit) {
                            ("error" | "failed", _) => ToolStatus::Failed,
                            (_, Some(0)) => ToolStatus::Succeeded,
                            (_, Some(_)) => ToolStatus::Failed,
                            ("completed", None) => ToolStatus::Succeeded,
                            ("pending" | "running", None) => ToolStatus::Unknown,
                            _ => ToolStatus::Unknown,
                        };
                        (input, output, status, exit)
                    }
                    None => (Value::Null, String::new(), ToolStatus::Unknown, None),
                };
                let mut input_fields = BTreeMap::new();
                if let Some(input) = input.as_object() {
                    for key in ["command", "cmd", "filePath", "file_path", "path", "pattern", "query"] {
                        if let Some(v) = input.get(key) {
                            let normalized = if key == "filePath" { "file_path" } else { key };
                            input_fields.insert(normalized.to_string(), v.clone());
                        }
                    }
                }
                let call_text = if input.is_null() {
                    tool.clone()
                } else {
                    serde_json::to_string(&input).unwrap_or_default()
                };
                ctx.emit(Turn {
                    role: Role::Tool,
                    speaker: if tool.is_empty() { "tool_use".into() } else { tool.clone() },
                    text: call_text,
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Use),
                        name: if tool.is_empty() { "tool_use".into() } else { tool },
                        pair_id: call_id.clone(),
                        status: ToolStatus::Unknown,
                        input_fields,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                if !output.is_empty() || status != ToolStatus::Unknown {
                    ctx.emit(Turn {
                        role: Role::Tool,
                        speaker: "tool_result".into(),
                        text: output.clone(),
                        tokens: TokenCounts {
                            estimated: rogrep_model::tokens::estimate_text_tokens(&output),
                            ..Default::default()
                        },
                        tool: Some(ToolInfo {
                            direction: Some(ToolDirection::Output),
                            name: "tool_result".into(),
                            pair_id: call_id,
                            status,
                            exit_code,
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
            }
            "step-finish" => {
                // Per-step token accounting → most recent assistant turn in
                // the open exchange.
                let Some(tokens) = data.get("tokens").and_then(Value::as_object) else {
                    return;
                };
                let cache = tokens.get("cache").and_then(Value::as_object);
                let delta = TokenCounts {
                    input: tokens.get("input").and_then(Value::as_u64).unwrap_or(0),
                    output: tokens.get("output").and_then(Value::as_u64).unwrap_or(0),
                    reasoning_output: tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0),
                    cache_read: cache.and_then(|c| c.get("read")).and_then(Value::as_u64).unwrap_or(0),
                    cache_creation: cache.and_then(|c| c.get("write")).and_then(Value::as_u64).unwrap_or(0),
                    estimated: 0,
                };
                if delta.is_zero() {
                    return;
                }
                for t in ctx.amendable().iter_mut().rev() {
                    if t.role == Role::Assistant {
                        t.tokens.add(&delta);
                        return;
                    }
                }
            }
            // reasoning / step-start / patch / snapshot parts add no turns.
            _ => {}
        }
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}
