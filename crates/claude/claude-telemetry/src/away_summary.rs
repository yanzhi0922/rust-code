//! Away Summary — generates session recaps for the "while you were away" card.
//!
//! When a user steps away and returns, this module produces a short 1–3 sentence
//! recap of what happened during the session. It tracks idle periods, manages
//! the summary window, and provides configuration for the recap behavior.
//!
//! # Architecture
//!
//! - [`AwaySummaryConfig`] — configuration for recap generation
//! - [`AwaySummaryEntry`] — a single summary snapshot
//! - [`AwaySummaryTracker`] — tracks idle periods and generates summaries

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default number of recent messages to include in the summary window.
const DEFAULT_RECENT_MESSAGE_WINDOW: usize = 30;

/// Default idle threshold before triggering a summary (5 minutes).
const DEFAULT_IDLE_THRESHOLD_SECS: u64 = 300;

/// Default maximum summary length in characters.
const DEFAULT_MAX_SUMMARY_LENGTH: usize = 500;

/// Configuration for the away summary feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaySummaryConfig {
    /// Number of recent messages to include in the recap window.
    pub recent_message_window: usize,
    /// Idle duration (seconds) before considering the user "away".
    pub idle_threshold_secs: u64,
    /// Maximum character length for generated summaries.
    pub max_summary_length: usize,
    /// Whether the away summary feature is enabled.
    pub enabled: bool,
}

impl Default for AwaySummaryConfig {
    fn default() -> Self {
        Self {
            recent_message_window: DEFAULT_RECENT_MESSAGE_WINDOW,
            idle_threshold_secs: DEFAULT_IDLE_THRESHOLD_SECS,
            max_summary_length: DEFAULT_MAX_SUMMARY_LENGTH,
            enabled: true,
        }
    }
}

impl AwaySummaryConfig {
    /// Creates a new config with custom idle threshold.
    #[must_use]
    pub fn with_idle_threshold(secs: u64) -> Self {
        Self {
            idle_threshold_secs: secs,
            ..Self::default()
        }
    }

    /// Creates a disabled config.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Summary Entry
// ---------------------------------------------------------------------------

/// A single away summary snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaySummaryEntry {
    /// The summary text (1–3 sentences).
    pub summary: String,
    /// Timestamp when the summary was generated (epoch millis).
    pub generated_at: u64,
    /// Number of messages in the session at the time of generation.
    pub message_count: usize,
    /// Duration the user was away (seconds).
    pub away_duration_secs: u64,
    /// Number of tool calls that occurred while away.
    pub tool_calls_while_away: usize,
    /// Whether this summary was generated from an empty transcript.
    pub from_empty_transcript: bool,
}

impl AwaySummaryEntry {
    /// Creates a new summary entry.
    #[must_use]
    pub fn new(summary: String, message_count: usize, away_duration_secs: u64) -> Self {
        let generated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);

        Self {
            summary,
            generated_at,
            message_count,
            away_duration_secs,
            tool_calls_while_away: 0,
            from_empty_transcript: false,
        }
    }

    /// Creates an empty/null summary entry for cases where generation is skipped.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            summary: String::new(),
            generated_at: 0,
            message_count: 0,
            away_duration_secs: 0,
            tool_calls_while_away: 0,
            from_empty_transcript: true,
        }
    }

    /// Returns `true` if this is a non-empty summary.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.summary.is_empty() && !self.from_empty_transcript
    }

    /// Truncates the summary to the configured maximum length.
    pub fn truncate_to(&mut self, max_len: usize) {
        if self.summary.len() > max_len {
            // Find the last word boundary before max_len
            let truncated = &self.summary[..max_len.saturating_sub(3)];
            if let Some(last_space) = truncated.rfind(' ') {
                self.summary = format!("{}...", &self.summary[..last_space]);
            } else {
                self.summary = format!("{truncated}...");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Message Context (simplified)
// ---------------------------------------------------------------------------

/// A simplified representation of a conversation message for summary generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryMessage {
    /// Message role (user, assistant, system).
    pub role: String,
    /// Message text content.
    pub content: String,
    /// Whether this message contains tool calls.
    pub has_tool_calls: bool,
    /// Timestamp (epoch millis).
    pub timestamp: u64,
}

impl SummaryMessage {
    /// Creates a new summary message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            has_tool_calls: false,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
        }
    }

    /// Creates a summary message with tool call flag.
    #[must_use]
    pub fn with_tool_calls(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            has_tool_calls: true,
            ..Self::new(role, content)
        }
    }
}

