//! Context Collapse engine for detecting and collapsing oversized contexts.
//!
//! When a conversation grows too large for the model's context window, this
//! module detects the condition and applies a series of *collapse operations*
//! that preserve critical information while removing redundancy.
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────────────────┐
//! │  ContextCollapseEngine    │
//! │  - should_collapse()      │
//! │  - execute_collapse()     │
//! └──────────┬────────────────┘
//!            │ uses
//! ┌──────────▼────────────────┐
//! │  CollapseOperation(s)     │
//! │  - RemoveOldToolResults   │
//! │  - SummarizeConversation  │
//! │  - TrimToolOutputs        │
//! │  - PreserveRecentMessages │
//! └──────────┬────────────────┘
//!            │ records
//! ┌──────────▼────────────────┐
//! │  CollapsePersistence      │
//! │  - save() / load()        │
//! │  - history tracking       │
//! └───────────────────────────┘
//! ```

use std::collections::VecDeque;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::estimate_messages_tokens;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the context collapse engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextCollapseConfig {
    /// Maximum context tokens before collapse is triggered.
    pub max_context_tokens: usize,
    /// Fraction (0.0–1.0) of max tokens at which collapse starts.
    pub collapse_threshold: Ratio64,
    /// Number of recent messages to always preserve.
    pub preserve_recent_messages: usize,
    /// Whether to preserve system messages during collapse.
    pub preserve_system_messages: bool,
    /// Maximum tokens for a single tool output before trimming.
    pub max_tool_output_tokens: usize,
}

/// A validated 0.0–1.0 ratio stored as a fixed-point `u64` (0 = 0%, 1000 = 100%).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ratio64(pub u64);

impl Ratio64 {
    /// Create a ratio from a float value, clamping to [0, 1].
    #[must_use]
    pub fn from_f64(value: f64) -> Self {
        Self((value.clamp(0.0, 1.0) * 1000.0) as u64)
    }

    /// Return the ratio as a float.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.0 as u32) / 1000.0
    }
}

impl Default for ContextCollapseConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 200_000,
            collapse_threshold: Ratio64::from_f64(0.85),
            preserve_recent_messages: 10,
            preserve_system_messages: true,
            max_tool_output_tokens: 5_000,
        }
    }
}

// ---------------------------------------------------------------------------
// CollapseOperation — individual collapse strategies
// ---------------------------------------------------------------------------

/// A single collapse operation that can be applied to a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollapseOperation {
    /// Remove old tool results beyond the recent window.
    RemoveOldToolResults,
    /// Replace old messages with a summary (requires LLM).
    SummarizeConversation,
    /// Trim oversized tool outputs to a maximum length.
    TrimToolOutputs,
    /// Preserve only the most recent N messages.
    PreserveRecentMessages,
    /// Remove duplicate or redundant system messages.
    DeduplicateSystemMessages,
    /// Remove messages marked as tombstones.
    RemoveTombstones,
}

impl CollapseOperation {
    /// Return a human-readable name for the operation.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::RemoveOldToolResults => "Remove old tool results",
            Self::SummarizeConversation => "Summarize conversation",
            Self::TrimToolOutputs => "Trim tool outputs",
            Self::PreserveRecentMessages => "Preserve recent messages",
            Self::DeduplicateSystemMessages => "Deduplicate system messages",
            Self::RemoveTombstones => "Remove tombstones",
        }
    }

    /// Return all available operations in recommended execution order.
    #[must_use]
    pub fn all_in_order() -> Vec<Self> {
        vec![
            Self::RemoveTombstones,
            Self::DeduplicateSystemMessages,
            Self::RemoveOldToolResults,
            Self::TrimToolOutputs,
            Self::PreserveRecentMessages,
            Self::SummarizeConversation,
        ]
    }
}

// ---------------------------------------------------------------------------
// CollapseResult — outcome of a collapse pass
// ---------------------------------------------------------------------------

/// Result of a context collapse operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollapseResult {
    /// Unique identifier for this collapse pass.
    pub id: String,
    /// When the collapse was performed.
    pub timestamp: DateTime<Utc>,
    /// Estimated token count before collapse.
    pub original_token_count: usize,
    /// Estimated token count after collapse.
    pub collapsed_token_count: usize,
    /// Operations that were applied.
    pub operations_applied: Vec<CollapseOperation>,
    /// Number of messages preserved.
    pub preserved_message_count: usize,
    /// Number of messages removed.
    pub removed_message_count: usize,
}

