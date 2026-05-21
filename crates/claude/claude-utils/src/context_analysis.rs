//! Context window usage analysis.
//!
//! Provides analysis of context window utilization for conversation
//! messages, helping determine when compaction is needed.

use claude_core::ConversationEntry;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Analysis of context window usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextAnalysis {
    /// Total estimated tokens used.
    pub total_tokens: usize,
    /// Estimated tokens used by system prompts.
    pub system_tokens: usize,
    /// Estimated tokens used by conversation messages.
    pub conversation_tokens: usize,
    /// Estimated tokens used by tool call results.
    pub tool_tokens: usize,
    /// Remaining tokens before hitting the limit.
    pub remaining_tokens: usize,
}

impl ContextAnalysis {
    /// Calculate the utilization percentage (0.0 to 100.0).
    pub fn utilization_percent(&self) -> f64 {
        if self.total_tokens == 0 {
            return 0.0;
        }
        let max = self.total_tokens + self.remaining_tokens;
        if max == 0 {
            return 0.0;
        }
        (self.total_tokens as f64 / max as f64) * 100.0
    }

    /// Check if the context is critically low on remaining tokens.
    pub fn is_critical(&self) -> bool {
        self.remaining_tokens < 1000
    }

    /// Check if the context usage is high (above 80%).
    pub fn is_high(&self) -> bool {
        self.utilization_percent() > 80.0
    }
}

// ---------------------------------------------------------------------------
// Analysis functions
// ---------------------------------------------------------------------------

/// Analyze context usage for a set of conversation entries.
///
/// Estimates token counts based on rough heuristics (~4 characters per token).
pub fn analyze_context_usage(messages: &[ConversationEntry], max_tokens: usize) -> ContextAnalysis {
    let mut system_tokens = 0usize;
    let mut conversation_tokens = 0usize;
    let mut tool_tokens = 0usize;

    for entry in messages {
        let text_tokens = estimate_tokens(&entry.text);

        // Count tool call results
        let entry_tool_tokens: usize = entry
            .tool_calls
            .iter()
            .map(|tc| estimate_tokens(&tc.name) + estimate_tokens(&tc.input.to_string()))
            .sum();

        match entry.role {
            claude_core::ConversationRole::System => {
                system_tokens += text_tokens;
            }
            claude_core::ConversationRole::Tool => {
                tool_tokens += text_tokens + entry_tool_tokens;
            }
            _ => {
                conversation_tokens += text_tokens + entry_tool_tokens;
            }
        }
    }

    let total_tokens = system_tokens + conversation_tokens + tool_tokens;
    let remaining_tokens = max_tokens.saturating_sub(total_tokens);

    ContextAnalysis {
        total_tokens,
        system_tokens,
        conversation_tokens,
        tool_tokens,
        remaining_tokens,
    }
}

/// Suggest whether context compaction should be performed.
///
/// Returns `Some(reason)` if compaction is recommended, `None` otherwise.
pub fn suggest_compaction(analysis: &ContextAnalysis) -> Option<String> {
    if analysis.remaining_tokens == 0 {
        return Some("Context window is full. Compaction is required to continue.".to_string());
    }

    if analysis.is_critical() {
        return Some(format!(
            "Context window is critically low ({} tokens remaining). Compaction recommended.",
            analysis.remaining_tokens
        ));
    }

    if analysis.is_high() {
        return Some(format!(
            "Context usage is at {:.1}%. Consider compacting to free up space.",
            analysis.utilization_percent()
        ));
    }

    None
}

