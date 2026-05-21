//! Micro Compact (Cache Editing).
//!
//! Reduces token usage by clearing old tool result content that is unlikely
//! to be needed again.  Mirrors `services/compact/microCompact.ts`.
//!
//! # Algorithm
//!
//! 1. Scan all assistant messages for `ToolUse` blocks whose tool name is in
//!    [`COMPACTABLE_TOOLS`].  Collect their IDs in encounter order.
//! 2. Keep the **N** most recent IDs (where N = `keep_recent`).
//! 3. Scan `ToolUseSummary` messages: if `tool_call_id` is in the "to-clear"
//!    set, replace the summary text with [`TIME_BASED_MC_CLEARED_MESSAGE`].
//! 4. Also clear oversized `User` messages that look like tool output.

use std::collections::HashSet;

use claude_core::{
    AssistantContentBlock, Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype,
};

use crate::estimate_message_tokens;
use crate::prompt::rough_token_count;
use crate::strategy::{
    CompactOptions, CompactProgressEvent, CompactStrategy, CompactStrategyType, CompactionResult,
    ProgressCallback, SummaryProvider,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Message used to replace cleared tool result content.
pub const TIME_BASED_MC_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

/// Maximum token size for image blocks.
#[allow(dead_code)]
const IMAGE_MAX_TOKEN_SIZE: u64 = 2000;

/// Tool names whose results are eligible for micro-compaction.
const COMPACTABLE_TOOLS: &[&str] = &[
    "Read",
    "Bash",
    "PowerShell",
    "Grep",
    "Glob",
    "WebSearch",
    "WebFetch",
    "Edit",
    "Write",
];

// ---------------------------------------------------------------------------
// Micro compact config
// ---------------------------------------------------------------------------

/// Configuration for time-based micro-compaction.
#[derive(Debug, Clone)]
pub struct MicroCompactConfig {
    /// Minimum age in seconds before a tool result can be cleared.
    pub min_age_seconds: u64,
    /// Minimum tool result token size to be eligible for clearing.
    pub min_result_tokens: u64,
    /// Maximum number of tool results to clear in one pass.
    pub max_clears_per_pass: usize,
    /// Number of most-recent compactable tool results to preserve.
    pub keep_recent: usize,
}

impl Default for MicroCompactConfig {
    fn default() -> Self {
        Self {
            min_age_seconds: 300, // 5 minutes
            min_result_tokens: 500,
            max_clears_per_pass: 50,
            keep_recent: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Micro compact strategy
// ---------------------------------------------------------------------------

/// Micro-compact strategy that clears old tool results to save tokens.
#[derive(Default)]
pub struct MicroCompactStrategy {
    /// Configuration for this strategy.
    pub config: MicroCompactConfig,
}

impl MicroCompactStrategy {
    /// Create a new micro-compact strategy with custom config.
    pub fn new(config: MicroCompactConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl CompactStrategy for MicroCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::Micro
    }

    async fn compact(
        &self,
        messages: &[Message],
        _options: &CompactOptions,
        _provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        micro_compact(messages, &self.config, progress)
    }
}

// ---------------------------------------------------------------------------
// Core micro-compact implementation
// ---------------------------------------------------------------------------

/// Perform micro-compaction by clearing old tool results.
///
/// This does **not** call the LLM — it directly modifies messages by
/// replacing old tool result content with a placeholder.
///
/// # Algorithm
///
/// 1. Collect compactable tool-use IDs from assistant messages.
/// 2. Determine which IDs to keep (the `keep_recent` most recent).
/// 3. Clear `ToolUseSummary` messages whose `tool_call_id` is in the
///    to-clear set.
/// 4. Also clear oversized `User` messages that look like tool output.
pub fn micro_compact(
    messages: &[Message],
    config: &MicroCompactConfig,
    progress: Option<&ProgressCallback>,
) -> Result<CompactionResult, anyhow::Error> {
    if let Some(sink) = progress {
        sink(CompactProgressEvent::Started {
            strategy: CompactStrategyType::Micro,
        });
    }

    let pre_compact_tokens = estimate_message_tokens(messages);

    // Phase 1: Collect compactable tool-use IDs from assistant messages.
    let compactable_ids = collect_compactable_tool_ids(messages);

    // Phase 2: Determine which IDs to keep (the N most recent).
    let keep_count = config.keep_recent.max(1);
    let keep_set: HashSet<String> = compactable_ids
        .iter()
        .rev()
        .take(keep_count)
        .cloned()
        .collect();
    let clear_set: HashSet<&str> = compactable_ids
        .iter()
        .filter(|id| !keep_set.contains(id.as_str()))
        .map(String::as_str)
        .collect();

    // Phase 3: Clear old ToolUseSummary messages.
    let mut cleared_count: usize = 0;
    let mut tokens_saved: u64 = 0;
    let mut cleared_so_far: usize = 0;
    let mut modified_messages: Vec<Message> = messages.to_vec();

    for msg in &mut modified_messages {
        if cleared_so_far >= config.max_clears_per_pass {
            break;
        }

        if let Message::ToolUseSummary(tool_summary) = msg
            && clear_set.contains(tool_summary.tool_call_id.as_str())
        {
            let original_tokens = rough_token_count(&tool_summary.summary);
            if original_tokens >= config.min_result_tokens {
                let new_summary = TIME_BASED_MC_CLEARED_MESSAGE.to_string();
                let saved = original_tokens.saturating_sub(rough_token_count(&new_summary));
                if saved > 0 {
                    tool_summary.summary = new_summary;
                    tokens_saved += saved;
                    cleared_count += 1;
                    cleared_so_far += 1;
                }
            }
        }
    }

    // Phase 4: Also clear oversized User messages that look like tool output.
    for msg in &mut modified_messages {
        if cleared_so_far >= config.max_clears_per_pass {
            break;
        }

        if let Message::User(user_msg) = msg {
            let original_tokens = rough_token_count(&user_msg.text);
            if original_tokens >= config.min_result_tokens {
                let should_clear = user_msg.text.contains("tool_result")
                    || original_tokens > config.min_result_tokens * 2;

                if should_clear && !user_msg.text.is_empty() {
                    let new_text = TIME_BASED_MC_CLEARED_MESSAGE.to_string();
                    let saved = original_tokens.saturating_sub(rough_token_count(&new_text));
                    if saved > 0 {
                        user_msg.text = new_text;
                        tokens_saved += saved;
                        cleared_count += 1;
                        cleared_so_far += 1;
                    }
                }
            }
        }
    }

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Summarizing {
            messages_processed: cleared_count,
        });
    }

    let post_compact_tokens = pre_compact_tokens.saturating_sub(tokens_saved);

    let result = CompactionResult {
        summary: format!(
            "Micro-compaction: cleared {cleared_count} old tool results, saved ~{tokens_saved} tokens"
        ),
        messages_removed: cleared_count,
        tokens_saved,
        strategy_used: CompactStrategyType::Micro,
        preserved_segments: Vec::new(),
        pre_compact_token_count: Some(pre_compact_tokens),
        post_compact_token_count: Some(post_compact_tokens),
        messages_to_keep: modified_messages,
        attachments: vec![Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::Compact),
            subtype: SystemMessageSubtype::MicrocompactBoundary,
            text: format!("micro_compact: cleared={cleared_count}, tokens_saved={tokens_saved}"),
            error: None,
        })],
        hook_results: Vec::new(),
        user_display_message: None,
    };

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Completed(result.clone()));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect compactable tool-use IDs from assistant messages, in encounter order.
///
/// Scans all assistant messages for `ToolUse` blocks whose tool name is in
/// [`COMPACTABLE_TOOLS`] and returns their IDs.
fn collect_compactable_tool_ids(messages: &[Message]) -> Vec<String> {
    let mut ids = Vec::new();
    for msg in messages {
        if let Message::Assistant(assistant) = msg {
            for block in &assistant.blocks {
                if let AssistantContentBlock::ToolUse { id, name, .. } = block
                    && is_compactable_tool(name)
                {
                    ids.push(id.clone());
                }
            }
        }
    }
    ids
}