impl CollapseResult {
    /// Create a new collapse result.
    #[must_use]
    pub fn new(original_tokens: usize, collapsed_tokens: usize) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            original_token_count: original_tokens,
            collapsed_token_count: collapsed_tokens,
            operations_applied: Vec::new(),
            preserved_message_count: 0,
            removed_message_count: 0,
        }
    }

    /// Return the token reduction ratio (0.0–1.0).
    #[must_use]
    pub fn reduction_ratio(self) -> f64 {
        if self.original_token_count == 0 {
            return 0.0;
        }
        let saved = self
            .original_token_count
            .saturating_sub(self.collapsed_token_count);
        f64::from(saved as u32) / f64::from(self.original_token_count as u32)
    }

    /// Return the number of tokens saved.
    #[must_use]
    pub fn tokens_saved(self) -> usize {
        self.original_token_count
            .saturating_sub(self.collapsed_token_count)
    }
}

// ---------------------------------------------------------------------------
// CollapsePersistence — record collapse history
// ---------------------------------------------------------------------------

/// In-memory persistence for collapse results.
///
/// Maintains a bounded history of collapse operations for diagnostics
/// and audit purposes.
#[derive(Debug, Clone, Default)]
pub struct CollapsePersistence {
    history: VecDeque<CollapseResult>,
    max_history: usize,
}

impl CollapsePersistence {
    /// Create a new persistence layer with the given maximum history size.
    #[must_use]
    pub fn new(max_history: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(max_history.min(64)),
            max_history,
        }
    }

    /// Save a collapse result to the history.
    pub fn save(&mut self, result: CollapseResult) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(result);
    }

    /// Load the most recent collapse result.
    #[must_use]
    pub fn last(&self) -> Option<&CollapseResult> {
        self.history.back()
    }

    /// Return the full collapse history (oldest first).
    #[must_use]
    pub fn history(&self) -> &VecDeque<CollapseResult> {
        &self.history
    }

    /// Return the total number of collapse operations recorded.
    #[must_use]
    pub fn count(&self) -> usize {
        self.history.len()
    }

    /// Return the total tokens saved across all collapses.
    #[must_use]
    pub fn total_tokens_saved(&self) -> usize {
        self.history.iter().map(|r| r.clone().tokens_saved()).sum()
    }

    /// Clear all collapse history.
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Check if any collapse result matches a predicate.
    pub fn has_result(&self, predicate: impl Fn(&CollapseResult) -> bool) -> bool {
        self.history.iter().any(predicate)
    }
}

// ---------------------------------------------------------------------------
// ContextCollapseEngine — the main engine
// ---------------------------------------------------------------------------

/// Engine that detects oversized contexts and applies collapse operations.
///
/// The engine estimates the token count of the current conversation and
/// compares it against the configured thresholds. When the threshold is
/// exceeded, it applies a series of [`CollapseOperation`]s to reduce the
/// context size.
pub struct ContextCollapseEngine {
    config: ContextCollapseConfig,
    persistence: CollapsePersistence,
}

impl ContextCollapseEngine {
    /// Create a new engine with the given configuration.
    #[must_use]
    pub fn new(config: ContextCollapseConfig) -> Self {
        let max_history = 100;
        Self {
            config,
            persistence: CollapsePersistence::new(max_history),
        }
    }

