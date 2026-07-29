use crate::ids::ConversationId;
use crate::tokens::TokenCounts;
use crate::turn::Turn;
use crate::UnixMillis;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Claude,
    ClaudeCowork,
    Codex,
    Cursor,
    Grok,
    Hermes,
    Opencode,
    Generic,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::ClaudeCowork => "claude-cowork",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::Grok => "grok",
            AgentKind::Hermes => "hermes",
            AgentKind::Opencode => "opencode",
            AgentKind::Generic => "generic",
        }
    }

    pub fn parse(s: &str) -> Option<AgentKind> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "claude-cowork" => Some(AgentKind::ClaudeCowork),
            "codex" => Some(AgentKind::Codex),
            "cursor" => Some(AgentKind::Cursor),
            "grok" => Some(AgentKind::Grok),
            "hermes" => Some(AgentKind::Hermes),
            "opencode" => Some(AgentKind::Opencode),
            "generic" => Some(AgentKind::Generic),
            _ => None,
        }
    }

    pub const ALL: [AgentKind; 8] = [
        AgentKind::Claude,
        AgentKind::ClaudeCowork,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Grok,
        AgentKind::Hermes,
        AgentKind::Opencode,
        AgentKind::Generic,
    ];
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Interactive,
    Subagent,
    Scheduled,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::Interactive => "interactive",
            Origin::Subagent => "subagent",
            Origin::Scheduled => "scheduled",
        }
    }
}

/// Subagent lineage (claude subagent transcripts live in a sibling dir and
/// link back to a parent conversation).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentLink {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<ConversationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
}

/// A fully parsed conversation: summary metadata + normalized turns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub agent: AgentKind,
    pub source_path: String,
    /// Provider title (AI title / session title) if any; presentation falls
    /// back to the first visible user turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// First model seen; individual turns may differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Raw provider project slug (claude dash-encoding etc.), "" if unknown.
    pub project: String,
    /// Cross-provider normalized project key (see project.rs).
    pub normalized_project: String,
    /// FIRST cwd seen — cd drift never moves the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<UnixMillis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<UnixMillis>,
    /// Conversation-level totals. Provider-reported totals (codex, hermes,
    /// opencode) override turn sums when present.
    pub tokens: TokenCounts,
    pub malformed_lines: u32,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentLink>,
    pub turns: Vec<Turn>,
}

impl Conversation {
    /// Presentation title: provider title if real, else first visible user
    /// turn's first line, else the id.
    pub fn display_title(&self) -> String {
        if let Some(t) = &self.title {
            let t = t.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        for turn in &self.turns {
            if turn.role == crate::turn::Role::User && crate::turn::is_visible_turn(turn) {
                let line = turn.text.lines().next().unwrap_or("").trim();
                if !line.is_empty() {
                    let mut s: String = line.chars().take(120).collect();
                    if line.chars().count() > 120 {
                        s.push('…');
                    }
                    return s;
                }
            }
        }
        self.id.to_string()
    }
}
