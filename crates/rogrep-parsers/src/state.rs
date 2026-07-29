//! Seedable parse state — the incremental checkpoint contract.
//!
//! The watermark freezes at the START of the last exchange (the record that
//! emitted the last real user prompt). Everything before it is immutable;
//! the open tail is re-parsed and replaced on every sync. Late-arriving
//! records (tool results, cumulative usage) may therefore only amend turns
//! inside the open exchange — the driver enforces that barrier identically
//! in full and seeded parses, which is what makes
//! `parse(full) == parse(prefix) ⊕ resume(tail)` exact.

use rogrep_model::{Origin, TokenCounts, UnixMillis};
use serde::{Deserialize, Serialize};

/// Rewrite/truncation detector: length + xxh3 of the last up-to-4KiB before
/// the resume offset. If the file shrank or the tail-of-prefix changed, the
/// file was rewritten in place and the checkpoint is invalid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixFingerprint {
    pub offset: u64,
    pub hash: u64,
}

pub const FINGERPRINT_WINDOW: usize = 4096;

impl PrefixFingerprint {
    pub fn compute(prefix_tail: &[u8], offset: u64) -> Self {
        PrefixFingerprint {
            offset,
            hash: xxhash_rust::xxh3::xxh3_64(prefix_tail),
        }
    }
}

/// Frozen-part rollup as of the watermark.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrozenSummary {
    pub turn_count: u32,
    pub tokens: TokenCounts,
    pub malformed_lines: u32,
    pub first_seen: Option<UnixMillis>,
    pub last_seen: Option<UnixMillis>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParseState {
    /// Provider parser version at checkpoint time; mismatch → full reparse.
    pub parser_version: u32,
    /// 1-based number of the next line to read.
    pub line_number: u64,
    /// Byte offset of the next line to read (== fingerprint.offset).
    pub byte_offset: u64,
    /// Turn index the next emitted turn will get; also the tail-refresh
    /// watermark (stored turns >= this index get replaced).
    pub next_turn_index: u32,
    pub frozen: FrozenSummary,

    // Conversation-level signals as of the watermark.
    pub conversation_cwd: Option<String>,
    /// True once an in-record cwd claimed the conversation cwd (path-derived
    /// seeds are overridable, record cwds are first-wins).
    pub cwd_from_record: bool,
    pub current_cwd: Option<String>,
    pub conversation_model: Option<String>,
    pub current_model: Option<String>,
    pub title: Option<String>,
    pub origin: Origin,

    pub fingerprint: PrefixFingerprint,
    /// Provider-private serialized state (codex usage carry etc.).
    pub provider_state: serde_json::Value,
}

impl ParseState {
    pub fn fresh(parser_version: u32) -> Self {
        ParseState {
            parser_version,
            line_number: 1,
            byte_offset: 0,
            provider_state: serde_json::Value::Null,
            ..Default::default()
        }
    }
}
