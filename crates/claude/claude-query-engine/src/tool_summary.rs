//! Tool result summary generation.
//!
//! Provides LLM-style summarization of tool results to keep the conversation
//! context manageable. When tool output is large, a summary replaces the
//! full output in the conversation sent to the provider.

use serde::{Deserialize, Serialize};

/// Configuration for tool result summarization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultSummarizer {
    /// Maximum length of a tool result before it gets summarized.
    max_result_length: usize,
    /// Maximum length of the generated summary.
    max_summary_length: usize,
    /// Whether summarization is enabled.
    enabled: bool,
}

impl ToolResultSummarizer {
    /// Create a new summarizer with the given length thresholds.
    #[must_use]
    pub fn new(max_result_length: usize, max_summary_length: usize) -> Self {
        Self {
            max_result_length,
            max_summary_length,
            enabled: true,
        }
    }

    /// Returns whether summarization is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable summarization.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable summarization.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Returns the maximum result length threshold.
    #[must_use]
    pub fn max_result_length(&self) -> usize {
        self.max_result_length
    }

    /// Returns the maximum summary length.
    #[must_use]
    pub fn max_summary_length(&self) -> usize {
        self.max_summary_length
    }

    /// Check if a tool result needs summarization.
    #[must_use]
    pub fn needs_summary(&self, content: &str) -> bool {
        self.enabled && content.len() > self.max_result_length
    }

    /// Summarize a tool result if it exceeds the threshold.
    /// Returns the original content if it's within limits or summarization is disabled.
    #[must_use]
    pub fn summarize(&self, content: &str) -> SummaryResult {
        if !self.needs_summary(content) {
            return SummaryResult::NotNeeded {
                content: content.to_owned(),
            };
        }

        let summary = self.generate_summary(content);
        SummaryResult::Summarized {
            original_length: content.len(),
            summary,
            truncated: true,
        }
    }

    /// Generate a summary from the content.
    fn generate_summary(&self, content: &str) -> String {
        // Simple heuristic-based summarization:
        // 1. Take the first N characters
        // 2. Add a truncation marker
        // 3. Take the last M characters for tail context
        let marker_overhead = 40; // approximate size of the marker text
        let available = self.max_summary_length.saturating_sub(marker_overhead);
        let head_size = (available * 7) / 10; // 70% head
        let tail_size = available.saturating_sub(head_size); // 30% tail

        if head_size == 0 || tail_size == 0 {
            return content.chars().take(self.max_summary_length).collect();
        }

        let head: String = content.chars().take(head_size).collect();
        let tail: String = content
            .chars()
            .rev()
            .take(tail_size)
            .collect::<String>()
            .chars()
            .rev()
            .collect();

        let omitted = content
            .len()
            .saturating_sub(head.len())
            .saturating_sub(tail.len());
        format!("{head}\n\n... [{omitted} characters omitted] ...\n\n{tail}")
    }
}

/// Result of a summarization attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryResult {
    /// The content was within limits and doesn't need summarization.
    NotNeeded { content: String },
    /// The content was summarized.
    Summarized {
        original_length: usize,
        summary: String,
        truncated: bool,
    },
}

impl SummaryResult {
    /// Returns the text content (either original or summary).
    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            Self::NotNeeded { content } => content,
            Self::Summarized { summary, .. } => summary,
        }
    }

    /// Returns true if the content was summarized.
    #[must_use]
    pub fn was_summarized(&self) -> bool {
        matches!(self, Self::Summarized { .. })
    }
}

impl Default for ToolResultSummarizer {
    fn default() -> Self {
        Self::new(10_000, 2_000)
    }
}

#[cfg(test)]
mod tests {
    use super::{SummaryResult, ToolResultSummarizer};

    #[test]
    fn summarizer_does_not_summarize_short_content() {
        let summarizer = ToolResultSummarizer::new(100, 50);
        let result = summarizer.summarize("short content");
        assert!(!result.was_summarized());
        assert_eq!(result.content(), "short content");
    }

    #[test]
    fn summarizer_summarizes_long_content() {
        let summarizer = ToolResultSummarizer::new(50, 30);
        let long_content = "a".repeat(200);
        let result = summarizer.summarize(&long_content);
        assert!(result.was_summarized());
        if let SummaryResult::Summarized {
            original_length, ..
        } = &result
        {
            assert_eq!(*original_length, 200);
        }
    }

    #[test]
    fn summarizer_respects_disabled_state() {
        let mut summarizer = ToolResultSummarizer::new(10, 5);
        summarizer.disable();
        let long = "a".repeat(100);
        let result = summarizer.summarize(&long);
        assert!(!result.was_summarized());
    }

    #[test]
    fn summarizer_needs_summary_check() {
        let summarizer = ToolResultSummarizer::new(100, 50);
        assert!(!summarizer.needs_summary("short"));
        assert!(summarizer.needs_summary(&"x".repeat(200)));
    }

    #[test]
    fn summarizer_default_values() {
        let summarizer = ToolResultSummarizer::default();
        assert!(summarizer.is_enabled());
        assert_eq!(summarizer.max_result_length(), 10_000);
        assert_eq!(summarizer.max_summary_length(), 2_000);
    }

    #[test]
    fn summary_result_content_accessor() {
        let not_needed = SummaryResult::NotNeeded {
            content: "hello".to_string(),
        };
        assert_eq!(not_needed.content(), "hello");

        let summarized = SummaryResult::Summarized {
            original_length: 100,
            summary: "summary".to_string(),
            truncated: true,
        };
        assert_eq!(summarized.content(), "summary");
    }
}