/// Check if a tool name is eligible for micro-compaction.
fn is_compactable_tool(name: &str) -> bool {
    COMPACTABLE_TOOLS.contains(&name)
}

/// Estimate tokens for a slice of messages (public API).
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    estimate_message_tokens(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::message::UserMessage;
    use claude_core::{
        AssistantContentBlock, AssistantMessage, MessageBase, ToolUseSummaryMessage,
    };

    // -- Helper constructors --

    fn make_assistant_with_tool_use(id: &str, name: &str) -> Message {
        Message::Assistant(AssistantMessage {
            base: MessageBase::with_origin(MessageOrigin::Provider),
            text: String::new(),
            blocks: vec![AssistantContentBlock::ToolUse {
                id: id.to_owned(),
                name: name.to_owned(),
                input: serde_json::Value::Null,
            }],
            tool_calls: vec![],
            provider_content_blocks: vec![],
        })
    }

    fn make_tool_summary(id: &str, name: &str, summary: &str) -> Message {
        Message::ToolUseSummary(ToolUseSummaryMessage {
            base: MessageBase::with_origin(MessageOrigin::Tool),
            tool_call_id: id.to_owned(),
            tool_name: name.to_owned(),
            summary: summary.to_owned(),
            is_error: false,
            content_blocks: Vec::new(),
        })
    }

    fn make_user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            base: MessageBase::default(),
            text: text.to_owned(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    // -- Tests --

    #[test]
    fn is_compactable_tool_known() {
        assert!(is_compactable_tool("Read"));
        assert!(is_compactable_tool("Bash"));
        assert!(is_compactable_tool("Grep"));
        assert!(is_compactable_tool("PowerShell"));
    }

    #[test]
    fn is_compactable_tool_unknown() {
        assert!(!is_compactable_tool("UnknownTool"));
        assert!(!is_compactable_tool("CustomMCP"));
    }

    #[test]
    fn micro_compact_empty_messages() {
        let config = MicroCompactConfig::default();
        let result = micro_compact(&[], &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 0);
        assert_eq!(result.tokens_saved, 0);
        assert_eq!(result.strategy_used, CompactStrategyType::Micro);
    }

    #[test]
    fn micro_compact_preserves_short_results() {
        let messages = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: "short result".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })];
        let config = MicroCompactConfig {
            min_result_tokens: 5000,
            ..MicroCompactConfig::default()
        };
        let result = micro_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 0);
    }

    #[test]
    fn micro_compact_clears_old_tool_summaries() {
        // Two tool uses: both compactable
        let messages = vec![
            make_assistant_with_tool_use("tu-1", "Read"),
            make_tool_summary("tu-1", "Read", &"x".repeat(4000)),
            make_assistant_with_tool_use("tu-2", "Bash"),
            make_tool_summary("tu-2", "Bash", &"y".repeat(4000)),
        ];

        let config = MicroCompactConfig {
            keep_recent: 1, // keep only the last one
            min_result_tokens: 100,
            ..MicroCompactConfig::default()
        };

        let result = micro_compact(&messages, &config, None).expect("should succeed");
        assert!(
            result.messages_removed >= 1,
            "should clear at least one old tool summary"
        );
        assert!(result.tokens_saved > 0);

        // Verify the kept messages reflect the clearing
        let kept = &result.messages_to_keep;
        let cleared_count = kept
            .iter()
            .filter(|m| {
                if let Message::ToolUseSummary(ts) = m {
                    ts.summary == TIME_BASED_MC_CLEARED_MESSAGE
                } else {
                    false
                }
            })
            .count();
        assert!(
            cleared_count >= 1,
            "at least one ToolUseSummary should be cleared"
        );
    }

    #[test]
    fn micro_compact_preserves_recent_tool_summaries() {
        let messages = vec![
            make_assistant_with_tool_use("tu-1", "Read"),
            make_tool_summary("tu-1", "Read", &"x".repeat(4000)),
            make_assistant_with_tool_use("tu-2", "Bash"),
            make_tool_summary("tu-2", "Bash", "recent result"),
        ];

        let config = MicroCompactConfig {
            keep_recent: 1, // keep only the last one
            min_result_tokens: 100,
            ..MicroCompactConfig::default()
        };

        let result = micro_compact(&messages, &config, None).expect("should succeed");

        // The most recent tool summary ("tu-2") should NOT be cleared
        let kept = &result.messages_to_keep;
        let tu2 = kept.iter().find_map(|m| {
            if let Message::ToolUseSummary(ts) = m
                && ts.tool_call_id == "tu-2"
            {
                return Some(ts.summary.clone());
            }
            None
        });
        assert_eq!(tu2.as_deref(), Some("recent result"));
    }

    #[test]
    fn micro_compact_respects_max_clears_per_pass() {
        let mut messages = Vec::new();
        for i in 0..10 {
            messages.push(make_assistant_with_tool_use(&format!("tu-{i}"), "Read"));
            messages.push(make_tool_summary(
                &format!("tu-{i}"),
                "Read",
                &"x".repeat(4000),
            ));
        }

        let config = MicroCompactConfig {
            keep_recent: 1,
            min_result_tokens: 100,
            max_clears_per_pass: 3,
            ..MicroCompactConfig::default()
        };

        let result = micro_compact(&messages, &config, None).expect("should succeed");
        assert!(
            result.messages_removed <= 3,
            "should not clear more than max_clears_per_pass"
        );
    }

    #[test]
    fn micro_compact_skips_non_compactable_tools() {
        let messages = vec![
            make_assistant_with_tool_use("tu-1", "CustomTool"),
            make_tool_summary("tu-1", "CustomTool", &"x".repeat(4000)),
        ];

        let config = MicroCompactConfig {
            keep_recent: 0,
            min_result_tokens: 100,
            ..MicroCompactConfig::default()
        };

        let result = micro_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(
            result.messages_removed, 0,
            "non-compactable tools should not be cleared"
        );
    }

    #[test]
    fn micro_compact_clears_large_user_messages() {
        let messages = vec![make_user_msg(&"tool_result: ".repeat(1000))];
        let config = MicroCompactConfig {
            min_result_tokens: 100,
            ..MicroCompactConfig::default()
        };
        let result = micro_compact(&messages, &config, None).expect("should succeed");
        assert!(result.messages_removed >= 1);
        let kept = &result.messages_to_keep;
        if let Some(Message::User(u)) = kept.first() {
            assert_eq!(u.text, TIME_BASED_MC_CLEARED_MESSAGE);
        }
    }

    #[test]
    fn collect_compactable_tool_ids_ordering() {
        let messages = vec![
            make_assistant_with_tool_use("tu-1", "Read"),
            make_assistant_with_tool_use("tu-2", "Bash"),
            make_assistant_with_tool_use("tu-3", "CustomTool"), // not compactable
            make_assistant_with_tool_use("tu-4", "Grep"),
        ];
        let ids = collect_compactable_tool_ids(&messages);
        assert_eq!(ids, vec!["tu-1", "tu-2", "tu-4"]);
    }

    #[test]
    fn micro_compact_config_default_values() {
        let config = MicroCompactConfig::default();
        assert_eq!(config.min_age_seconds, 300);
        assert_eq!(config.min_result_tokens, 500);
        assert_eq!(config.max_clears_per_pass, 50);
        assert_eq!(config.keep_recent, 3);
    }
}
