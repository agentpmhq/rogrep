//! Cursor agent-transcript parser.
//!
//! `~/.cursor/projects/<slug>/agent-transcripts/**.jsonl` — claude-like
//! records `{role, message:{content:[blocks]}}` where the TOP-LEVEL role
//! drives (message.role is absent), user queries are wrapped in
//! `<timestamp>`/`<user_query>` XML tags, and the model is never recorded
//! (stamped as the literal "cursor"). The project slug is missing its
//! leading dash (repaired by normalized_project).

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{string_value, RawRecord};
use rogrep_model::{project, AgentKind, Role};

pub const CURSOR_PARSER_VERSION: u32 = 1;

pub struct CursorProvider;

impl Provider for CursorProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn parser_version(&self) -> u32 {
        CURSOR_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        let p = path.replace('\\', "/");
        p.contains("/.cursor/projects/") && p.contains("/agent-transcripts/") && p.ends_with(".jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        let slash = path.replace('\\', "/");
        let parts: Vec<&str> = slash.split('/').collect();
        let mut slug = String::new();
        for (i, part) in parts.iter().enumerate() {
            if *part == "projects" && i + 1 < parts.len() {
                slug = parts[i + 1].to_string();
                break;
            }
        }
        let cwd = project::slash_path_from_dash_project(&slug);
        SourceInfo {
            source_path: path.to_string(),
            project: slug,
            cwd_seed: (!cwd.is_empty()).then_some(cwd),
            subagent: None,
            default_ts: None,
        }
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(CursorParser)
    }
}

struct CursorParser;

impl RolloutParser for CursorParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let Some(message) = rec.object("message") else { return };
        // Top-level role drives; message.role is absent in cursor files.
        let role = {
            let top = crate::record::normalize_role(&rec.str_field("role"));
            if top.is_empty() {
                crate::record::normalize_role(&string_value(message.get("role")))
            } else {
                top
            }
        };
        ctx.set_model("cursor".to_string());
        for mut turn in super::claude::turns_from_message_generic(&role, message) {
            if turn.role == Role::User {
                if let Some((ts, query)) = unwrap_user_query(&turn.text) {
                    if let Some(ts) = ts {
                        turn.ts = Some(ts);
                    }
                    turn.text = query;
                }
            }
            ctx.emit(turn);
        }
    }
}

/// Unwrap `<timestamp>…</timestamp>\n<user_query>…</user_query>` blocks.
fn unwrap_user_query(text: &str) -> Option<(Option<i64>, String)> {
    let lower = text.to_lowercase();
    let q_start = lower.find("<user_query>")?;
    let q_body_start = q_start + "<user_query>".len();
    let q_end = lower[q_body_start..].find("</user_query>")?;
    let query = text[q_body_start..q_body_start + q_end].trim().to_string();
    if query.is_empty() {
        return None;
    }
    let ts = lower.find("<timestamp>").and_then(|t_start| {
        let body_start = t_start + "<timestamp>".len();
        let t_end = lower[body_start..].find("</timestamp>")?;
        parse_cursor_timestamp(text[body_start..body_start + t_end].trim())
    });
    Some((ts, query))
}

/// "Wednesday, May 27, 2026, 9:06 PM (UTC)" → unix millis.
fn parse_cursor_timestamp(label: &str) -> Option<i64> {
    let cleaned = label.replace(" (UTC)", "");
    let dt = jiff::civil::DateTime::strptime("%A, %B %d, %Y, %I:%M %p", &cleaned).ok()?;
    let zoned = dt.to_zoned(jiff::tz::TimeZone::UTC).ok()?;
    Some(zoned.timestamp().as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_parses() {
        let ts = parse_cursor_timestamp("Wednesday, May 27, 2026, 9:06 PM (UTC)").unwrap();
        let z = jiff::Timestamp::from_millisecond(ts).unwrap();
        assert_eq!(z.to_string(), "2026-05-27T21:06:00Z");
    }

    #[test]
    fn user_query_unwraps() {
        let (ts, q) = unwrap_user_query(
            "<timestamp>Wednesday, May 27, 2026, 9:06 PM (UTC)</timestamp>\n<user_query>\nExplain this codebase.\n</user_query>",
        )
        .unwrap();
        assert!(ts.is_some());
        assert_eq!(q, "Explain this codebase.");
    }
}