/// Estimate the number of tokens in a text string.
///
/// Uses a rough heuristic of ~4 characters per token, which is a reasonable
/// approximation for English text with typical code.
fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Rough heuristic: 1 token ≈ 4 characters
    text.len().div_ceil(4)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{ConversationRole, ToolCall};
    use serde_json::Value;
    use uuid::Uuid;

    fn make_entry(role: ConversationRole, text: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: Uuid::new_v4(),
            role,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls: vec![],
            attachments: vec![],
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    fn make_entry_with_tool_calls(
        role: ConversationRole,
        text: &str,
        tool_names: &[&str],
    ) -> ConversationEntry {
        let tool_calls: Vec<ToolCall> = tool_names
            .iter()
            .map(|name| ToolCall {
                id: format!("tc-{}", Uuid::new_v4()),
                name: name.to_string(),
                input: Value::Object(serde_json::Map::new()),
            })
            .collect();

        ConversationEntry {
            uuid: Uuid::new_v4(),
            role,
            text: text.to_string(),
            history_text: None,
            content_blocks: vec![],
            tool_calls,
            attachments: vec![],
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    #[test]
    fn empty_messages_zero_tokens() {
        let analysis = analyze_context_usage(&[], 100000);
        assert_eq!(analysis.total_tokens, 0);
        assert_eq!(analysis.remaining_tokens, 100000);
    }

    #[test]
    fn system_tokens_counted() {
        let messages = vec![make_entry(ConversationRole::System, "You are helpful.")];
        let analysis = analyze_context_usage(&messages, 100000);
        assert!(analysis.system_tokens > 0);
        assert_eq!(analysis.conversation_tokens, 0);
    }

    #[test]
    fn user_tokens_counted() {
        let messages = vec![make_entry(ConversationRole::User, "Hello there!")];
        let analysis = analyze_context_usage(&messages, 100000);
        assert!(analysis.conversation_tokens > 0);
        assert_eq!(analysis.system_tokens, 0);
    }

    #[test]
    fn assistant_tokens_counted() {
        let messages = vec![make_entry(
            ConversationRole::Assistant,
            "Hi! How can I help?",
        )];
        let analysis = analyze_context_usage(&messages, 100000);
        assert!(analysis.conversation_tokens > 0);
    }

    #[test]
    fn tool_tokens_counted() {
        let messages = vec![make_entry(ConversationRole::Tool, "result output")];
        let analysis = analyze_context_usage(&messages, 100000);
        assert!(analysis.tool_tokens > 0);
        assert_eq!(analysis.conversation_tokens, 0);
    }

    #[test]
    fn remaining_tokens_calculated() {
        let messages = vec![make_entry(ConversationRole::User, "short")];
        let analysis = analyze_context_usage(&messages, 100);
        assert!(analysis.remaining_tokens < 100);
        assert!(analysis.remaining_tokens > 0);
    }

    #[test]
    fn utilization_percent_empty() {
        let analysis = analyze_context_usage(&[], 100000);
        assert_eq!(analysis.utilization_percent(), 0.0);
    }

    #[test]
    fn utilization_percent_nonzero() {
        let messages = vec![make_entry(
            ConversationRole::User,
            &"a".repeat(40000), // ~10000 tokens
        )];
        let analysis = analyze_context_usage(&messages, 100000);
        assert!(analysis.utilization_percent() > 0.0);
        assert!(analysis.utilization_percent() < 100.0);
    }

    #[test]
    fn is_critical() {
        let analysis = ContextAnalysis {
            total_tokens: 99900,
            system_tokens: 100,
            conversation_tokens: 99700,
            tool_tokens: 100,
            remaining_tokens: 500,
        };
        assert!(analysis.is_critical());
    }

    #[test]
    fn is_not_critical() {
        let analysis = ContextAnalysis {
            total_tokens: 50000,
            system_tokens: 100,
            conversation_tokens: 49800,
            tool_tokens: 100,
            remaining_tokens: 50000,
        };
        assert!(!analysis.is_critical());
    }

    #[test]
    fn is_high_usage() {
        let analysis = ContextAnalysis {
            total_tokens: 85000,
            system_tokens: 100,
            conversation_tokens: 84800,
            tool_tokens: 100,
            remaining_tokens: 15000,
        };
        assert!(analysis.is_high());
    }

    #[test]
    fn suggest_compaction_full() {
        let analysis = ContextAnalysis {
            total_tokens: 100000,
            system_tokens: 100,
            conversation_tokens: 99800,
            tool_tokens: 100,
            remaining_tokens: 0,
        };
        let suggestion = suggest_compaction(&analysis);
        assert!(suggestion.is_some());
        assert!(
            suggestion
                .expect("full compaction should have a suggestion")
                .contains("full")
        );
    }

    #[test]
    fn suggest_compaction_critical() {
        let analysis = ContextAnalysis {
            total_tokens: 99500,
            system_tokens: 100,
            conversation_tokens: 99300,
            tool_tokens: 100,
            remaining_tokens: 500,
        };
        let suggestion = suggest_compaction(&analysis);
        assert!(suggestion.is_some());
        assert!(
            suggestion
                .expect("critical compaction should have a suggestion")
                .contains("critically low")
        );
    }

    #[test]
    fn suggest_compaction_high() {
        let analysis = ContextAnalysis {
            total_tokens: 85000,
            system_tokens: 100,
            conversation_tokens: 84800,
            tool_tokens: 100,
            remaining_tokens: 15000,
        };
        let suggestion = suggest_compaction(&analysis);
        assert!(suggestion.is_some());
    }

    #[test]
    fn suggest_compaction_low_usage() {
        let analysis = ContextAnalysis {
            total_tokens: 10000,
            system_tokens: 100,
            conversation_tokens: 9800,
            tool_tokens: 100,
            remaining_tokens: 90000,
        };
        let suggestion = suggest_compaction(&analysis);
        assert!(suggestion.is_none());
    }

    #[test]
    fn tool_calls_contribute_tokens() {
        let entry = make_entry_with_tool_calls(
            ConversationRole::Assistant,
            "Running tools",
            &["read_file", "write_file"],
        );
        let messages = vec![entry];
        let analysis = analyze_context_usage(&messages, 100000);
        // Should have conversation tokens from both text and tool call names
        assert!(analysis.conversation_tokens > 0);
    }

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_nonempty() {
        let tokens = estimate_tokens("Hello, world!"); // 13 chars
        assert!(tokens > 0);
        assert!(tokens <= 4); // 13/4 = 3.25, rounded up to 4
    }
}
