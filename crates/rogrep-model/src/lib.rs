//! Normalized data model for rogrep.
//!
//! Everything downstream (parsers, store, index, CLI) speaks these types.
//! This crate has no I/O beyond path/config resolution helpers.

pub mod config;
pub mod conversation;
pub mod exchange;
pub mod ids;
pub mod paths;
pub mod project;
pub mod remote;
pub mod special;
pub mod tokens;
pub mod turn;

pub use conversation::{AgentKind, Conversation, Origin, SubagentLink};
pub use exchange::{build_exchanges, is_real_user_prompt, Exchange, ExchangeSignals};
pub use ids::ConversationId;
pub use special::{AttachmentKind, SpecialTurn};
pub use tokens::TokenCounts;
pub use turn::{Role, SourceSpan, ToolDirection, ToolInfo, ToolStatus, Turn};

/// Timestamps are unix milliseconds UTC throughout the model and the store.
pub type UnixMillis = i64;

/// Parse an RFC3339-ish timestamp into unix millis. Returns None on failure.
pub fn parse_timestamp(s: &str) -> Option<UnixMillis> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    // Some providers write bare unix seconds or millis.
    if let Ok(n) = s.parse::<i64>() {
        return millis_from_number(n as f64);
    }
    None
}

/// Interpret a numeric timestamp (seconds, millis, micros) as unix millis.
pub fn millis_from_number(n: f64) -> Option<UnixMillis> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    // Heuristic: seconds < 1e11 < millis < 1e14 < micros.
    let ms = if n < 1e11 {
        n * 1000.0
    } else if n < 1e14 {
        n
    } else {
        n / 1000.0
    };
    Some(ms as i64)
}
