//! Tool result summarization and truncation.
//!
//! Provides [`ToolResultSummarizer`] which can truncate large tool outputs
//! into a head + tail preview with a configurable budget.  This keeps the
//! conversation context manageable without losing the most relevant parts
//! of the output.

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Summary result
// ---------------------------------------------------------------------------

/// The outcome of a summarization pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SummaryOutcome {
    /// Content was within budget – no summarization needed.
    NotNeeded { content: String },
    /// Content was truncated to fit within the budget.
    Truncated {
        original_length: usize,
        preview: String,
        tail: String,
    },
    /// Content was replaced by a generated summary.
    Summarized {
        original_length: usize,
        summary: String,
    },
}

impl SummaryOutcome {
    /// Return the effective content string.
    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            Self::NotNeeded { content } => content,
            Self::Truncated { preview, .. } => preview,
            Self::Summarized { summary, .. } => summary,
        }
    }

    /// Original length of the content before summarization.
    #[must_use]
    pub fn original_length(&self) -> usize {
        match self {
            Self::NotNeeded { content } => content.len(),
            Self::Truncated {
                original_length, ..
            } => *original_length,
            Self::Summarized {
                original_length, ..
            } => *original_length,
        }
    }

    /// Whether summarization was applied.
    #[must_use]
    pub fn was_summarized(&self) -> bool {
        !matches!(self, Self::NotNeeded { .. })
    }
}

// ---------------------------------------------------------------------------
// ToolResultSummarizer
// ---------------------------------------------------------------------------

/// Configuration for tool result summarization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultSummarizer {
    /// Maximum length of a tool result before it gets truncated.
    max_result_length: usize,
    /// Maximum length of the generated preview (head portion).
    max_preview_length: usize,
    /// Maximum length of the tail portion.
    max_tail_length: usize,
    /// Whether summarization is enabled.
    enabled: bool,
    /// The truncation marker inserted between head and tail.
    marker: String,
}

impl ToolResultSummarizer {
    /// Create a new summarizer with the given length thresholds.
    #[must_use]
    pub fn new(
        max_result_length: usize,
        max_preview_length: usize,
        max_tail_length: usize,
    ) -> Self {
        Self {
            max_result_length,
            max_preview_length,
            max_tail_length,
            enabled: true,
            marker: "\n\n... [truncated] ...\n\n".into(),
        }
    }

    /// Create with sensible defaults (10 KB result limit, 2 KB preview, 1 KB tail).
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(10_000, 2_000, 1_000)
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

    /// Returns the maximum preview length.
    #[must_use]
    pub fn max_preview_length(&self) -> usize {
        self.max_preview_length
    }

    /// Returns the maximum tail length.
    #[must_use]
    pub fn max_tail_length(&self) -> usize {
        self.max_tail_length
    }

    /// Set the truncation marker.
    pub fn set_marker(&mut self, marker: impl Into<String>) {
        self.marker = marker.into();
    }

    /// Check if a tool result needs summarization.
    #[must_use]
    pub fn needs_summary(&self, content: &str) -> bool {
        self.enabled && content.len() > self.max_result_length
    }

    /// Summarize a tool result if it exceeds the threshold.
    ///
    /// Returns [`SummaryOutcome::NotNeeded`] if the content is within limits
    /// or summarization is disabled.
    #[must_use]
    pub fn summarize(&self, content: &str) -> SummaryOutcome {
        if !self.needs_summary(content) {
            return SummaryOutcome::NotNeeded {
                content: content.to_owned(),
            };
        }
        self.truncate(content)
    }

    /// Force-truncate regardless of the enabled flag.
    #[must_use]
    pub fn truncate(&self, content: &str) -> SummaryOutcome {
        let original_length = content.len();

        // Take the first `max_preview_length` chars for the head
        let preview: String = content.chars().take(self.max_preview_length).collect();

        // Take the last `max_tail_length` chars for the tail
        let tail_chars: Vec<char> = content.chars().rev().take(self.max_tail_length).collect();
        let tail: String = tail_chars.into_iter().rev().collect();

        SummaryOutcome::Truncated {
            original_length,
            preview,
            tail,
        }
    }

