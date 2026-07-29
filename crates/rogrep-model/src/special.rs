use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Harness/system event turns that need distinct semantics — most
/// importantly, user-shaped records that must NOT open a new exchange
/// (task notifications, scheduled prompts, compact boundaries).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpecialTurn {
    TaskNotification {
        queued: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        summary: String,
        /// De-dupes queued-vs-delivered echoes of the same notification.
        signature: String,
    },
    ScheduledPrompt {
        queued: bool,
        summary: String,
        signature: String,
    },
    CompactBoundary,
    Attachment {
        subtype: AttachmentKind,
        summary: String,
        #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
        fields: BTreeMap<String, String>,
    },
    TurnAborted {
        reason: String,
        /// The user turn this abort answers, when identifiable.
        #[serde(skip_serializing_if = "Option::is_none")]
        aborted_user_turn: Option<u32>,
    },
    Other {
        label: String,
        #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
        fields: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    SelectedLinesInIde,
    OpenedFileInIde,
    SkillListing,
    DeferredToolsDelta,
    Other(String),
}

impl SpecialTurn {
    /// Specials that must not open a new exchange even when the record is
    /// user-shaped.
    pub fn suppresses_exchange_boundary(&self) -> bool {
        matches!(
            self,
            SpecialTurn::TaskNotification { .. }
                | SpecialTurn::ScheduledPrompt { .. }
                | SpecialTurn::CompactBoundary
        )
    }
}
