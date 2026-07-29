use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Stable conversation identifier: `rg_` + first 24 hex chars of
/// sha256(source_path). Derived purely from the source path so the id is
/// linkable the moment a session file exists, and stable across re-indexing.
/// (agentpm: `goc_` + sha256(agent_id || "\0" || path)[..24]; the agent
/// dimension collapses on a single machine.)
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(pub String);

pub const ID_PREFIX: &str = "rg_";
const ID_HEX_LEN: usize = 24;

impl ConversationId {
    pub fn from_source_path(path: &str) -> Self {
        let digest = Sha256::digest(path.as_bytes());
        let mut hex = String::with_capacity(ID_PREFIX.len() + ID_HEX_LEN);
        hex.push_str(ID_PREFIX);
        for byte in digest.iter() {
            if hex.len() >= ID_PREFIX.len() + ID_HEX_LEN {
                break;
            }
            hex.push_str(&format!("{byte:02x}"));
        }
        hex.truncate(ID_PREFIX.len() + ID_HEX_LEN);
        ConversationId(hex)
    }

    /// Sub-stream id (e.g. codex history sessions split out of one file).
    pub fn with_suffix(&self, suffix: &str) -> Self {
        ConversationId(format!("{}:{suffix}", self.0))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Does a bare query token look like a conversation id? Used for the
    /// exact-id search short-circuit.
    pub fn looks_like_id(token: &str) -> bool {
        let Some(rest) = token.strip_prefix(ID_PREFIX) else {
            return false;
        };
        let head = rest.split(':').next().unwrap_or(rest);
        head.len() == ID_HEX_LEN && head.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reference to a specific exchange within a conversation, e.g. `rg_ab12..#e3`.
/// Exchange ordinals are 1-based in user-facing ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeRef {
    pub conversation: ConversationId,
    pub ordinal: u32,
}

impl ExchangeRef {
    pub fn parse(s: &str) -> Option<ExchangeRef> {
        let (conv, ex) = s.split_once("#e")?;
        if !ConversationId::looks_like_id(conv) {
            return None;
        }
        let ordinal: u32 = ex.parse().ok()?;
        if ordinal == 0 {
            return None;
        }
        Some(ExchangeRef {
            conversation: ConversationId(conv.to_string()),
            ordinal,
        })
    }
}

impl fmt::Display for ExchangeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#e{}", self.conversation, self.ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_and_shaped() {
        let a = ConversationId::from_source_path("/home/u/.claude/projects/-home-u/x.jsonl");
        let b = ConversationId::from_source_path("/home/u/.claude/projects/-home-u/x.jsonl");
        assert_eq!(a, b);
        assert!(ConversationId::looks_like_id(a.as_str()));
        assert_eq!(a.as_str().len(), ID_PREFIX.len() + ID_HEX_LEN);
    }

    #[test]
    fn distinct_paths_distinct_ids() {
        let a = ConversationId::from_source_path("/a.jsonl");
        let b = ConversationId::from_source_path("/b.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn exchange_ref_roundtrip() {
        let id = ConversationId::from_source_path("/a.jsonl");
        let r = ExchangeRef {
            conversation: id.clone(),
            ordinal: 14,
        };
        let s = r.to_string();
        assert_eq!(ExchangeRef::parse(&s), Some(r));
        assert_eq!(ExchangeRef::parse("nonsense#e3"), None);
        assert_eq!(ExchangeRef::parse(&format!("{id}#e0")), None);
    }

    #[test]
    fn suffix_ids_still_look_like_ids() {
        let id = ConversationId::from_source_path("/a.jsonl").with_suffix("h1");
        assert!(ConversationId::looks_like_id(id.as_str()));
    }
}