    /// Generate a structured summary with line counts and key snippets.
    #[must_use]
    pub fn structured_summary(&self, content: &str) -> StructuredSummary {
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let total_bytes = content.len();

        let error_lines: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let lower = line.to_lowercase();
                lower.contains("error") || lower.contains("failed") || lower.contains("panic")
            })
            .map(|(i, _)| i + 1)
            .collect();

        let warning_lines: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let lower = line.to_lowercase();
                lower.contains("warning") && !lower.contains("error")
            })
            .map(|(i, _)| i + 1)
            .collect();

        // Extract first few error lines as snippets
        let error_snippets: Vec<String> = error_lines
            .iter()
            .take(5)
            .map(|&i| {
                let line = lines.get(i - 1).copied().unwrap_or("");
                if line.len() > 200 {
                    format!("L{i}: {}...", &line[..200])
                } else {
                    format!("L{i}: {line}")
                }
            })
            .collect();

        StructuredSummary {
            total_lines,
            total_bytes,
            error_count: error_lines.len(),
            warning_count: warning_lines.len(),
            error_snippets,
            preview: content.lines().take(5).collect::<Vec<&str>>().join("\n"),
        }
    }
}

impl Default for ToolResultSummarizer {
    fn default() -> Self {
        Self::default_config()
    }
}

impl fmt::Display for ToolResultSummarizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ToolResultSummarizer(max={}B, preview={}B, tail={}B, enabled={})",
            self.max_result_length, self.max_preview_length, self.max_tail_length, self.enabled
        )
    }
}

// ---------------------------------------------------------------------------
// StructuredSummary
// ---------------------------------------------------------------------------

/// A structured summary of tool output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuredSummary {
    /// Total number of lines in the original output.
    pub total_lines: usize,
    /// Total bytes in the original output.
    pub total_bytes: usize,
    /// Number of lines containing "error" / "failed" / "panic".
    pub error_count: usize,
    /// Number of lines containing "warning".
    pub warning_count: usize,
    /// First few error line snippets.
    pub error_snippets: Vec<String>,
    /// First 5 lines preview.
    pub preview: String,
}

