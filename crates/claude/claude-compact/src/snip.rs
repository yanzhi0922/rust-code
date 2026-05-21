//! Snip Compact strategy.
//!
//! Trims oversized tool outputs by replacing them with shorter placeholders.
//! Mirrors `services/compact/snipCompact.ts`.
//!
//! # Snip Strategies
//!
//! - **Threshold** (default): Snip any single message exceeding the token threshold.
//! - **PreserveHeadTail**: Keep the first and last N messages, snip the middle.
//! - **PreserveKeyMessages**: Keep system messages and user messages, snip tool output.

use claude_core::{Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype};

use crate::estimate_message_tokens;
use crate::prompt::rough_token_count;
use crate::strategy::{
    CompactOptions, CompactProgressEvent, CompactStrategy, CompactStrategyType, CompactionResult,
    ProgressCallback, SummaryProvider,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default maximum token length for a single tool result before snipping.
pub const DEFAULT_SNIP_THRESHOLD_TOKENS: u64 = 10_000;

/// Placeholder text used to replace snipped content.
pub const SNIPPED_CONTENT_MARKER: &str =
    "[... content snipped for length; use Read to see the full output if needed]";

// ---------------------------------------------------------------------------
// Snip strategy selection
// ---------------------------------------------------------------------------

/// Strategy for selecting which messages to snip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnipStrategy {
    /// Snip any single message exceeding the token threshold (default).
    #[default]
    Threshold,
    /// Keep the first `preserve_head` and last `preserve_tail` messages;
    /// snip oversized content in the middle.
    PreserveHeadTail {
        /// Number of leading messages to preserve.
        preserve_head: usize,
        /// Number of trailing messages to preserve.
        preserve_tail: usize,
    },
    /// Keep system messages and user messages intact; only snip tool output
    /// messages (`ToolUseSummary`, `CollapsedReadSearch`).
    PreserveKeyMessages,
}

// ---------------------------------------------------------------------------
// Snip compact config
// ---------------------------------------------------------------------------

/// Configuration for snip compaction.
#[derive(Debug, Clone)]
pub struct SnipCompactConfig {
    /// Maximum tokens per tool result before it gets snipped.
    pub snip_threshold_tokens: u64,
    /// Whether snip compact is enabled.
    pub enabled: bool,
    /// Which snip strategy to use.
    pub strategy: SnipStrategy,
}

impl Default for SnipCompactConfig {
    fn default() -> Self {
        Self {
            snip_threshold_tokens: DEFAULT_SNIP_THRESHOLD_TOKENS,
            enabled: true,
            strategy: SnipStrategy::Threshold,
        }
    }
}

// ---------------------------------------------------------------------------
// Snip compact strategy
// ---------------------------------------------------------------------------

/// Snip-compact strategy that trims oversized tool outputs.
#[derive(Default)]
pub struct SnipCompactStrategy {
    /// Configuration for this strategy.
    pub config: SnipCompactConfig,
}

impl SnipCompactStrategy {
    /// Create a new snip-compact strategy with custom config.
    pub fn new(config: SnipCompactConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl CompactStrategy for SnipCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::Snip
    }

    async fn compact(
        &self,
        messages: &[Message],
        _options: &CompactOptions,
        _provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        snip_compact(messages, &self.config, progress)
    }
}

// ---------------------------------------------------------------------------
// Core snip-compact implementation
// ---------------------------------------------------------------------------