// ---------------------------------------------------------------------------
// Summary Prompt Builder
// ---------------------------------------------------------------------------

/// Builds the prompt for the away summary generation.
///
/// The prompt instructs the model to write 1–3 short sentences:
/// 1. The high-level task
/// 2. The concrete next step
/// 3. (Optional) Any blockers or important context
#[must_use]
pub fn build_away_summary_prompt(session_memory: Option<&str>) -> String {
    let memory_block = session_memory.map_or_else(String::new, |m| {
        format!("Session memory (broader context):\n{m}\n\n")
    });

    format!(
        "{memory_block}\
         The user stepped away and is coming back. Write exactly 1-3 short sentences. \
         Start by stating the high-level task — what they are building or debugging, \
         not implementation details. Next: the concrete next step. \
         Skip status reports and commit recaps."
    )
}

// ---------------------------------------------------------------------------
// Tracker
// ---------------------------------------------------------------------------

/// Tracks idle periods and manages away summary generation.
pub struct AwaySummaryTracker {
    /// Configuration.
    config: AwaySummaryConfig,
    /// Last activity timestamp.
    last_activity: Option<Instant>,
    /// Generated summaries.
    summaries: Vec<AwaySummaryEntry>,
    /// Number of tool calls since last activity.
    pending_tool_calls: usize,
    /// Total messages seen.
    total_messages: usize,
}

impl AwaySummaryTracker {
    /// Creates a new tracker with the given configuration.
    #[must_use]
    pub fn new(config: AwaySummaryConfig) -> Self {
        Self {
            config,
            last_activity: None,
            summaries: Vec::new(),
            pending_tool_calls: 0,
            total_messages: 0,
        }
    }

    /// Creates a tracker with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(AwaySummaryConfig::default())
    }

    /// Records activity (resets the idle timer).
    pub fn record_activity(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    /// Records a tool call.
    pub fn record_tool_call(&mut self) {
        self.pending_tool_calls += 1;
    }

    /// Records messages and updates the count.
    pub fn record_messages(&mut self, count: usize) {
        self.total_messages = count;
    }

    /// Returns whether the user is considered "away" based on idle threshold.
    #[must_use]
    pub fn is_away(&self) -> bool {
        self.last_activity.is_none_or(|last| {
            last.elapsed() >= Duration::from_secs(self.config.idle_threshold_secs)
        })
    }

    /// Returns the idle duration since last activity.
    #[must_use]
    pub fn idle_duration(&self) -> Duration {
        self.last_activity
            .map_or(Duration::MAX, |last| last.elapsed())
    }

    /// Generates an away summary from the given messages.
    /// Returns `None` if the transcript is empty or the feature is disabled.
    pub fn generate_summary(
        &mut self,
        messages: &[SummaryMessage],
        session_memory: Option<&str>,
    ) -> Option<AwaySummaryEntry> {
        if !self.config.enabled {
            return None;
        }

        if messages.is_empty() {
            return None;
        }

        // Take the recent message window
        let start = messages
            .len()
            .saturating_sub(self.config.recent_message_window);
        let recent = &messages[start..];

        // Count tool calls in recent messages
        let tool_calls: usize = recent.iter().filter(|m| m.has_tool_calls).count();

        // Build a simple heuristic summary from recent messages
        let summary_text = build_heuristic_summary(recent, session_memory);

        let away_secs = self.idle_duration().as_secs();

        let mut entry = AwaySummaryEntry {
            summary: summary_text,
            generated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            message_count: messages.len(),
            away_duration_secs: away_secs,
            tool_calls_while_away: tool_calls + self.pending_tool_calls,
            from_empty_transcript: false,
        };

        entry.truncate_to(self.config.max_summary_length);

        self.summaries.push(entry.clone());
        self.pending_tool_calls = 0;
        self.record_activity();

        Some(entry)
    }

    /// Returns all generated summaries.
    #[must_use]
    pub fn summaries(&self) -> &[AwaySummaryEntry] {
        &self.summaries
    }

    /// Returns the most recent summary, if any.
    #[must_use]
    pub fn last_summary(&self) -> Option<&AwaySummaryEntry> {
        self.summaries.last()
    }

    /// Returns the number of summaries generated.
    #[must_use]
    pub fn summary_count(&self) -> usize {
        self.summaries.len()
    }

    /// Resets the tracker state.
    pub fn reset(&mut self) {
        self.last_activity = None;
        self.summaries.clear();
        self.pending_tool_calls = 0;
        self.total_messages = 0;
    }

    /// Returns a reference to the config.
    #[must_use]
    pub fn config(&self) -> &AwaySummaryConfig {
        &self.config
    }
}

