use serde::{Deserialize, Serialize};

/// Per-turn / per-conversation token accounting.
///
/// Provider-reported usage populates the primary counters; when no usage is
/// available, a length/4 estimate lands in `estimated` (and role-bucketed
/// estimates in the `*_est` view) so totals degrade gracefully instead of
/// reading as zero. Mirrors agentpm's conversationTokenCounts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub reasoning_output: u64,
    /// Length-based estimate for turns with no provider usage.
    pub estimated: u64,
}

impl TokenCounts {
    pub fn is_zero(&self) -> bool {
        *self == TokenCounts::default()
    }

    /// Provider-accounted total (excludes estimates).
    pub fn accounted(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }

    /// Best-effort total for display: accounted if present, else estimate.
    pub fn display_total(&self) -> u64 {
        let accounted = self.accounted();
        if accounted > 0 {
            accounted
        } else {
            self.estimated
        }
    }

    pub fn add(&mut self, other: &TokenCounts) {
        self.input += other.input;
        self.output += other.output;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
        self.reasoning_output += other.reasoning_output;
        self.estimated += other.estimated;
    }

    /// Saturating delta against a previous cumulative snapshot (codex reports
    /// cumulative token_count records; per-turn usage is the delta).
    pub fn saturating_delta(&self, prev: &TokenCounts) -> TokenCounts {
        TokenCounts {
            input: self.input.saturating_sub(prev.input),
            output: self.output.saturating_sub(prev.output),
            cache_creation: self.cache_creation.saturating_sub(prev.cache_creation),
            cache_read: self.cache_read.saturating_sub(prev.cache_read),
            reasoning_output: self.reasoning_output.saturating_sub(prev.reasoning_output),
            estimated: 0,
        }
    }
}

/// The canonical text-length token estimate (agentpm: len/4).
pub fn estimate_text_tokens(text: &str) -> u64 {
    (text.len() as u64) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_saturates() {
        let prev = TokenCounts {
            input: 100,
            output: 50,
            ..Default::default()
        };
        let cur = TokenCounts {
            input: 150,
            output: 40, // provider reset — must not underflow
            ..Default::default()
        };
        let d = cur.saturating_delta(&prev);
        assert_eq!(d.input, 50);
        assert_eq!(d.output, 0);
    }

    #[test]
    fn display_total_prefers_accounted() {
        let t = TokenCounts {
            input: 10,
            estimated: 99,
            ..Default::default()
        };
        assert_eq!(t.display_total(), 10);
        let e = TokenCounts {
            estimated: 99,
            ..Default::default()
        };
        assert_eq!(e.display_total(), 99);
    }
}