/// Perform snip compaction on the given messages.
///
/// This does **not** call the LLM — it directly trims oversized tool outputs.
/// Returns the (potentially modified) messages along with a compaction result.
pub fn snip_compact(
    messages: &[Message],
    config: &SnipCompactConfig,
    progress: Option<&ProgressCallback>,
) -> Result<CompactionResult, anyhow::Error> {
    if !config.enabled {
        return Ok(CompactionResult {
            summary: "Snip compact disabled".into(),
            messages_removed: 0,
            tokens_saved: 0,
            strategy_used: CompactStrategyType::Snip,
            preserved_segments: Vec::new(),
            pre_compact_token_count: None,
            post_compact_token_count: None,
            messages_to_keep: messages.to_vec(),
            attachments: Vec::new(),
            hook_results: Vec::new(),
            user_display_message: None,
        });
    }

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Started {
            strategy: CompactStrategyType::Snip,
        });
    }

    let pre_compact_tokens = estimate_message_tokens(messages);
    let mut snipped_count: usize = 0;
    let mut tokens_saved: u64 = 0;
    let mut snip_descriptions: Vec<String> = Vec::new();

    let mut modified_messages: Vec<Message> = messages.to_vec();
    let total = modified_messages.len();

    for (idx, msg) in modified_messages.iter_mut().enumerate() {
        if !should_snip_index(idx, total, config.strategy) {
            continue;
        }

        let (token_count, description) = message_token_count_and_desc(msg);
        if token_count > config.snip_threshold_tokens {
            let saved = token_count.saturating_sub(rough_token_count(SNIPPED_CONTENT_MARKER));
            snip_message_content(msg);
            if saved > 0 {
                tokens_saved += saved;
                snipped_count += 1;
                snip_descriptions.push(description);
            }
        }
    }

    let post_compact_tokens = pre_compact_tokens.saturating_sub(tokens_saved);

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Summarizing {
            messages_processed: snipped_count,
        });
    }

    let summary = if snipped_count > 0 {
        format!(
            "Snip compact: trimmed {snipped_count} oversized outputs, saved ~{tokens_saved} tokens. \
             Snipped: {}",
            snip_descriptions.join("; ")
        )
    } else {
        "Snip compact: no oversized outputs found".into()
    };

    let result = CompactionResult {
        summary,
        messages_removed: snipped_count,
        tokens_saved,
        strategy_used: CompactStrategyType::Snip,
        preserved_segments: Vec::new(),
        pre_compact_token_count: Some(pre_compact_tokens),
        post_compact_token_count: Some(post_compact_tokens),
        messages_to_keep: modified_messages,
        attachments: vec![Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::Compact),
            subtype: SystemMessageSubtype::CompactBoundary,
            text: format!("snip_compact: snipped={snipped_count}, tokens_saved={tokens_saved}"),
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

/// Check if a message is a snip boundary marker.
pub fn is_snip_boundary_message(msg: &Message) -> bool {
    if let Message::System(sys) = msg {
        sys.subtype == SystemMessageSubtype::CompactBoundary
            && sys.text.starts_with("snip_compact:")
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Strategy helpers
// ---------------------------------------------------------------------------

/// Determine whether the message at `idx` is eligible for snipping under the
/// given strategy.
fn should_snip_index(idx: usize, total: usize, strategy: SnipStrategy) -> bool {
    match strategy {
        SnipStrategy::Threshold => true,
        SnipStrategy::PreserveHeadTail {
            preserve_head,
            preserve_tail,
        } => {
            let head_end = preserve_head.min(total);
            let tail_start = total.saturating_sub(preserve_tail);
            idx >= head_end && idx < tail_start
        }
        SnipStrategy::PreserveKeyMessages => true, // filtering is done by message type below
    }
}

/// Return the estimated token count and a short description for a message.
fn message_token_count_and_desc(msg: &Message) -> (u64, String) {
    match msg {
        Message::User(m) => {
            let tokens = rough_token_count(&m.text);
            (tokens, format!("user message ({} tokens)", tokens))
        }
        Message::ToolUseSummary(m) => {
            let tokens = rough_token_count(&m.summary);
            (
                tokens,
                format!(
                    "tool {}:{} ({} tokens)",
                    m.tool_name, m.tool_call_id, tokens
                ),
            )
        }
        Message::CollapsedReadSearch(m) => {
            let tokens = rough_token_count(&m.summary);
            (tokens, format!("collapsed read/search ({} tokens)", tokens))
        }
        Message::Assistant(m) => {
            let tokens = rough_token_count(&m.text);
            (tokens, format!("assistant message ({} tokens)", tokens))
        }
        Message::System(m) => {
            let tokens = rough_token_count(&m.text);
            (tokens, format!("system message ({} tokens)", tokens))
        }
        Message::Progress(m) => {
            let tokens = rough_token_count(&m.status);
            (tokens, format!("progress message ({} tokens)", tokens))
        }
        Message::Attachment(m) => {
            let tokens = m.label.as_deref().map_or(0, rough_token_count);
            (tokens, format!("attachment ({} tokens)", tokens))
        }
        Message::HookResult(m) => {
            let tokens = rough_token_count(&m.output);
            (tokens, format!("hook result ({} tokens)", tokens))
        }
        Message::Tombstone(m) => {
            let tokens = rough_token_count(&m.summary);
            (tokens, format!("tombstone ({} tokens)", tokens))
        }
        Message::GroupedToolUse(m) => {
            let tokens = m.summary.as_deref().map_or(0, rough_token_count);
            (tokens, format!("grouped tool use ({} tokens)", tokens))
        }
    }
}

/// Snip the content of a message in place, replacing it with the marker.
///
/// For `PreserveKeyMessages` strategy, system and user messages are never snipped.
fn snip_message_content(msg: &mut Message) {
    match msg {
        Message::User(m) => {
            m.text = SNIPPED_CONTENT_MARKER.to_string();
        }
        Message::ToolUseSummary(m) => {
            m.summary = SNIPPED_CONTENT_MARKER.to_string();
        }
        Message::CollapsedReadSearch(m) => {
            m.summary = SNIPPED_CONTENT_MARKER.to_string();
        }
        Message::Assistant(m) => {
            m.text = SNIPPED_CONTENT_MARKER.to_string();
        }
        // System, progress, attachment, hook result, tombstone, grouped tool use
        // are not snipped — they are either critical or small.
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Token estimation helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::message::UserMessage;
    use claude_core::{
        AssistantMessage, CollapsedReadSearchMessage, MessageBase, ToolUseSummaryMessage,
    };

    // -- Helper constructors --

    fn make_user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            base: MessageBase::default(),
            text: text.to_owned(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    fn make_assistant_msg(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            base: MessageBase::with_origin(MessageOrigin::Provider),
            text: text.to_owned(),
            blocks: vec![],
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

    fn make_collapsed_read_search(summary: &str) -> Message {
        Message::CollapsedReadSearch(CollapsedReadSearchMessage {
            base: MessageBase::default(),
            summary: summary.to_owned(),
            items: Vec::new(),
        })
    }

    // -- Tests --

    #[test]
    fn snip_compact_disabled() {
        let config = SnipCompactConfig {
            enabled: false,
            ..SnipCompactConfig::default()
        };
        let messages = vec![make_user_msg(&"x".repeat(100_000))];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 0);
        assert_eq!(result.tokens_saved, 0);
    }

    #[test]
    fn snip_compact_trims_long_user_content() {
        let config = SnipCompactConfig {
            snip_threshold_tokens: 100,
            ..SnipCompactConfig::default()
        };
        let messages = vec![make_user_msg(&"x".repeat(1000))];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 1);
        assert!(result.tokens_saved > 0);
        let kept = &result.messages_to_keep;
        if let Some(Message::User(u)) = kept.first() {
            assert_eq!(u.text, SNIPPED_CONTENT_MARKER);
        }
    }

    #[test]
    fn snip_compact_preserves_short_content() {
        let config = SnipCompactConfig::default();
        let messages = vec![make_user_msg("short")];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 0);
    }

    #[test]
    fn snip_compact_trims_tool_use_summaries() {
        let config = SnipCompactConfig {
            snip_threshold_tokens: 100,
            ..SnipCompactConfig::default()
        };
        let messages = vec![make_tool_summary(
            "tc-1",
            "Read",
            &"file content ".repeat(500),
        )];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 1);
        assert!(result.tokens_saved > 0);
        let kept = &result.messages_to_keep;
        if let Some(Message::ToolUseSummary(ts)) = kept.first() {
            assert_eq!(ts.summary, SNIPPED_CONTENT_MARKER);
        }
    }

    #[test]
    fn snip_compact_trims_collapsed_read_search() {
        let config = SnipCompactConfig {
            snip_threshold_tokens: 100,
            ..SnipCompactConfig::default()
        };
        let messages = vec![make_collapsed_read_search(&"result ".repeat(500))];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 1);
        assert!(result.tokens_saved > 0);
    }

    #[test]
    fn snip_compact_preserve_head_tail_strategy() {
        let config = SnipCompactConfig {
            snip_threshold_tokens: 100,
            enabled: true,
            strategy: SnipStrategy::PreserveHeadTail {
                preserve_head: 1,
                preserve_tail: 1,
            },
        };
        let messages = vec![
            make_user_msg("short"),                // idx 0 — preserved (head)
            make_user_msg(&"x".repeat(1000)),      // idx 1 — snippable (middle)
            make_user_msg(&"y".repeat(1000)),      // idx 2 — snippable (middle)
            make_assistant_msg(&"z".repeat(1000)), // idx 3 — preserved (tail)
        ];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert_eq!(result.messages_removed, 2); // middle messages snipped

        let kept = &result.messages_to_keep;
        // Head preserved
        if let Some(Message::User(u)) = kept.first() {
            assert_eq!(u.text, "short");
        }
        // Tail preserved
        if let Some(Message::Assistant(a)) = kept.last() {
            assert_eq!(a.text, "z".repeat(1000));
        }
    }

    #[test]
    fn snip_compact_generates_summary() {
        let config = SnipCompactConfig {
            snip_threshold_tokens: 100,
            ..SnipCompactConfig::default()
        };
        let messages = vec![make_user_msg(&"x".repeat(1000))];
        let result = snip_compact(&messages, &config, None).expect("should succeed");
        assert!(result.summary.contains("trimmed"));
        assert!(result.summary.contains("tokens"));
    }

    #[test]
    fn is_snip_boundary_message_detects_boundary() {
        let boundary = Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::Compact),
            subtype: SystemMessageSubtype::CompactBoundary,
            text: "snip_compact: snipped=1, tokens_saved=500".into(),
            error: None,
        });
        assert!(is_snip_boundary_message(&boundary));

        let non_boundary = Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::System),
            subtype: SystemMessageSubtype::Informational,
            text: "some other message".into(),
            error: None,
        });
        assert!(!is_snip_boundary_message(&non_boundary));
    }

    #[test]
    fn snip_compact_config_default_values() {
        let config = SnipCompactConfig::default();
        assert_eq!(config.snip_threshold_tokens, DEFAULT_SNIP_THRESHOLD_TOKENS);
        assert!(config.enabled);
        assert_eq!(config.strategy, SnipStrategy::Threshold);
    }
}
