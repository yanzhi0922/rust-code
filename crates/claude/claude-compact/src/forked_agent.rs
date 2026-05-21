//! Forked agent compact support.
//!
//! When a conversation is forked (e.g., for a sub-agent or parallel branch),
//! the full conversation history is often unnecessary. This module provides
//! utilities to compact messages for forked contexts by replacing verbose
//! tool results with lightweight placeholders.
//!
//! # Overview
//!
//! A *fork compact* replaces tool result content with a short placeholder
//! string, preserving the conversation structure while dramatically reducing
//! token count. This is especially useful when spawning sub-agents that only
//! need the gist of prior tool interactions.

use claude_core::Message;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for forked-agent compaction.
#[derive(Debug, Clone)]
pub struct ForkedAgentCompactConfig {
    /// Maximum number of tool results to keep intact before replacing with
    /// placeholders. Tool results beyond this limit (from oldest to newest)
    /// are replaced.
    pub max_tool_results: usize,
    /// Placeholder text substituted in place of the original tool output.
    pub placeholder_text: String,
}

impl Default for ForkedAgentCompactConfig {
    fn default() -> Self {
        Self {
            max_tool_results: 3,
            placeholder_text: "[Tool result omitted for fork]".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Check whether a message slice warrants fork compaction.
///
/// Returns `true` when the number of tool-result-bearing messages exceeds
/// [`ForkedAgentCompactConfig::max_tool_results`].
#[must_use]
pub fn should_compact_for_fork(messages: &[Message]) -> bool {
    count_tool_results(messages) > default_config().max_tool_results
}

/// Compact a message list for a forked agent context.
///
/// Tool results beyond the most recent `config.max_tool_results` are replaced
/// with `config.placeholder_text`. All other message types are preserved
/// unchanged.
///
/// # Returns
///
/// A new `Vec<Message>` with old tool results replaced by placeholders.
pub fn compact_for_fork(messages: &[Message], config: &ForkedAgentCompactConfig) -> Vec<Message> {
    let tool_result_count = count_tool_results(messages);

    if tool_result_count <= config.max_tool_results {
        return messages.to_vec();
    }

    // Identify which tool-result messages to keep (the most recent N).
    let keep_threshold = tool_result_count - config.max_tool_results;
    let mut tool_result_index = 0usize;

    messages
        .iter()
        .map(|msg| {
            if is_tool_result_message(msg) {
                tool_result_index += 1;
                if tool_result_index <= keep_threshold {
                    replace_tool_result_content(msg, &config.placeholder_text)
                } else {
                    msg.clone()
                }
            } else {
                msg.clone()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return the default config so we don't need `unwrap` in `should_compact_for_fork`.
fn default_config() -> ForkedAgentCompactConfig {
    ForkedAgentCompactConfig::default()
}

/// Count messages that carry tool-result content.
fn count_tool_results(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| is_tool_result_message(m))
        .count()
}

/// Check whether a message represents a tool result.
fn is_tool_result_message(msg: &Message) -> bool {
    matches!(msg, Message::ToolUseSummary(_))
}

/// Replace the content of a tool-result message with a placeholder.
fn replace_tool_result_content(msg: &Message, placeholder: &str) -> Message {
    match msg {
        Message::ToolUseSummary(summary) => {
            Message::ToolUseSummary(claude_core::ToolUseSummaryMessage {
                summary: placeholder.to_owned(),
                ..summary.clone()
            })
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{MessageBase, MessageOrigin, ToolUseSummaryMessage};
    use uuid::Uuid;

    /// Helper: create a tool-use-summary message.
    fn tool_summary(id: &str, content: &str) -> Message {
        Message::ToolUseSummary(ToolUseSummaryMessage {
            base: MessageBase {
                uuid: Uuid::new_v4(),
                parent_uuid: None,
                timestamp: chrono::Utc::now(),
                is_meta: false,
                is_virtual: false,
                is_compact_summary: false,
                is_visible_in_transcript_only: false,
                origin: Some(MessageOrigin::Tool),
            },
            tool_call_id: id.to_owned(),
            tool_name: "bash".to_owned(),
            summary: content.to_owned(),
            is_error: false,
            content_blocks: Vec::new(),
        })
    }

    /// Helper: create a user message.
    fn user_msg(text: &str) -> Message {
        Message::User(claude_core::UserMessage {
            base: MessageBase::with_origin(MessageOrigin::UserInput),
            text: text.to_owned(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    #[test]
    fn should_compact_returns_false_for_few_tool_results() {
        let messages = vec![user_msg("hello"), tool_summary("1", "output")];
        assert!(!should_compact_for_fork(&messages));
    }

    #[test]
    fn should_compact_returns_true_for_many_tool_results() {
        let messages: Vec<Message> = (0..10)
            .map(|i| tool_summary(&format!("tc-{i}"), "long output content here"))
            .collect();
        assert!(should_compact_for_fork(&messages));
    }

    #[test]
    fn compact_for_fork_preserves_recent_tool_results() {
        let config = ForkedAgentCompactConfig {
            max_tool_results: 2,
            placeholder_text: "[omitted]".to_owned(),
        };
        let messages = vec![
            tool_summary("1", "old output A"),
            tool_summary("2", "old output B"),
            tool_summary("3", "recent output C"),
            tool_summary("4", "recent output D"),
        ];

        let result = compact_for_fork(&messages, &config);
        assert_eq!(result.len(), 4);

        // First two should be replaced.
        match &result[0] {
            Message::ToolUseSummary(s) => assert_eq!(s.summary, "[omitted]"),
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
        match &result[1] {
            Message::ToolUseSummary(s) => assert_eq!(s.summary, "[omitted]"),
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
        // Last two should be preserved.
        match &result[2] {
            Message::ToolUseSummary(s) => assert_eq!(s.summary, "recent output C"),
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
        match &result[3] {
            Message::ToolUseSummary(s) => assert_eq!(s.summary, "recent output D"),
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
    }

    #[test]
    fn compact_for_fork_preserves_non_tool_messages() {
        let config = ForkedAgentCompactConfig {
            max_tool_results: 1,
            placeholder_text: "[placeholder]".to_owned(),
        };
        let messages = vec![
            user_msg("do something"),
            tool_summary("1", "output A"),
            user_msg("do more"),
            tool_summary("2", "output B"),
        ];

        let result = compact_for_fork(&messages, &config);
        assert_eq!(result.len(), 4);

        // User messages should be unchanged.
        match &result[0] {
            Message::User(u) => assert_eq!(u.text, "do something"),
            other => panic!("expected User, got {other:?}"),
        }
        match &result[2] {
            Message::User(u) => assert_eq!(u.text, "do more"),
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn compact_for_fork_noop_when_under_limit() {
        let config = ForkedAgentCompactConfig {
            max_tool_results: 5,
            placeholder_text: "[placeholder]".to_owned(),
        };
        let messages = vec![tool_summary("1", "output")];
        let result = compact_for_fork(&messages, &config);
        assert_eq!(result.len(), 1);

        match &result[0] {
            Message::ToolUseSummary(s) => assert_eq!(s.summary, "output"),
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
    }

    #[test]
    fn default_config_values() {
        let config = ForkedAgentCompactConfig::default();
        assert_eq!(config.max_tool_results, 3);
        assert!(!config.placeholder_text.is_empty());
    }
}
