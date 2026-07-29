//! Claude Code JSONL parser.
//!
//! Format: `~/.claude/projects/<dash-encoded-cwd>/<session-uuid>.jsonl`.
//! Records: user/assistant/system with a nested `message` object whose
//! `content` is a string or an array of typed blocks (text, thinking,
//! tool_use, tool_result, image); plus meta records (`ai-title`, `summary`,
//! `attachment`, `queue-operation`, `compact-boundary` subtype markers,
//! `file-history-*`, `permission-mode`, …). Subagent transcripts live at
//! `<proj>/<session-uuid>/subagents/<agent-id>.jsonl`.

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{compact_json, content_text, string_value, u64_value, RawRecord};
use rogrep_model::{
    project, AgentKind, AttachmentKind, Role, SpecialTurn, SubagentLink, TokenCounts, ToolDirection,
    ToolInfo, ToolStatus, Turn,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const CLAUDE_PARSER_VERSION: u32 = 2;

pub struct ClaudeProvider;

impl Provider for ClaudeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn parser_version(&self) -> u32 {
        CLAUDE_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        let p = path.replace('\\', "/");
        p.contains("/.claude/projects/")
            && p.ends_with(".jsonl")
            && !p.ends_with("/history.jsonl")
            && !p.ends_with("/audit.jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        source_info_for_claude_path(path)
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(ClaudeParser::default())
    }
}

pub fn source_info_for_claude_path(path: &str) -> SourceInfo {
    let slash = path.replace('\\', "/");
    let parts: Vec<&str> = slash.split('/').collect();
    let mut project_slug = String::new();
    for (i, part) in parts.iter().enumerate() {
        if *part == "projects" && i + 1 < parts.len() {
            project_slug = parts[i + 1].trim_end_matches(".jsonl").to_string();
            break;
        }
    }
    let cwd_seed = {
        let cwd = project::slash_path_from_dash_project(&project_slug);
        (!cwd.is_empty()).then_some(cwd)
    };
    let subagent = subagent_info_from_path(&slash);
    SourceInfo {
        source_path: path.to_string(),
        project: project_slug,
        cwd_seed,
        subagent,
        default_ts: None,
    }
}

/// `<proj>/<parent-session-uuid>/subagents/<agent-id>.jsonl` → link to the
/// sibling parent transcript `<proj>/<parent-session-uuid>.jsonl`.
fn subagent_info_from_path(slash_path: &str) -> Option<SubagentLink> {
    let (dir, file) = slash_path.rsplit_once('/')?;
    let (parent_dir, marker) = dir.rsplit_once('/')?;
    if marker != "subagents" {
        return None;
    }
    let subagent_id = file.trim_end_matches(".jsonl").to_string();
    let parent_source = format!("{parent_dir}.jsonl");
    Some(SubagentLink {
        parent_id: Some(rogrep_model::ConversationId::from_source_path(&parent_source)),
        parent_source_path: Some(parent_source),
        subagent_id: Some(subagent_id),
    })
}

#[derive(Default)]
struct ClaudeParser {
    /// Last assistant message.id whose usage was attached — usage repeats on
    /// every content-block record of the same API message; count it once.
    last_usage_message_id: Option<String>,
    /// True once an ai-title claimed the title (beats `summary` records).
    title_from_ai: bool,
}

impl RolloutParser for ClaudeParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        // Record-level signals.
        let cwd = rec.str_field("cwd");
        if cwd.starts_with('/') {
            ctx.set_cwd(cwd);
        }

        let rtype = rec.record_type();
        match rtype.as_str() {
            "ai-title" => {
                let t = rec.str_field("aiTitle");
                if !t.is_empty() {
                    self.title_from_ai = true;
                    ctx.set_title(t);
                }
                return;
            }
            "summary" => {
                if !self.title_from_ai {
                    let t = rec.str_field("summary");
                    if !t.is_empty() {
                        ctx.set_title(t);
                    }
                }
                return;
            }
            "attachment" => {
                if let Some(turn) = attachment_turn(rec) {
                    ctx.emit(turn);
                }
                return;
            }
            "queue-operation" => {
                let text = crate::record::extract_text(&rec.obj);
                if text.is_empty() {
                    return;
                }
                let mut turn = Turn {
                    role: Role::System,
                    speaker: "queue-operation".into(),
                    text,
                    ..Default::default()
                };
                turn.provider_meta.insert(
                    "queue_operation".into(),
                    Value::String(rec.str_field("operation")),
                );
                ctx.emit(turn);
                return;
            }
            // Bookkeeping records with no conversational payload.
            "file-history-snapshot" | "file-history-delta" | "permission-mode" | "mode"
            | "last-prompt" | "agent-name" | "todos" | "session-hook" => return,
            _ => {}
        }

        let Some(message) = rec.object("message") else {
            // system records sometimes carry top-level content.
            if rtype == "system" {
                let subtype = rec.str_field("subtype");
                if subtype == "compact_boundary" {
                    ctx.emit(compact_boundary_turn());
                    return;
                }
                let text = crate::record::extract_text(&rec.obj);
                if !text.is_empty() {
                    ctx.emit(Turn {
                        role: Role::System,
                        speaker: "system".into(),
                        text,
                        ..Default::default()
                    });
                }
            }
            return;
        };

        if rec.str_field("subtype") == "compact_boundary" {
            ctx.emit(compact_boundary_turn());
            return;
        }

        if let Some(model) = message.get("model").map(|v| string_value(Some(v))) {
            if !model.is_empty() && model != "<synthetic>" {
                ctx.set_model(model);
            }
        }

        let role = crate::record::normalize_role(&string_value(message.get("role")));
        let is_meta = rec.bool_field("isMeta").unwrap_or(false);
        let mut turns = turns_from_message(role.as_str(), message);

        // Attach usage once per API message id (it repeats on every
        // content-block record of the same message).
        let message_id = string_value(message.get("id"));
        let usage_target = turns.iter().position(|t| t.role == Role::Assistant);
        if let (Some(idx), Some(usage)) = (usage_target, message.get("usage").and_then(Value::as_object)) {
            if message_id.is_empty() || self.last_usage_message_id.as_deref() != Some(message_id.as_str()) {
                turns[idx].tokens = usage_tokens(usage);
                if !message_id.is_empty() {
                    self.last_usage_message_id = Some(message_id.clone());
                }
            }
        }

        for mut turn in turns {
            if is_meta {
                turn.provider_meta.insert("is_meta".into(), Value::Bool(true));
                if turn.role == Role::User && !turn.text.to_lowercase().contains("<scheduled-task") {
                    // Meta user records (caveats, command echoes) are harness
                    // context, not real prompts.
                    turn.synthetic_context = true;
                }
            }
            if is_request_interrupted_text(&turn.text) {
                turn = interrupted_turn(ctx);
            }
            for (meta_key, raw_key) in [
                ("claude_uuid", "uuid"),
                ("claude_parent_uuid", "parentUuid"),
                ("claude_prompt_id", "promptId"),
                ("claude_request_id", "requestId"),
                ("claude_session_id", "sessionId"),
                ("claude_entrypoint", "entrypoint"),
            ] {
                let v = rec.str_field(raw_key);
                if !v.is_empty() {
                    turn.provider_meta.insert(meta_key.into(), Value::String(v));
                }
            }
            ctx.emit(turn);
        }
    }
}