    /// Create with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ContextCollapseConfig::default())
    }

    /// Return a reference to the engine configuration.
    #[must_use]
    pub fn config(&self) -> &ContextCollapseConfig {
        &self.config
    }

    /// Return a reference to the persistence layer.
    #[must_use]
    pub fn persistence(&self) -> &CollapsePersistence {
        &self.persistence
    }

    /// Return a mutable reference to the persistence layer.
    #[must_use]
    pub fn persistence_mut(&mut self) -> &mut CollapsePersistence {
        &mut self.persistence
    }

    /// Check whether the given messages should trigger a collapse.
    ///
    /// Returns `true` when the estimated token count exceeds the configured
    /// threshold relative to the maximum context tokens.
    pub fn should_collapse(&self, messages: &[claude_core::Message]) -> bool {
        let tokens = estimate_messages_tokens(messages);
        let threshold = (self.config.max_context_tokens as f64
            * self.config.collapse_threshold.as_f64()) as u64;
        tokens > threshold
    }

    /// Execute a collapse pass on the given messages.
    ///
    /// Applies collapse operations in recommended order and returns
    /// the reduced message list and a [`CollapseResult`] describing
    /// what was done.
    ///
    /// # Errors
    /// Returns an error if the collapse fails unexpectedly.
    pub fn execute_collapse(
        &mut self,
        messages: &[claude_core::Message],
    ) -> Result<(Vec<claude_core::Message>, CollapseResult)> {
        let original_tokens = estimate_messages_tokens(messages);
        let original_count = messages.len();

        let mut result =
            CollapseResult::new(usize::try_from(original_tokens).unwrap_or(usize::MAX), 0);
        let mut collapsed: Vec<claude_core::Message> = messages.to_vec();

        // Phase 1: Remove tombstones
        let before = collapsed.len();
        collapsed.retain(|m| !Self::is_tombstone(m));
        if collapsed.len() < before {
            result
                .operations_applied
                .push(CollapseOperation::RemoveTombstones);
        }

        // Phase 2: Deduplicate system messages
        let before = collapsed.len();
        collapsed = Self::deduplicate_system_messages(&collapsed);
        if collapsed.len() < before {
            result
                .operations_applied
                .push(CollapseOperation::DeduplicateSystemMessages);
        }

        // Phase 3: Remove old tool results (keep recent window)
        let before = collapsed.len();
        collapsed = Self::remove_old_tool_results(&collapsed, self.config.preserve_recent_messages);
        if collapsed.len() < before {
            result
                .operations_applied
                .push(CollapseOperation::RemoveOldToolResults);
        }

        // Phase 4: Trim oversized tool outputs
        let trimmed = Self::trim_tool_outputs(&mut collapsed, self.config.max_tool_output_tokens);
        if trimmed {
            result
                .operations_applied
                .push(CollapseOperation::TrimToolOutputs);
        }

        // Phase 5: Preserve only recent messages if still over budget
        if collapsed.len() > self.config.preserve_recent_messages {
            let total = collapsed.len();
            let start = total.saturating_sub(self.config.preserve_recent_messages);

            // Preserve system messages from the beginning (before the recent tail).
            // System messages inside the recent tail are already included below,
            // so we only collect those that would otherwise be dropped.
            let mut preserved: Vec<claude_core::Message> = Vec::new();
            if self.config.preserve_system_messages {
                for msg in collapsed.iter().take(start) {
                    if Self::is_system_message(msg) {
                        preserved.push(msg.clone());
                    }
                }
            }

            // Add the recent tail
            for msg in collapsed.iter().skip(start) {
                preserved.push(msg.clone());
            }

            if preserved.len() < collapsed.len() {
                collapsed = preserved;
                result
                    .operations_applied
                    .push(CollapseOperation::PreserveRecentMessages);
            }
        }

        let collapsed_tokens = estimate_messages_tokens(&collapsed);
        result.collapsed_token_count = usize::try_from(collapsed_tokens).unwrap_or(usize::MAX);
        result.preserved_message_count = collapsed.len();
        result.removed_message_count = original_count.saturating_sub(collapsed.len());

        self.persistence.save(result.clone());

        Ok((collapsed, result))
    }

    /// Check if a message is a tombstone.
    fn is_tombstone(msg: &claude_core::Message) -> bool {
        matches!(msg, claude_core::Message::Tombstone(_))
    }

    /// Check if a message is a system message.
    fn is_system_message(msg: &claude_core::Message) -> bool {
        matches!(msg, claude_core::Message::System(_))
    }

    /// Remove old tool results, preserving the recent window.
    fn remove_old_tool_results(
        messages: &[claude_core::Message],
        preserve_recent: usize,
    ) -> Vec<claude_core::Message> {
        if messages.len() <= preserve_recent {
            return messages.to_vec();
        }

        // Identify which indices are tool results
        let tool_result_indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m, claude_core::Message::ToolUseSummary(_)))
            .map(|(i, _)| i)
            .collect();

        // Keep tool results only in the recent window
        let recent_start = messages.len().saturating_sub(preserve_recent);
        let preserved_indices: Vec<usize> = tool_result_indices
            .iter()
            .filter(|&&i| i >= recent_start)
            .copied()
            .collect();

        let preserved_set: std::collections::HashSet<usize> =
            preserved_indices.into_iter().collect();

        messages
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                if matches!(m, claude_core::Message::ToolUseSummary(_)) {
                    preserved_set.contains(i)
                } else {
                    true
                }
            })
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// Deduplicate consecutive system messages with identical text.
    fn deduplicate_system_messages(messages: &[claude_core::Message]) -> Vec<claude_core::Message> {
        let mut result = Vec::with_capacity(messages.len());
        let mut last_system_text: Option<String> = None;

        for msg in messages {
            if let claude_core::Message::System(sys) = msg {
                if last_system_text.as_deref() == Some(sys.text.as_str()) {
                    continue; // skip duplicate
                }
                last_system_text = Some(sys.text.clone());
            } else {
                last_system_text = None;
            }
            result.push(msg.clone());
        }

        result
    }

    /// Trim oversized tool outputs in place. Returns true if any trimming occurred.
    fn trim_tool_outputs(messages: &mut [claude_core::Message], max_tokens: usize) -> bool {
        let mut trimmed = false;
        // We can only trim text content in tool use summaries
        for msg in messages.iter_mut() {
            if let claude_core::Message::ToolUseSummary(tool_summary) = msg {
                let estimated_tokens = tool_summary.summary.len() / 4;
                if estimated_tokens > max_tokens {
                    let max_chars = max_tokens * 4;
                    tool_summary.summary.truncate(max_chars);
                    tool_summary.summary.push_str("\n...[trimmed]");
                    trimmed = true;
                }
            }
        }
        trimmed
    }
}

