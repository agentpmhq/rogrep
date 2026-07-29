//! The remote-analysis seam.
//!
//! rogrep ships no LLM features. A future subscription backend can implement
//! `AnalysisBackend`; everything it may see must pass through
//! `RedactedTranscript`, the single choke point where redaction will live.
//! The only implementation in this codebase is `DisabledBackend`.

use crate::ids::ConversationId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisKind {
    ConversationSummary,
    ExchangeSummary,
    DailyDigest,
}

/// Placeholder for the future redaction pass. Deliberately opaque: callers
/// construct it from a conversation, backends consume it, and redaction
/// policy will be applied in the constructor when a real backend exists.
pub struct RedactedTranscript {
    pub conversation: ConversationId,
    pub text: String,
}

pub struct AnalysisRequest {
    pub kind: AnalysisKind,
    pub payload: RedactedTranscript,
}

pub struct AnalysisResponse {
    pub markdown: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("remote analysis is not configured; rogrep is local-only by default (see [remote] in config)")]
    NotConfigured,
    #[error("remote analysis failed: {0}")]
    Backend(String),
}

pub trait AnalysisBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn analyze(&self, req: AnalysisRequest) -> Result<AnalysisResponse, RemoteError>;
}

/// The default (and only) backend: always declines.
pub struct DisabledBackend;

impl AnalysisBackend for DisabledBackend {
    fn id(&self) -> &'static str {
        "disabled"
    }

    fn analyze(&self, _req: AnalysisRequest) -> Result<AnalysisResponse, RemoteError> {
        Err(RemoteError::NotConfigured)
    }
}
