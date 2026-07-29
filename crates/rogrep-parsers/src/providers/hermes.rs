//! Hermes spool parser (records materialized by spool/hermes_db.rs).

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{string_value, RawRecord};
use rogrep_model::{
    AgentKind, Origin, Role, SubagentLink, TokenCounts, ToolDirection, ToolInfo, ToolStatus, Turn,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const HERMES_PARSER_VERSION: u32 = 1;

pub struct HermesProvider;

impl Provider for HermesProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Hermes
    }

    fn parser_version(&self) -> u32 {
        HERMES_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        path.replace('\\', "/").contains("/spool/hermes/") && path.ends_with(".jsonl")
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
        Box::new(HermesParser)
    }
}

struct HermesParser;

impl RolloutParser for HermesParser {
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
            "message" => self.process_message(rec, ctx),
            _ => {}
        }
    }
}

impl HermesParser {
    fn process_message(&self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let role = rec.str_field("role");
        let content = rec.str_field("content");
        let ts = rec
            .get("timestamp")
            .and_then(Value::as_f64)
            .and_then(rogrep_model::millis_from_number);
        match role.as_str() {
            "user" => {
                if content.is_empty() {
                    return;
                }
                ctx.emit(Turn {
                    role: Role::User,
                    speaker: "user".into(),
                    text: content,
                    ts,
                    ..Default::default()
                });
            }
            "assistant" => {
                if !content.trim().is_empty() {
                    let tokens = rec
                        .get("token_count")
                        .and_then(Value::as_u64)
                        .map(|n| TokenCounts {
                            output: n,
                            ..Default::default()
                        })
                        .unwrap_or_default();
                    ctx.emit(Turn {
                        role: Role::Assistant,
                        speaker: "assistant".into(),
                        text: content,
                        ts,
                        tokens,
                        ..Default::default()
                    });
                }
                // tool_calls is a JSON string of an OpenAI-ish array.
                let calls = rec.str_field("tool_calls");
                if let Ok(Value::Array(calls)) = serde_json::from_str::<Value>(&calls) {
                    for call in calls {
                        let Some(call) = call.as_object() else { continue };
                        let function = call.get("function").and_then(Value::as_object);
                        let name = {
                            let n = string_value(call.get("name"));
                            if n.is_empty() {
                                function.map(|f| string_value(f.get("name"))).unwrap_or_default()
                            } else {
                                n
                            }
                        };
                        let arguments = {
                            let a = string_value(call.get("arguments"));
                            if a.is_empty() {
                                function
                                    .map(|f| string_value(f.get("arguments")))
                                    .unwrap_or_default()
                            } else {
                                a
                            }
                        };
                        let mut input_fields = BTreeMap::new();
                        if let Ok(Value::Object(args)) = serde_json::from_str::<Value>(&arguments) {
                            for key in ["command", "cmd", "path", "file_path", "pattern", "query"] {
                                if let Some(v) = args.get(key) {
                                    input_fields.insert(key.to_string(), v.clone());
                                }
                            }
                        }
                        ctx.emit(Turn {
                            role: Role::Tool,
                            speaker: if name.is_empty() { "tool_use".into() } else { name.clone() },
                            text: if arguments.is_empty() { name.clone() } else { arguments },
                            ts,
                            tool: Some(ToolInfo {
                                direction: Some(ToolDirection::Use),
                                name: if name.is_empty() { "tool_use".into() } else { name },
                                pair_id: non_empty(string_value(call.get("id"))),
                                status: ToolStatus::Unknown,
                                input_fields,
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    }
                }
            }
            "tool" => {
                ctx.emit(Turn {
                    role: Role::Tool,
                    speaker: "tool_result".into(),
                    text: content.clone(),
                    ts,
                    tokens: TokenCounts {
                        estimated: rogrep_model::tokens::estimate_text_tokens(&content),
                        ..Default::default()
                    },
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Output),
                        name: non_empty(rec.str_field("tool_name")).unwrap_or_else(|| "tool_result".into()),
                        pair_id: non_empty(rec.str_field("tool_call_id")),
                        status: ToolStatus::Succeeded,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            "system" => {
                if !content.is_empty() {
                    ctx.emit(Turn {
                        role: Role::System,
                        speaker: "system".into(),
                        text: content,
                        ts,
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Build the subagent link for a hermes spool file whose meta names a parent
/// session (used by the driver via source_info only — parent links for
/// hermes flow through Origin::Subagent instead; kept for future use).
#[allow(dead_code)]
fn parent_link(spool_path: &str, parent_session: &str) -> Option<SubagentLink> {
    let dir = std::path::Path::new(spool_path).parent()?;
    let parent_path = dir.join(format!("{parent_session}.jsonl"));
    let parent_str = parent_path.to_string_lossy().to_string();
    Some(SubagentLink {
        parent_id: Some(rogrep_model::ConversationId::from_source_path(&parent_str)),
        parent_source_path: Some(parent_str),
        subagent_id: None,
    })
}