fn compact_boundary_turn() -> Turn {
    Turn {
        role: Role::System,
        speaker: "compaction".into(),
        text: "Conversation compacted".into(),
        special: Some(SpecialTurn::CompactBoundary),
        ..Default::default()
    }
}

fn is_request_interrupted_text(text: &str) -> bool {
    let t = text.trim();
    t == "[Request interrupted by user]" || t == "[Request interrupted by user for tool use]"
}

/// Forward-pointing abort: reference the opening user prompt of the current
/// exchange rather than mutating it (keeps incremental parses exact).
fn interrupted_turn(ctx: &mut ParseCtx<'_>) -> Turn {
    let aborted_user_turn = ctx
        .amendable()
        .iter()
        .find(|t| rogrep_model::exchange::is_real_user_prompt(t))
        .map(|t| t.turn_index);
    Turn {
        role: Role::System,
        speaker: "turn_aborted".into(),
        text: "Turn aborted: interrupted".into(),
        special: Some(SpecialTurn::TurnAborted {
            reason: "interrupted".into(),
            aborted_user_turn,
        }),
        ..Default::default()
    }
}

fn usage_tokens(usage: &Map<String, Value>) -> TokenCounts {
    TokenCounts {
        input: u64_value(usage.get("input_tokens")),
        output: u64_value(usage.get("output_tokens")),
        cache_creation: u64_value(usage.get("cache_creation_input_tokens")),
        cache_read: u64_value(usage.get("cache_read_input_tokens")),
        reasoning_output: 0,
        estimated: 0,
    }
}

