use crate::special::SpecialTurn;
use crate::tokens::TokenCounts;
use crate::UnixMillis;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
    Event,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
            Role::Event => "event",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "tool" => Some(Role::Tool),
            "system" => Some(Role::System),
            "event" => Some(Role::Event),
            _ => None,
        }
    }
}

/// Byte/line position of the source record(s) that produced a turn.
/// Offsets are within the source JSONL file; lines are 1-based.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub line: u64,
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDirection {
    /// The agent invoking a tool (the call).
    Use,
    /// The tool's result coming back.
    Output,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    Rejected,
    Interrupted,
    #[default]
    Unknown,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStatus::Succeeded => "succeeded",
            ToolStatus::Failed => "failed",
            ToolStatus::Rejected => "rejected",
            ToolStatus::Interrupted => "interrupted",
            ToolStatus::Unknown => "unknown",
        }
    }
}

/// Tool call / tool result payload attached to a `Role::Tool` turn.
/// Calls and results are both turns; `pair_id` links them (provider-specific
/// ids — claude tool_use_id, codex call_id, cursor/grok/hermes variants —
/// unified into one field).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolInfo {
    pub direction: Option<ToolDirection>,
    /// Normalized tool name ("Bash", "Read", "tool_result", mcp tool names…).
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<String>,
    pub status: ToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Selected structured input fields (command, file_path, pattern, …) kept
    /// as raw JSON for facet extraction and display.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub input_fields: BTreeMap<String, serde_json::Value>,
}

/// One normalized turn. Turn indexes are dense (0..n) and strictly additive
/// for append-only sources — the search index relies on that.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Turn {
    pub turn_index: u32,
    pub role: Role,
    /// Finer-grained speaker: tool name, "tool_result", "turn_aborted",
    /// "task_notification", subagent label, or "" when the role suffices.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub speaker: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<UnixMillis>,
    /// Live cwd at this turn (tracks `cd` drift within a session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub tokens: TokenCounts,
    pub source: SourceSpan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special: Option<SpecialTurn>,
    /// Synthetic context injected by the harness (attachments, IDE selection,
    /// skill listings) rather than authored by user/agent.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub synthetic_context: bool,
    /// Provider-specific metadata worth keeping (uuids, session ids, part
    /// ids). Small allow-listed bag, not a dumping ground.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_meta: BTreeMap<String, serde_json::Value>,
}

impl Default for Role {
    fn default() -> Self {
        Role::System
    }
}

impl Turn {
    pub fn is_tool_output(&self) -> bool {
        matches!(
            self.tool,
            Some(ToolInfo {
                direction: Some(ToolDirection::Output),
                ..
            })
        )
    }

    pub fn is_tool_use(&self) -> bool {
        matches!(
            self.tool,
            Some(ToolInfo {
                direction: Some(ToolDirection::Use),
                ..
            })
        )
    }
}

/// Injected-context detection: harness-generated blocks that should not count
/// as user-authored text for titles/snippets/visibility (they stay in the
/// transcript). Port of agentpm's isInjectedPublicContextText.
pub fn is_injected_context_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    const PREFIXES: &[&str] = &[
        "<environment_context>",
        "<permissions instructions>",
        "<collaboration_mode>",
        "<apps_instructions>",
        "<skills_instructions>",
        "<plugins_instructions>",
        "<turn_aborted>",
        "<system-reminder>",
        "<local-command-caveat>",
        "<local-command-stdout>",
        "<command-name>",
        "<user_instructions>",
        "<environment_details>",
    ];
    if PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
        return true;
    }
    trimmed.starts_with("# AGENTS.md instructions for ")
}

/// Is this turn visible in default (public) renderings? Drops empty turns,
/// injected context, and system noise, keeping meaningful system speakers.
pub fn is_visible_turn(turn: &Turn) -> bool {
    if turn.text.trim().is_empty() && turn.special.is_none() {
        return false;
    }
    if turn.synthetic_context {
        return false;
    }
    if is_injected_context_text(&turn.text) {
        return false;
    }
    if turn.role == Role::System {
        return matches!(
            turn.speaker.as_str(),
            "rogrep" | "error" | "tool_rejected" | "turn_aborted" | "task_notification"
                | "scheduled_prompt"
        ) || turn.special.is_some();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_context_detected() {
        assert!(is_injected_context_text("  <environment_context>x"));
        assert!(is_injected_context_text("# AGENTS.md instructions for /x"));
        assert!(!is_injected_context_text("please fix the bug"));
    }

    #[test]
    fn visibility_rules() {
        let mut t = Turn {
            role: Role::User,
            text: "hello".into(),
            ..Default::default()
        };
        assert!(is_visible_turn(&t));
        t.text = "<system-reminder>noise".into();
        assert!(!is_visible_turn(&t));
        let sys = Turn {
            role: Role::System,
            speaker: "compiler".into(),
            text: "warning".into(),
            ..Default::default()
        };
        assert!(!is_visible_turn(&sys));
        let aborted = Turn {
            role: Role::System,
            speaker: "turn_aborted".into(),
            text: "aborted".into(),
            ..Default::default()
        };
        assert!(is_visible_turn(&aborted));
    }
}