/// Builds a heuristic summary from recent messages.
/// In production, this would call an LLM; here we extract key information.
fn build_heuristic_summary(recent: &[SummaryMessage], session_memory: Option<&str>) -> String {
    let mut user_messages: Vec<&str> = Vec::new();
    let mut assistant_actions: Vec<&str> = Vec::new();

    for msg in recent {
        match msg.role.as_str() {
            "user" => {
                let content = msg.content.trim();
                if !content.is_empty() && content.len() < 200 {
                    user_messages.push(content);
                }
            }
            "assistant" => {
                let content = msg.content.trim();
                if !content.is_empty() && content.len() < 200 {
                    assistant_actions.push(content);
                }
            }
            _ => {}
        }
    }

    let mut parts = Vec::new();

    // Include session memory context if available
    if let Some(memory) = session_memory
        && !memory.is_empty()
    {
        // Extract first line as high-level context
        if let Some(first_line) = memory.lines().next() {
            let trimmed = first_line.trim();
            if !trimmed.is_empty() && trimmed.len() < 100 {
                parts.push(format!("Context: {trimmed}"));
            }
        }
    }

    // Add last user intent
    if let Some(last_user) = user_messages.last() {
        parts.push(format!("Last task: {last_user}"));
    }

    // Add last assistant action
    if let Some(last_action) = assistant_actions.last() {
        parts.push(format!("Progress: {last_action}"));
    }

    if parts.is_empty() {
        "Session in progress.".to_string()
    } else {
        parts.join(". ")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- AwaySummaryConfig tests ---

    #[test]
    fn test_config_default() {
        let config = AwaySummaryConfig::default();
        assert_eq!(config.recent_message_window, 30);
        assert_eq!(config.idle_threshold_secs, 300);
        assert_eq!(config.max_summary_length, 500);
        assert!(config.enabled);
    }

    #[test]
    fn test_config_with_idle_threshold() {
        let config = AwaySummaryConfig::with_idle_threshold(600);
        assert_eq!(config.idle_threshold_secs, 600);
        assert!(config.enabled);
    }

    #[test]
    fn test_config_disabled() {
        let config = AwaySummaryConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = AwaySummaryConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: AwaySummaryConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.recent_message_window, 30);
    }

    // --- AwaySummaryEntry tests ---

    #[test]
    fn test_entry_new() {
        let entry = AwaySummaryEntry::new("Test summary".to_string(), 10, 300);
        assert_eq!(entry.summary, "Test summary");
        assert_eq!(entry.message_count, 10);
        assert_eq!(entry.away_duration_secs, 300);
        assert!(entry.is_valid());
    }

    #[test]
    fn test_entry_empty() {
        let entry = AwaySummaryEntry::empty();
        assert!(entry.summary.is_empty());
        assert!(!entry.is_valid());
        assert!(entry.from_empty_transcript);
    }

    #[test]
    fn test_entry_truncate_short_text() {
        let mut entry = AwaySummaryEntry::new("Short".to_string(), 5, 100);
        entry.truncate_to(100);
        assert_eq!(entry.summary, "Short");
    }

    #[test]
    fn test_entry_truncate_long_text() {
        let long_text = "This is a very long summary text ".repeat(20);
        let mut entry = AwaySummaryEntry::new(long_text, 5, 100);
        entry.truncate_to(50);
        assert!(entry.summary.len() <= 53); // 50 + "..." max
        assert!(entry.summary.ends_with("..."));
    }

    // --- SummaryMessage tests ---

    #[test]
    fn test_summary_message_new() {
        let msg = SummaryMessage::new("user", "Hello world");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello world");
        assert!(!msg.has_tool_calls);
    }

    #[test]
    fn test_summary_message_with_tool_calls() {
        let msg = SummaryMessage::with_tool_calls("assistant", "Running tests");
        assert!(msg.has_tool_calls);
    }

    // --- build_away_summary_prompt tests ---

    #[test]
    fn test_build_prompt_without_memory() {
        let prompt = build_away_summary_prompt(None);
        assert!(prompt.contains("stepped away"));
        assert!(!prompt.contains("Session memory"));
    }

    #[test]
    fn test_build_prompt_with_memory() {
        let prompt = build_away_summary_prompt(Some("Building a Rust CLI"));
        assert!(prompt.contains("Session memory"));
        assert!(prompt.contains("Building a Rust CLI"));
    }

    // --- AwaySummaryTracker tests ---

    #[test]
    fn test_tracker_new() {
        let tracker = AwaySummaryTracker::with_defaults();
        assert!(tracker.summaries().is_empty());
        assert_eq!(tracker.summary_count(), 0);
    }

    #[test]
    fn test_tracker_record_activity() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        tracker.record_activity();
        assert!(!tracker.is_away());
    }

    #[test]
    fn test_tracker_is_away_without_activity() {
        let tracker = AwaySummaryTracker::with_defaults();
        assert!(tracker.is_away());
    }

    #[test]
    fn test_tracker_generate_summary_empty_messages() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        let result = tracker.generate_summary(&[], None);
        assert!(result.is_none());
    }

    #[test]
    fn test_tracker_generate_summary_disabled() {
        let mut tracker = AwaySummaryTracker::new(AwaySummaryConfig::disabled());
        let messages = vec![SummaryMessage::new("user", "Hello")];
        let result = tracker.generate_summary(&messages, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_tracker_generate_summary_with_messages() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        let messages = vec![
            SummaryMessage::new("user", "Fix the login bug"),
            SummaryMessage::new("assistant", "I found the issue in auth.rs"),
            SummaryMessage::new("user", "Great, apply the fix"),
        ];
        let result = tracker.generate_summary(&messages, None);
        assert!(result.is_some());
        let entry = result.expect("entry");
        assert!(!entry.summary.is_empty());
        assert_eq!(entry.message_count, 3);
        assert_eq!(tracker.summary_count(), 1);
    }

    #[test]
    fn test_tracker_generate_summary_with_memory() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        let messages = vec![
            SummaryMessage::new("user", "Add tests"),
            SummaryMessage::new("assistant", "Created test module"),
        ];
        let result = tracker.generate_summary(&messages, Some("Working on auth module"));
        assert!(result.is_some());
        assert!(result.expect("entry").summary.contains("auth"));
    }

    #[test]
    fn test_tracker_tool_call_tracking() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        tracker.record_tool_call();
        tracker.record_tool_call();
        tracker.record_tool_call();

        let messages = vec![
            SummaryMessage::with_tool_calls("assistant", "Running tests"),
            SummaryMessage::new("user", "Check results"),
        ];
        let result = tracker.generate_summary(&messages, None);
        assert!(result.is_some());
        // 1 tool call in messages + 3 pending = 4
        assert_eq!(result.expect("entry").tool_calls_while_away, 4);
    }

    #[test]
    fn test_tracker_reset() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        tracker.record_activity();
        tracker.record_tool_call();
        let messages = vec![SummaryMessage::new("user", "Test")];
        let _ = tracker.generate_summary(&messages, None);

        tracker.reset();
        assert!(tracker.summaries().is_empty());
        assert!(tracker.is_away());
        assert_eq!(tracker.summary_count(), 0);
    }

    #[test]
    fn test_tracker_last_summary() {
        let mut tracker = AwaySummaryTracker::with_defaults();
        assert!(tracker.last_summary().is_none());

        let messages = vec![SummaryMessage::new("user", "Test")];
        let _ = tracker.generate_summary(&messages, None);
        assert!(tracker.last_summary().is_some());
    }

    #[test]
    fn test_heuristic_summary_empty_messages() {
        let summary = build_heuristic_summary(&[], None);
        assert_eq!(summary, "Session in progress.");
    }

    #[test]
    fn test_heuristic_summary_with_messages() {
        let messages = vec![
            SummaryMessage::new("user", "Fix the bug"),
            SummaryMessage::new("assistant", "Fixed in main.rs"),
        ];
        let summary = build_heuristic_summary(&messages, None);
        assert!(summary.contains("Fix the bug"));
    }

    #[test]
    fn test_tracker_config_access() {
        let tracker = AwaySummaryTracker::with_defaults();
        assert_eq!(tracker.config().recent_message_window, 30);
    }
}