impl fmt::Display for StructuredSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} lines, {} bytes, {} errors, {} warnings",
            self.total_lines, self.total_bytes, self.error_count, self.warning_count
        )?;
        if !self.error_snippets.is_empty() {
            write!(f, "\nFirst errors:")?;
            for snippet in &self.error_snippets {
                write!(f, "\n  {snippet}")?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Truncate content to a maximum byte length, respecting char boundaries.
#[must_use]
pub fn truncate_to_bytes(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_owned();
    }
    let mut end = max_bytes;
    while !content.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    content[..end].to_owned()
}

/// Count the number of lines in content.
#[must_use]
pub fn count_lines(content: &str) -> usize {
    content.lines().count()
}

/// Extract the first N lines of content.
#[must_use]
pub fn head_lines(content: &str, n: usize) -> String {
    content.lines().take(n).collect::<Vec<&str>>().join("\n")
}

/// Extract the last N lines of content.
#[must_use]
pub fn tail_lines(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- SummaryOutcome -----------------------------------------------------

    #[test]
    fn outcome_not_needed_content() {
        let o = SummaryOutcome::NotNeeded {
            content: "hello".into(),
        };
        assert_eq!(o.content(), "hello");
        assert_eq!(o.original_length(), 5);
        assert!(!o.was_summarized());
    }

    #[test]
    fn outcome_truncated_was_summarized() {
        let o = SummaryOutcome::Truncated {
            original_length: 1000,
            preview: "head".into(),
            tail: "tail".into(),
        };
        assert!(o.was_summarized());
        assert_eq!(o.original_length(), 1000);
    }

    #[test]
    fn outcome_summarized_content() {
        let o = SummaryOutcome::Summarized {
            original_length: 500,
            summary: "brief".into(),
        };
        assert_eq!(o.content(), "brief");
        assert!(o.was_summarized());
    }

    // -- ToolResultSummarizer basic -----------------------------------------

    #[test]
    fn default_config_values() {
        let s = ToolResultSummarizer::default();
        assert!(s.is_enabled());
        assert_eq!(s.max_result_length(), 10_000);
        assert_eq!(s.max_preview_length(), 2_000);
        assert_eq!(s.max_tail_length(), 1_000);
    }

    #[test]
    fn needs_summary_below_threshold() {
        let s = ToolResultSummarizer::new(100, 50, 20);
        assert!(!s.needs_summary("short"));
    }

    #[test]
    fn needs_summary_above_threshold() {
        let s = ToolResultSummarizer::new(10, 5, 3);
        let long = "a".repeat(20);
        assert!(s.needs_summary(&long));
    }

    #[test]
    fn needs_summary_disabled() {
        let mut s = ToolResultSummarizer::new(10, 5, 3);
        s.disable();
        assert!(!s.is_enabled());
        let long = "a".repeat(20);
        assert!(!s.needs_summary(&long));
    }

    #[test]
    fn summarize_short_content_not_needed() {
        let s = ToolResultSummarizer::new(100, 50, 20);
        let result = s.summarize("hello");
        assert!(!result.was_summarized());
    }

    #[test]
    fn summarize_long_content_truncated() {
        let s = ToolResultSummarizer::new(10, 5, 3);
        let long = "abcdefghijKLMNOP";
        let result = s.summarize(long);
        assert!(result.was_summarized());
        if let SummaryOutcome::Truncated {
            original_length,
            preview,
            tail,
        } = result
        {
            assert_eq!(original_length, long.len());
            assert!(preview.chars().count() <= 5);
            assert!(tail.chars().count() <= 3);
        } else {
            panic!("expected Truncated");
        }
    }

    #[test]
    fn truncate_forced_even_when_disabled() {
        let mut s = ToolResultSummarizer::new(10, 5, 3);
        s.disable();
        let long = "a".repeat(50);
        let result = s.truncate(&long);
        assert!(result.was_summarized());
    }

    #[test]
    fn display_impl() {
        let s = ToolResultSummarizer::new(100, 50, 20);
        let display = format!("{s}");
        assert!(display.contains("max=100B"));
        assert!(display.contains("enabled=true"));
    }

    // -- StructuredSummary --------------------------------------------------

    #[test]
    fn structured_summary_counts_errors() {
        let content = "line1\nerror: something\nline3\nwarning: hmm\nerror: again";
        let s = ToolResultSummarizer::default();
        let summary = s.structured_summary(content);
        assert_eq!(summary.total_lines, 5);
        assert_eq!(summary.error_count, 2);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.error_snippets.len(), 2);
    }

    #[test]
    fn structured_summary_display() {
        let summary = StructuredSummary {
            total_lines: 10,
            total_bytes: 200,
            error_count: 1,
            warning_count: 2,
            error_snippets: vec!["L3: error here".into()],
            preview: "line1".into(),
        };
        let display = format!("{summary}");
        assert!(display.contains("10 lines"));
        assert!(display.contains("1 errors"));
    }

    // -- Utility functions --------------------------------------------------

    #[test]
    fn truncate_to_bytes_short() {
        assert_eq!(truncate_to_bytes("hi", 10), "hi");
    }

    #[test]
    fn truncate_to_bytes_exact() {
        assert_eq!(truncate_to_bytes("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_bytes_truncates() {
        let result = truncate_to_bytes("hello world", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn count_lines_test() {
        assert_eq!(count_lines("a\nb\nc"), 3);
        assert_eq!(count_lines(""), 0); // empty string has 0 lines
        assert_eq!(count_lines("single"), 1);
    }

    #[test]
    fn head_lines_test() {
        let content = "line1\nline2\nline3\nline4";
        assert_eq!(head_lines(content, 2), "line1\nline2");
    }

    #[test]
    fn tail_lines_test() {
        let content = "line1\nline2\nline3\nline4";
        assert_eq!(tail_lines(content, 2), "line3\nline4");
    }

    #[test]
    fn set_marker() {
        let mut s = ToolResultSummarizer::new(10, 5, 3);
        s.set_marker("!!CUT!!");
        // Marker is stored internally; we verify it doesn't panic
        let long = "a".repeat(50);
        let _ = s.summarize(&long);
    }
}
