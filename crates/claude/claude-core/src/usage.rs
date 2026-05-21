use serde::{Deserialize, Serialize};

use crate::UsageSummary;

/// Mutable usage tracker shared across multi-turn operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageAccumulator {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub server_tool_use_web_search_requests: u64,
    #[serde(default)]
    pub server_tool_use_web_fetch_requests: u64,
    #[serde(default)]
    pub cache_creation_ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_ephemeral_1h_input_tokens: u64,
    #[serde(default)]
    pub requests: u64,
}

impl UsageAccumulator {
    /// Total observed tokens across prompt, completion, and cache surfaces.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_input_tokens
            + self.cache_creation_input_tokens
    }

    /// Record provider usage from a single response.
    pub fn record_summary(&mut self, summary: &UsageSummary) {
        self.input_tokens += summary.input_tokens;
        self.output_tokens += summary.output_tokens;
        self.cache_read_input_tokens += summary.cache_read_input_tokens;
        self.cache_creation_input_tokens += summary.cache_creation_input_tokens;
        self.server_tool_use_web_search_requests += summary.server_tool_use_web_search_requests;
        self.server_tool_use_web_fetch_requests += summary.server_tool_use_web_fetch_requests;
        self.cache_creation_ephemeral_5m_input_tokens +=
            summary.cache_creation_ephemeral_5m_input_tokens;
        self.cache_creation_ephemeral_1h_input_tokens +=
            summary.cache_creation_ephemeral_1h_input_tokens;
        self.requests += 1;
    }

    /// Merge another accumulator into this one.
    pub fn merge(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.server_tool_use_web_search_requests += other.server_tool_use_web_search_requests;
        self.server_tool_use_web_fetch_requests += other.server_tool_use_web_fetch_requests;
        self.cache_creation_ephemeral_5m_input_tokens +=
            other.cache_creation_ephemeral_5m_input_tokens;
        self.cache_creation_ephemeral_1h_input_tokens +=
            other.cache_creation_ephemeral_1h_input_tokens;
        self.requests += other.requests;
    }
}

#[cfg(test)]
mod tests {
    use super::UsageAccumulator;
    use crate::UsageSummary;

    #[test]
    fn usage_accumulator_merges_and_counts_requests() {
        let mut usage = UsageAccumulator::default();
        usage.record_summary(&UsageSummary {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 5,
            cache_creation_input_tokens: 1,
            ..Default::default()
        });

        let mut other = UsageAccumulator::default();
        other.record_summary(&UsageSummary {
            input_tokens: 2,
            output_tokens: 3,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            ..Default::default()
        });

        usage.merge(&other);
        assert_eq!(usage.requests, 2);
        assert_eq!(usage.total_tokens(), 41);
    }
}
