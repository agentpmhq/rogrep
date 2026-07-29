//! Generic fallback parser for unknown JSONL: best-effort role/text/timestamp
//! extraction so unrecognized agent formats still index usefully.

use crate::driver::{ParseCtx, Provider, RolloutParser, SourceInfo};
use crate::record::{extract_role, extract_text, RawRecord};
use rogrep_model::{AgentKind, Role, Turn};

pub const GENERIC_PARSER_VERSION: u32 = 1;

pub struct GenericProvider;

impl Provider for GenericProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::Generic
    }

    fn parser_version(&self) -> u32 {
        GENERIC_PARSER_VERSION
    }

    fn claims_path(&self, path: &str) -> bool {
        path.ends_with(".jsonl")
    }

    fn source_info(&self, path: &str) -> SourceInfo {
        SourceInfo {
            source_path: path.to_string(),
            project: String::new(),
            cwd_seed: None,
            subagent: None,
            default_ts: None,
        }
    }

    fn new_parser(&self) -> Box<dyn RolloutParser> {
        Box::new(GenericParser)
    }
}

struct GenericParser;

impl RolloutParser for GenericParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>) {
        let cwd = rec.str_field("cwd");
        if cwd.starts_with('/') {
            ctx.set_cwd(cwd);
        }
        // Claude-shaped records (nested message object) get the block
        // treatment via the claude message helper; else flat extraction.
        if let Some(message) = rec.object("message") {
            let role = crate::record::normalize_role(&crate::record::string_value(message.get("role")));
            for turn in super::claude::turns_from_message_generic(&role, message) {
                ctx.emit(turn);
            }
            return;
        }
        let text = extract_text(&rec.obj);
        if text.is_empty() {
            return;
        }
        let role_str = extract_role(&rec.obj);
        let role = Role::parse(&role_str).unwrap_or(Role::Event);
        ctx.emit(Turn {
            role,
            speaker: if role_str.is_empty() { "event".into() } else { role_str },
            text,
            ..Default::default()
        });
    }
}