// ---------------------------------------------------------------------------
// Standalone context_collapse function
// ---------------------------------------------------------------------------

/// Perform context collapse on the given messages.
///
/// This is a convenience wrapper around [`ContextCollapseEngine::execute_collapse`]
/// that creates a default engine, runs the collapse, and returns a
/// [`CompactionResult`] suitable for the compact pipeline.
///
/// This does **not** call the LLM — it applies deterministic collapse operations.
pub fn context_collapse(
    messages: &[claude_core::Message],
    config: &ContextCollapseConfig,
) -> Result<(Vec<claude_core::Message>, CollapseResult)> {
    let mut engine = ContextCollapseEngine::new(config.clone());
    engine.execute_collapse(messages)
}

// ---------------------------------------------------------------------------
// ContextCollapseStrategy — CompactStrategy adapter
// ---------------------------------------------------------------------------

/// Adapter that wraps [`ContextCollapseEngine`] as a [`CompactStrategy`].
///
/// This allows the context-collapse engine to be used through the same
/// [`CompactStrategy::compact()`] interface as all other strategies.
pub struct ContextCollapseStrategy {
    /// Configuration for the collapse engine.
    pub config: ContextCollapseConfig,
}

impl ContextCollapseStrategy {
    /// Create a new strategy with the given configuration.
    #[must_use]
    pub fn new(config: ContextCollapseConfig) -> Self {
        Self { config }
    }

    /// Create a strategy with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ContextCollapseConfig::default())
    }
}

impl Default for ContextCollapseStrategy {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[async_trait::async_trait]
impl crate::strategy::CompactStrategy for ContextCollapseStrategy {
    fn strategy_type(&self) -> crate::strategy::CompactStrategyType {
        crate::strategy::CompactStrategyType::Full
    }

