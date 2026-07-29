//! Grok session parser.
//!
//! `~/.grok/sessions/<url-escaped-cwd>/<session-id>/chat_history.jsonl` —
//! flat records `{type: user|assistant|system|tool_result, content,
//! tool_calls: [{id, name, arguments}], reasoning, tool_call_id}`. The cwd
//! lives URL-escaped in the path; the model only in the system prompt text
//! ("You are Grok 4.3 released by xAI…").

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{content_text, string_value, RawRecord};
use rogrep_model::{AgentKind, Role, ToolDirection, ToolInfo, ToolStatus, Turn};
use serde_json::Value;
use std::collections::BTreeMap;

pub const GROK_PARSER_VERSION: u32 = 2;

pub struct GrokProvider;

impl Provider for GrokProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Grok
    }

    fn parser_version(&self) -> u32 {
        GROK_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        let p = path.replace('\\', "/");
        p.contains("/.grok/sessions/") && p.ends_with("/chat_history.jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        let slash = path.replace('\\', "/");
        let parts: Vec<&str> = slash.split('/').collect();
        let mut cwd = None;
        let mut default_ts = None;
        for (i, part) in parts.iter().enumerate() {
            if *part == "sessions" && i + 1 < parts.len() {
                let decoded = percent_decode(parts[i + 1]);
                if decoded.starts_with('/') {
                    cwd = Some(decoded);
                }
                // The session dir is a uuid7; its first 48 bits are a unix
                // millisecond timestamp — the only time signal grok records.
                if i + 2 < parts.len() {
                    default_ts = uuid7_millis(parts[i + 2]);
                }
                break;
            }
        }
        SourceInfo {
            source_path: path.to_string(),
            project: "home".to_string(),
            cwd_seed: cwd,
            subagent: None,
            default_ts,
        }
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(GrokParser)
    }
}

/// uuid7 → unix millis (first 48 bits), sanity-bounded to 2015–2100.
fn uuid7_millis(uuid: &str) -> Option<i64> {
    let hex: String = uuid.chars().filter(|c| c.is_ascii_hexdigit()).take(12).collect();
    if hex.len() != 12 {
        return None;
    }
    let ms = i64::from_str_radix(&hex, 16).ok()?;
    (1_420_000_000_000..4_100_000_000_000).contains(&ms).then_some(ms)
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct GrokParser;

impl RolloutParser for GrokParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let rtype = rec.record_type();
        match rtype.as_str() {
            "system" => {
                let text = content_text(rec.get("content").unwrap_or(&Value::Null));
                if let Some(model) = model_from_system_text(&text) {
                    ctx.set_model(model);
                }
                if !text.is_empty() {
                    ctx.emit(Turn {
                        role: Role::System,
                        speaker: "system".into(),
                        text,
                        ..Default::default()
                    });
                }
            }
            "user" => {
                let mut text = content_text(rec.get("content").unwrap_or(&Value::Null));
                if text.is_empty() {
                    return;
                }
                // Real prompts arrive wrapped in <user_query> tags (possibly
                // with other context blocks around them).
                let lower = text.to_lowercase();
                if let Some(start) = lower.find("<user_query>") {
                    let body_start = start + "<user_query>".len();
                    if let Some(end) = lower[body_start..].find("</user_query>") {
                        let q = text[body_start..body_start + end].trim();
                        if !q.is_empty() {
                            text = q.to_string();
                        }
                    }
                }
                // Synthetic system-reminder / user_info blocks are harness
                // context, not prompts.
                let synthetic = rec.str_field("synthetic_reason") == "system_reminder";
                let trimmed = text.trim_start();
                let injected = trimmed.starts_with("<user_info>") || trimmed.starts_with("<system-reminder>");
                ctx.emit(Turn {
                    role: if synthetic || injected { Role::System } else { Role::User },
                    speaker: if synthetic || injected { "system".into() } else { "user".into() },
                    text,
                    synthetic_context: synthetic || injected,
                    ..Default::default()
                });
            }
            "assistant" => {
                let text = content_text(rec.get("content").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    ctx.emit(Turn {
                        role: Role::Assistant,
                        speaker: "assistant".into(),
                        text,
                        ..Default::default()
                    });
                }
                if let Some(calls) = rec.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let Some(call) = call.as_object() else { continue };
                        let name = {
                            let n = string_value(call.get("name"));
                            if n.is_empty() {
                                // OpenAI-ish nesting: {function:{name,arguments}}.
                                call.get("function")
                                    .and_then(Value::as_object)
                                    .map(|f| string_value(f.get("name")))
                                    .unwrap_or_default()
                            } else {
                                n
                            }
                        };
                        let arguments = {
                            let a = string_value(call.get("arguments"));
                            if a.is_empty() {
                                call.get("function")
                                    .and_then(Value::as_object)
                                    .map(|f| string_value(f.get("arguments")))
                                    .unwrap_or_default()
                            } else {
                                a
                            }
                        };
                        let mut input_fields = BTreeMap::new();
                        if let Ok(Value::Object(args)) = serde_json::from_str::<Value>(&arguments) {
                            for key in ["command", "target_directory", "path", "file_path", "pattern", "query"] {
                                if let Some(v) = args.get(key) {
                                    input_fields.insert(key.to_string(), v.clone());
                                }
                            }
                            // grok's shell tool uses `command`.
                        }
                        let text = if arguments.is_empty() {
                            crate::record::compact_json(&Value::Object(call.clone()))
                        } else {
                            arguments
                        };
                        ctx.emit(Turn {
                            role: Role::Tool,
                            speaker: if name.is_empty() { "tool_use".into() } else { name.clone() },
                            text,
                            tool: Some(ToolInfo {
                                direction: Some(ToolDirection::Use),
                                name: normalize_grok_tool(&name),
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
            "tool_result" => {
                let text = content_text(rec.get("content").unwrap_or(&Value::Null));
                let is_error = rec.bool_field("is_error");
                ctx.emit(Turn {
                    role: Role::Tool,
                    speaker: "tool_result".into(),
                    text: text.clone(),
                    tokens: rogrep_model::TokenCounts {
                        estimated: rogrep_model::tokens::estimate_text_tokens(&text),
                        ..Default::default()
                    },
                    tool: Some(ToolInfo {
                        direction: Some(ToolDirection::Output),
                        name: "tool_result".into(),
                        pair_id: non_empty(rec.str_field("tool_call_id")),
                        status: match is_error {
                            Some(true) => ToolStatus::Failed,
                            _ => ToolStatus::Succeeded,
                        },
                        is_error,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
}

/// Grok's shell tool is `run_terminal_command`; map to a shell-tool name the
/// facet extractor recognizes.
fn normalize_grok_tool(name: &str) -> String {
    match name {
        "run_terminal_command" => "run_terminal_cmd".to_string(),
        other => other.to_string(),
    }
}

/// "You are Grok 4.3 released by xAI…" → "grok-4.3".
fn model_from_system_text(text: &str) -> Option<String> {
    let idx = text.find("You are Grok ")?;
    let version_start = idx + "You are Grok ".len();
    let version: String = text[version_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let version = version.trim_end_matches('.');
    if version.is_empty() {
        Some("grok".to_string())
    } else {
        Some(format!("grok-{version}"))
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("%2Fhome%2Fu%2Fsrc%2Fx"), "/home/u/src/x");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn model_regex() {
        assert_eq!(
            model_from_system_text("You are Grok 4.3 released by xAI in April 2026. You are…"),
            Some("grok-4.3".into())
        );
        assert_eq!(model_from_system_text("unrelated"), None);
    }
}