/// Shared with the generic fallback: claude-shaped `message` objects appear
/// in other providers' files too.
pub(crate) fn turns_from_message_generic(role: &str, message: &Map<String, Value>) -> Vec<Turn> {
    turns_from_message(role, message)
}

/// Convert message.content into turns. Consecutive text/thinking blocks
/// merge into one turn; tool_use / tool_result become tool turns.
fn turns_from_message(role: &str, message: &Map<String, Value>) -> Vec<Turn> {
    let base_role = Role::parse(role).unwrap_or(Role::Event);
    let content = message.get("content");
    let mut turns: Vec<Turn> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();

    let flush = |parts: &mut Vec<String>, turns: &mut Vec<Turn>| {
        let text = parts.join("\n").trim().to_string();
        parts.clear();
        if !text.is_empty() {
            turns.push(Turn {
                role: base_role,
                speaker: role.to_string(),
                text,
                ..Default::default()
            });
        }
    };

    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(item) = block.as_object() else {
                    let t = content_text(block);
                    if !t.is_empty() {
                        text_parts.push(t);
                    }
                    continue;
                };
                match string_value(item.get("type")).as_str() {
                    "tool_use" => {
                        flush(&mut text_parts, &mut turns);
                        turns.push(tool_use_turn(item));
                    }
                    "tool_result" => {
                        flush(&mut text_parts, &mut turns);
                        turns.push(tool_result_turn(item));
                    }
                    "thinking" => {
                        let t = string_value(item.get("thinking"));
                        if !t.is_empty() {
                            text_parts.push(t);
                        }
                    }
                    "image" => {
                        flush(&mut text_parts, &mut turns);
                        let mut turn = Turn {
                            role: base_role,
                            speaker: role.to_string(),
                            text: "[image]".into(),
                            ..Default::default()
                        };
                        turn.provider_meta.insert("content_image".into(), Value::Bool(true));
                        turns.push(turn);
                    }
                    _ => {
                        let t = content_text(block);
                        if !t.is_empty() {
                            text_parts.push(t);
                        }
                    }
                }
            }
            flush(&mut text_parts, &mut turns);
        }
        Some(other) => {
            let t = content_text(other);
            if !t.is_empty() {
                turns.push(Turn {
                    role: base_role,
                    speaker: role.to_string(),
                    text: t,
                    ..Default::default()
                });
            }
        }
        None => {}
    }
    turns
}