    async fn compact(
        &self,
        messages: &[claude_core::Message],
        _options: &crate::strategy::CompactOptions,
        _provider: &dyn crate::strategy::SummaryProvider,
        progress: Option<&crate::strategy::ProgressCallback>,
    ) -> Result<crate::strategy::CompactionResult, anyhow::Error> {
        if let Some(sink) = progress {
            sink(crate::strategy::CompactProgressEvent::Started {
                strategy: crate::strategy::CompactStrategyType::Full,
            });
        }

        let original_tokens = estimate_messages_tokens(messages);
        let (collapsed, collapse_result) = context_collapse(messages, &self.config)?;

        let tokens_saved = original_tokens.saturating_sub(estimate_messages_tokens(&collapsed));
        let pre_tokens = usize::try_from(original_tokens).unwrap_or(usize::MAX);
        let post_tokens =
            usize::try_from(estimate_messages_tokens(&collapsed)).unwrap_or(usize::MAX);
        let ops_count = collapse_result.operations_applied.len();
        let removed_count = collapse_result.removed_message_count;
        let saved_by_collapse = collapse_result.tokens_saved();

        let result = crate::strategy::CompactionResult {
            summary: format!(
                "Context collapse: {ops_count} operations applied, {removed_count} messages removed, ~{saved_by_collapse} tokens saved"
            ),
            messages_removed: removed_count,
            tokens_saved,
            strategy_used: crate::strategy::CompactStrategyType::Full,
            preserved_segments: Vec::new(),
            pre_compact_token_count: Some(u64::try_from(pre_tokens).unwrap_or(u64::MAX)),
            post_compact_token_count: Some(u64::try_from(post_tokens).unwrap_or(u64::MAX)),
            messages_to_keep: collapsed,
            attachments: Vec::new(),
            hook_results: Vec::new(),
            user_display_message: None,
        };

        if let Some(sink) = progress {
            sink(crate::strategy::CompactProgressEvent::Completed(
                result.clone(),
            ));
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Detect collapsible message groups
// ---------------------------------------------------------------------------

/// A span of consecutive messages that can be collapsed into a single summary.
#[derive(Debug, Clone)]
pub struct CollapsibleSpan {
    /// Start index (inclusive) in the original message list.
    pub start: usize,
    /// End index (exclusive) in the original message list.
    pub end: usize,
    /// Estimated tokens in this span.
    pub estimated_tokens: u64,
    /// Description of the span's content.
    pub description: String,
}

/// Detect consecutive tool-call/result groups that can be collapsed.
///
/// Scans the message list for runs of `ToolUseSummary` or `CollapsedReadSearch`
/// messages and returns spans that exceed the given minimum token threshold.
pub fn detect_collapsible_spans(
    messages: &[claude_core::Message],
    min_span_tokens: u64,
) -> Vec<CollapsibleSpan> {
    let mut spans = Vec::new();
    let mut span_start: Option<usize> = None;
    let mut span_tokens: u64 = 0;

    for (i, msg) in messages.iter().enumerate() {
        let is_tool = matches!(
            msg,
            claude_core::Message::ToolUseSummary(_) | claude_core::Message::CollapsedReadSearch(_)
        );

        if is_tool {
            if span_start.is_none() {
                span_start = Some(i);
            }
            span_tokens += estimate_messages_tokens(std::slice::from_ref(msg));
        } else if let Some(start) = span_start.take() {
            if span_tokens >= min_span_tokens {
                spans.push(CollapsibleSpan {
                    start,
                    end: i,
                    estimated_tokens: span_tokens,
                    description: format!("tool results [{}..{})", start, i),
                });
            }
            span_tokens = 0;
        }
    }

    // Handle trailing span
    if let Some(start) = span_start.take()
        && span_tokens >= min_span_tokens
    {
        spans.push(CollapsibleSpan {
            start,
            end: messages.len(),
            estimated_tokens: span_tokens,
            description: format!("tool results [{}..{})", start, messages.len()),
        });
    }

    spans
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::message::MessageOrigin;
    use claude_core::{
        AssistantMessage, ConversationEntry, Message, MessageBase, SystemMessage,
        SystemMessageSubtype, ToolUseSummaryMessage,
    };

    fn make_user_msg(text: &str) -> Message {
        Message::from(ConversationEntry::user(text))
    }

    fn make_system_msg(text: &str) -> Message {
        Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::System),
            subtype: SystemMessageSubtype::Informational,
            text: text.to_owned(),
            error: None,
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

    fn make_tombstone(summary: &str) -> Message {
        Message::Tombstone(claude_core::TombstoneMessage {
            base: MessageBase::default(),
            replaced_message_ids: vec![],
            summary: summary.to_owned(),
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

    #[test]
    fn collapse_config_default_values() {
        let config = ContextCollapseConfig::default();
        assert_eq!(config.max_context_tokens, 200_000);
        assert_eq!(config.preserve_recent_messages, 10);
        assert!(config.preserve_system_messages);
        assert_eq!(config.max_tool_output_tokens, 5_000);
    }

    #[test]
    fn collapse_config_custom_values() {
        let config = ContextCollapseConfig {
            max_context_tokens: 100_000,
            collapse_threshold: Ratio64::from_f64(0.9),
            preserve_recent_messages: 5,
            preserve_system_messages: false,
            max_tool_output_tokens: 2_000,
        };
        assert_eq!(config.max_context_tokens, 100_000);
        assert_eq!(config.preserve_recent_messages, 5);
        assert!(!config.preserve_system_messages);
    }

    #[test]
    fn ratio64_clamps_and_converts() {
        let r = Ratio64::from_f64(0.85);
        assert!((r.as_f64() - 0.85).abs() < 0.01);

        let clamped_high = Ratio64::from_f64(2.0);
        assert!((clamped_high.as_f64() - 1.0).abs() < 0.01);

        let clamped_low = Ratio64::from_f64(-1.0);
        assert!((clamped_low.as_f64() - 0.0).abs() < 0.01);
    }

    #[test]
    fn collapse_operation_labels() {
        assert_eq!(
            CollapseOperation::RemoveOldToolResults.label(),
            "Remove old tool results"
        );
        assert_eq!(
            CollapseOperation::SummarizeConversation.label(),
            "Summarize conversation"
        );
    }

    #[test]
    fn collapse_operation_order() {
        let ops = CollapseOperation::all_in_order();
        assert_eq!(ops.len(), 6);
        assert_eq!(ops[0], CollapseOperation::RemoveTombstones);
        assert_eq!(ops[5], CollapseOperation::SummarizeConversation);
    }

    #[test]
    fn collapse_result_reduction_ratio() {
        let result = CollapseResult::new(1000, 400);
        let ratio = result.clone().reduction_ratio();
        assert!((ratio - 0.6).abs() < 0.01);
        assert_eq!(result.tokens_saved(), 600);
    }

    #[test]
    fn collapse_result_zero_original() {
        let result = CollapseResult::new(0, 0);
        assert_eq!(result.clone().reduction_ratio(), 0.0);
        assert_eq!(result.tokens_saved(), 0);
    }

    #[test]
    fn should_collapse_below_threshold() {
        let engine = ContextCollapseEngine::with_defaults();
        let messages = vec![make_user_msg("short message")];
        assert!(!engine.should_collapse(&messages));
    }

    #[test]
    fn should_collapse_with_low_threshold() {
        let config = ContextCollapseConfig {
            max_context_tokens: 10,
            collapse_threshold: Ratio64::from_f64(0.01),
            ..ContextCollapseConfig::default()
        };
        let engine = ContextCollapseEngine::new(config);
        let messages = vec![make_user_msg(
            "this is a message that should exceed the threshold",
        )];
        assert!(engine.should_collapse(&messages));
    }

    #[test]
    fn execute_collapse_removes_tombstones() {
        let mut engine = ContextCollapseEngine::with_defaults();
        let messages = vec![
            make_user_msg("hello"),
            make_tombstone("old message"),
            make_assistant_msg("world"),
        ];
        let (collapsed, result) = engine
            .execute_collapse(&messages)
            .expect("collapse should succeed");
        assert!(collapsed.len() < messages.len());
        assert!(
            result
                .operations_applied
                .contains(&CollapseOperation::RemoveTombstones)
        );
        assert!(
            collapsed
                .iter()
                .all(|m| !matches!(m, Message::Tombstone(_)))
        );
    }

    #[test]
    fn execute_collapse_preserves_recent() {
        let config = ContextCollapseConfig {
            preserve_recent_messages: 2,
            preserve_system_messages: false,
            ..ContextCollapseConfig::default()
        };
        let mut engine = ContextCollapseEngine::new(config);
        let messages = vec![
            make_user_msg("msg1"),
            make_user_msg("msg2"),
            make_user_msg("msg3"),
            make_user_msg("msg4"),
            make_user_msg("msg5"),
        ];
        let (collapsed, _result) = engine
            .execute_collapse(&messages)
            .expect("collapse should succeed");
        assert!(collapsed.len() <= 2);
    }

    #[test]
    fn execute_collapse_preserves_system_messages() {
        let config = ContextCollapseConfig {
            preserve_recent_messages: 1,
            preserve_system_messages: true,
            ..ContextCollapseConfig::default()
        };
        let mut engine = ContextCollapseEngine::new(config);
        let messages = vec![
            make_system_msg("system prompt"),
            make_user_msg("msg1"),
            make_user_msg("msg2"),
            make_user_msg("msg3"),
        ];
        let (collapsed, _result) = engine
            .execute_collapse(&messages)
            .expect("collapse should succeed");
        let has_system = collapsed.iter().any(|m| matches!(m, Message::System(_)));
        assert!(has_system);
    }

    #[test]
    fn execute_collapse_removes_old_tool_results() {
        let config = ContextCollapseConfig {
            preserve_recent_messages: 2,
            ..ContextCollapseConfig::default()
        };
        let mut engine = ContextCollapseEngine::new(config);
        let messages = vec![
            make_tool_summary("tc-1", "bash", "old result"),
            make_tool_summary("tc-2", "read", "old file"),
            make_user_msg("recent1"),
            make_assistant_msg("recent2"),
        ];
        let (collapsed, result) = engine
            .execute_collapse(&messages)
            .expect("collapse should succeed");
        assert!(
            result
                .operations_applied
                .contains(&CollapseOperation::RemoveOldToolResults)
        );
        // Tool results in the old section should be removed
        let tool_count = collapsed
            .iter()
            .filter(|m| matches!(m, Message::ToolUseSummary(_)))
            .count();
        assert_eq!(tool_count, 0);
    }

    #[test]
    fn execute_collapse_deduplicates_system_messages() {
        let mut engine = ContextCollapseEngine::with_defaults();
        let messages = vec![
            make_system_msg("system prompt"),
            make_system_msg("system prompt"), // duplicate
            make_system_msg("different prompt"),
            make_user_msg("hello"),
        ];
        let (collapsed, result) = engine
            .execute_collapse(&messages)
            .expect("collapse should succeed");
        assert!(
            result
                .operations_applied
                .contains(&CollapseOperation::DeduplicateSystemMessages)
        );
        assert!(collapsed.len() < messages.len());
    }

    #[test]
    fn collapse_persistence_save_and_load() {
        let mut persistence = CollapsePersistence::new(10);
        let result = CollapseResult::new(1000, 500);
        persistence.save(result);

        assert_eq!(persistence.count(), 1);
        let last = persistence.last().expect("should have a result");
        assert_eq!(last.original_token_count, 1000);
        assert_eq!(last.collapsed_token_count, 500);
    }

    #[test]
    fn collapse_persistence_max_history() {
        let mut persistence = CollapsePersistence::new(3);
        for i in 0..5 {
            persistence.save(CollapseResult::new(i * 100, i * 50));
        }
        assert_eq!(persistence.count(), 3);
    }

    #[test]
    fn collapse_persistence_total_tokens_saved() {
        let mut persistence = CollapsePersistence::new(10);
        persistence.save(CollapseResult::new(1000, 500));
        persistence.save(CollapseResult::new(2000, 1000));
        assert_eq!(persistence.total_tokens_saved(), 1500);
    }

    #[test]
    fn collapse_persistence_clear() {
        let mut persistence = CollapsePersistence::new(10);
        persistence.save(CollapseResult::new(1000, 500));
        persistence.clear();
        assert_eq!(persistence.count(), 0);
        assert!(persistence.last().is_none());
    }

    #[test]
    fn collapse_persistence_has_result() {
        let mut persistence = CollapsePersistence::new(10);
        persistence.save(CollapseResult::new(1000, 500));
        assert!(persistence.has_result(|r| r.original_token_count == 1000));
        assert!(!persistence.has_result(|r| r.original_token_count == 9999));
    }

    #[test]
    fn collapse_result_serialization() {
        let result = CollapseResult::new(1000, 500);
        let json = serde_json::to_string(&result).expect("serialize should succeed");
        let parsed: CollapseResult =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(parsed.original_token_count, 1000);
        assert_eq!(parsed.collapsed_token_count, 500);
    }

    #[test]
    fn collapse_operation_serialization() {
        let op = CollapseOperation::TrimToolOutputs;
        let json = serde_json::to_string(&op).expect("serialize should succeed");
        let parsed: CollapseOperation =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(parsed, op);
    }

    // -- ContextCollapseStrategy tests --

    #[tokio::test]
    async fn strategy_compact_removes_tombstones() {
        use crate::strategy::{CompactOptions, CompactStrategy, FnSummaryProvider};

        let strategy = ContextCollapseStrategy::with_defaults();
        let messages = vec![
            make_user_msg("hello"),
            make_tombstone("old message"),
            make_assistant_msg("world"),
        ];
        let options = CompactOptions::default();
        let provider =
            FnSummaryProvider::new(|_msgs, _sys, _user| Box::pin(async { Ok("summary".into()) }));
        let result = strategy
            .compact(&messages, &options, &provider, None)
            .await
            .expect("should succeed");
        assert!(result.messages_removed >= 1);
        assert!(result.tokens_saved > 0);
    }

    #[tokio::test]
    async fn strategy_compact_preserves_recent() {
        use crate::strategy::{CompactOptions, CompactStrategy, FnSummaryProvider};

        let config = ContextCollapseConfig {
            preserve_recent_messages: 2,
            preserve_system_messages: false,
            ..ContextCollapseConfig::default()
        };
        let strategy = ContextCollapseStrategy::new(config);
        let messages = vec![
            make_user_msg("msg1"),
            make_user_msg("msg2"),
            make_user_msg("msg3"),
            make_user_msg("msg4"),
            make_user_msg("msg5"),
        ];
        let options = CompactOptions::default();
        let provider =
            FnSummaryProvider::new(|_msgs, _sys, _user| Box::pin(async { Ok("summary".into()) }));
        let result = strategy
            .compact(&messages, &options, &provider, None)
            .await
            .expect("should succeed");
        assert!(result.messages_to_keep.len() <= 2);
    }

    #[tokio::test]
    async fn strategy_compact_empty_messages() {
        use crate::strategy::{CompactOptions, CompactStrategy, FnSummaryProvider};

        let strategy = ContextCollapseStrategy::with_defaults();
        let messages: Vec<Message> = Vec::new();
        let options = CompactOptions::default();
        let provider =
            FnSummaryProvider::new(|_msgs, _sys, _user| Box::pin(async { Ok("summary".into()) }));
        let result = strategy
            .compact(&messages, &options, &provider, None)
            .await
            .expect("should succeed");
        assert_eq!(result.messages_removed, 0);
    }

    #[tokio::test]
    async fn strategy_compact_progress_events() {
        use crate::strategy::{
            CompactOptions, CompactProgressEvent, CompactStrategy, FnSummaryProvider,
        };

        let strategy = ContextCollapseStrategy::with_defaults();
        let messages = vec![make_user_msg("hello"), make_tombstone("old")];
        let options = CompactOptions::default();
        let provider =
            FnSummaryProvider::new(|_msgs, _sys, _user| Box::pin(async { Ok("summary".into()) }));

        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let events_clone = events.clone();
        let progress: Box<crate::strategy::ProgressCallback> = Box::new(move |evt| {
            let label = match &evt {
                CompactProgressEvent::Started { strategy } => format!("started:{strategy}"),
                CompactProgressEvent::Completed(r) => format!("completed:{}", r.strategy_used),
                CompactProgressEvent::Failed(msg) => format!("failed:{msg}"),
                CompactProgressEvent::Summarizing { messages_processed } => {
                    format!("summarizing:{messages_processed}")
                }
            };
            events_clone.lock().expect("lock").push(label);
        });

        let _result = strategy
            .compact(&messages, &options, &provider, Some(&*progress))
            .await
            .expect("should succeed");

        let evts = events.lock().expect("lock");
        assert!(evts.iter().any(|e| e.starts_with("started:")));
        assert!(evts.iter().any(|e| e.starts_with("completed:")));
    }

    // -- detect_collapsible_spans tests --

    #[test]
    fn detect_spans_no_tool_messages() {
        let messages = vec![make_user_msg("hello"), make_assistant_msg("world")];
        let spans = detect_collapsible_spans(&messages, 10);
        assert!(spans.is_empty());
    }

    #[test]
    fn detect_spans_consecutive_tool_summaries() {
        let messages = vec![
            make_user_msg("hello"),
            make_tool_summary("tc-1", "bash", &"x".repeat(2000)),
            make_tool_summary("tc-2", "read", &"y".repeat(2000)),
            make_assistant_msg("done"),
        ];
        let spans = detect_collapsible_spans(&messages, 10);
        assert_eq!(
            spans.len(),
            1,
            "should detect one span of consecutive tool results"
        );
        assert_eq!(spans[0].start, 1);
        assert_eq!(spans[0].end, 3);
    }

    #[test]
    fn detect_spans_below_threshold() {
        let messages = vec![make_tool_summary("tc-1", "bash", "short")];
        let spans = detect_collapsible_spans(&messages, 10_000);
        assert!(spans.is_empty(), "short span should be below threshold");
    }
}