fn tool_use_turn(item: &Map<String, Value>) -> Turn {
    let name = {
        let n = string_value(item.get("name"));
        if n.is_empty() {
            "tool_use".to_string()
        } else {
            n
        }
    };
    let input = item.get("input");
    let text = match input {
        Some(v) if !v.is_null() => {
            let t = content_text(v);
            if t.is_empty() {
                compact_json(v)
            } else {
                t
            }
        }
        _ => compact_json(&Value::Object(item.clone())),
    };
    let mut input_fields = BTreeMap::new();
    if let Some(input) = input.and_then(Value::as_object) {
        for key in ["command", "file_path", "path", "pattern", "prompt", "description", "query", "url", "cmd", "skill"] {
            if let Some(v) = input.get(key) {
                input_fields.insert(key.to_string(), v.clone());
            }
        }
    }
    Turn {
        role: Role::Tool,
        speaker: name.clone(),
        text,
        tool: Some(ToolInfo {
            direction: Some(ToolDirection::Use),
            name,
            pair_id: non_empty(string_value(item.get("id"))),
            status: ToolStatus::Unknown,
            input_fields,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn tool_result_turn(item: &Map<String, Value>) -> Turn {
    let text = {
        let t = content_text(&Value::Object(item.clone()));
        if t.is_empty() {
            compact_json(&Value::Object(item.clone()))
        } else {
            t
        }
    };
    let is_error = item.get("is_error").and_then(Value::as_bool);
    Turn {
        role: Role::Tool,
        speaker: "tool_result".into(),
        text,
        tokens: TokenCounts {
            estimated: rogrep_model::tokens::estimate_text_tokens(&content_text(
                item.get("content").unwrap_or(&Value::Null),
            )),
            ..Default::default()
        },
        tool: Some(ToolInfo {
            direction: Some(ToolDirection::Output),
            name: "tool_result".into(),
            pair_id: non_empty(string_value(item.get("tool_use_id"))),
            status: match is_error {
                Some(true) => ToolStatus::Failed,
                Some(false) => ToolStatus::Succeeded,
                None => ToolStatus::Succeeded,
            },
            is_error,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// `type: attachment` records become synthetic tool turns; bookkeeping
/// subtypes are dropped.
fn attachment_turn(rec: &RawRecord) -> Option<Turn> {
    let attachment = rec.object("attachment")?;
    let subtype = string_value(attachment.get("type"));
    if subtype.is_empty() || subtype == "file-history-snapshot" || subtype == "permission-mode" {
        return None;
    }
    let (kind, speaker, text) = match subtype.as_str() {
        "selected_lines_in_ide" | "opened_file_in_ide" => {
            let filename = {
                let d = string_value(attachment.get("displayPath"));
                if d.is_empty() {
                    string_value(attachment.get("filename"))
                } else {
                    d
                }
            };
            let start = crate::record::int_value(attachment.get("lineStart")).unwrap_or(0);
            let end = crate::record::int_value(attachment.get("lineEnd")).unwrap_or(0);
            let content = string_value(attachment.get("content"));
            let locator = if start > 0 && end > start {
                format!("{filename}:{start}-{end}")
            } else if start > 0 {
                format!("{filename}:{start}")
            } else {
                filename
            };
            let text = if content.is_empty() {
                locator
            } else {
                format!("{locator}\n\n{content}")
            };
            let kind = if subtype == "selected_lines_in_ide" {
                AttachmentKind::SelectedLinesInIde
            } else {
                AttachmentKind::OpenedFileInIde
            };
            (kind, "ide selection", text)
        }
        "skill_listing" => (
            AttachmentKind::SkillListing,
            "skills available",
            content_text(&Value::Object(attachment.clone())),
        ),
        "deferred_tools_delta" => (
            AttachmentKind::DeferredToolsDelta,
            "tools available",
            content_text(&Value::Object(attachment.clone())),
        ),
        other => (
            AttachmentKind::Other(other.to_string()),
            "attachment",
            {
                let t = content_text(&Value::Object(attachment.clone()));
                if t.is_empty() {
                    compact_json(&Value::Object(attachment.clone()))
                } else {
                    t
                }
            },
        ),
    };
    if text.trim().is_empty() {
        return None;
    }
    let mut fields = BTreeMap::new();
    fields.insert("subtype".to_string(), subtype.clone());
    Some(Turn {
        role: Role::Tool,
        speaker: speaker.into(),
        text: text.clone(),
        special: Some(SpecialTurn::Attachment {
            subtype: kind,
            summary: text.chars().take(160).collect(),
            fields,
        }),
        synthetic_context: true,
        provider_meta: {
            let mut m = BTreeMap::new();
            m.insert("attachment_subtype".into(), json!(subtype));
            m
        },
        ..Default::default()
    })
}
