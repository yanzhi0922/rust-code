//! Reactive compact — triggered when a `prompt-too-long` error occurs.
//!
//! Inspired by Claude Code's `query.ts` (lines 1085–1183): when the API
//! returns a prompt-too-long / 413 error, this handler attempts a one-shot
//! reactive compaction to shrink the conversation before retrying.
//!
//! The handler is **bounded**: it will attempt at most `max_attempts` reactive
//! compactions per query to prevent infinite retry loops.

use anyhow::Result;

use claude_core::{Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default maximum number of reactive compact attempts per query.
pub const DEFAULT_MAX_ATTEMPTS: usize = 1;

/// Default number of recent turns to preserve during reactive compact.
pub const DEFAULT_PRESERVE_RECENT_TURNS: usize = 3;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Reactive compact handler — attempts emergency compaction when the API
/// rejects a request because the prompt is too long.
#[derive(Debug, Clone)]
pub struct ReactiveCompactHandler {
    /// Maximum number of reactive compact attempts allowed per query.
    pub max_attempts: usize,
    /// Number of attempts already made in the current query.
    pub attempt_count: usize,
    /// Number of recent message pairs (user + assistant) to preserve.
    pub preserve_recent_turns: usize,
}

impl Default for ReactiveCompactHandler {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            attempt_count: 0,
            preserve_recent_turns: DEFAULT_PRESERVE_RECENT_TURNS,
        }
    }
}

/// Result of a reactive compact operation.
#[derive(Debug, Clone)]
pub struct ReactiveCompactResult {
    /// The compacted message list.
    pub messages: Vec<Message>,
    /// Number of messages removed by compaction.
    pub messages_removed: usize,
    /// Whether a reactive compact was actually performed.
    pub was_compacted: bool,
}

impl ReactiveCompactHandler {
    /// Create a new handler with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new handler with a custom max-attempts limit.
    #[must_use]
    pub fn with_max_attempts(mut self, max: usize) -> Self {
        self.max_attempts = max;
        self
    }

    /// Returns `true` if a reactive compact can still be attempted.
    #[must_use]
    pub fn can_attempt(&self) -> bool {
        self.attempt_count < self.max_attempts
    }

    /// Reset the attempt counter (e.g. at the start of a new query).
    pub fn reset(&mut self) {
        self.attempt_count = 0;
    }

    /// Handle a prompt-too-long error by performing reactive compaction.
    ///
    /// If the handler has already exhausted its attempts, returns the
    /// original messages unchanged with `was_compacted = false`.
    ///
    /// Otherwise, compacts the conversation by:
    /// 1. Preserving all system messages
    /// 2. Preserving compact-summary messages
    /// 3. Preserving the N most recent turns (user/assistant pairs)
    /// 4. Replacing everything else with a tombstone summary
    pub fn handle_prompt_too_long(
        &mut self,
        messages: Vec<Message>,
    ) -> Result<ReactiveCompactResult> {
        if !self.can_attempt() {
            return Ok(ReactiveCompactResult {
                messages,
                messages_removed: 0,
                was_compacted: false,
            });
        }

        self.attempt_count += 1;
        let original_len = messages.len();

        let compacted = Self::compact_messages(messages, self.preserve_recent_turns);

        let messages_removed = original_len.saturating_sub(compacted.len());
        Ok(ReactiveCompactResult {
            messages: compacted,
            messages_removed,
            was_compacted: messages_removed > 0,
        })
    }

    /// Core compaction logic: keep system + summaries + recent tail.
    fn compact_messages(messages: Vec<Message>, preserve_recent: usize) -> Vec<Message> {
        if messages.is_empty() {
            return messages;
        }

        // Identify messages to always keep
        let mut always_keep = Vec::new();
        let mut removable_start = 0;

        for (i, msg) in messages.iter().enumerate() {
            match msg {
                // Always keep system messages
                Message::System(_) => {
                    always_keep.push(msg.clone());
                    removable_start = i + 1;
                }
                // Always keep compact summaries
                Message::Tombstone(_) => {
                    always_keep.push(msg.clone());
                    removable_start = i + 1;
                }
                other => {
                    if let Message::System(sys) = other
                        && sys.base.is_compact_summary
                    {
                        always_keep.push(msg.clone());
                        removable_start = i + 1;
                        continue;
                    }
                    break;
                }
            }
        }

        // Collect the "middle" section (candidates for removal)
        let tail_start = messages.len().saturating_sub(preserve_recent);
        let tail_start = tail_start.max(removable_start);

        let middle: Vec<&Message> = messages[removable_start..tail_start].iter().collect();

        // Build the result
        let mut result = always_keep;

        // If we removed anything from the middle, insert a tombstone
        if !middle.is_empty() {
            let removed_count = middle.len();
            let summary = format!(
                "[Reactive compact: {removed_count} older messages removed to recover from prompt-too-long error]"
            );
            result.push(Message::System(SystemMessage {
                base: MessageBase {
                    origin: Some(MessageOrigin::Compact),
                    is_compact_summary: true,
                    ..MessageBase::default()
                },
                subtype: SystemMessageSubtype::CompactBoundary,
                text: summary,
                error: None,
            }));
        }

        // Append the recent tail
        for msg in messages.into_iter().skip(tail_start) {
            result.push(msg);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{AssistantMessage, UserMessage};

    fn make_user_message(text: &str) -> Message {
        Message::User(UserMessage {
            base: MessageBase::with_origin(MessageOrigin::UserInput),
            text: text.to_owned(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    fn make_assistant_message(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            base: MessageBase::with_origin(MessageOrigin::Provider),
            text: text.to_owned(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        })
    }

    fn make_system_message(text: &str) -> Message {
        Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::System),
            subtype: SystemMessageSubtype::Informational,
            text: text.to_owned(),
            error: None,
        })
    }

    fn make_compact_summary(text: &str) -> Message {
        Message::System(SystemMessage {
            base: MessageBase {
                origin: Some(MessageOrigin::Compact),
                is_compact_summary: true,
                ..MessageBase::default()
            },
            subtype: SystemMessageSubtype::CompactBoundary,
            text: text.to_owned(),
            error: None,
        })
    }

    // ---- Test 1: Handler respects max_attempts ----

    #[test]
    fn handler_respects_max_attempts() {
        let mut handler = ReactiveCompactHandler::new();
        assert!(handler.can_attempt());

        let messages = vec![make_user_message("hello")];
        let _ = handler
            .handle_prompt_too_long(messages.clone())
            .expect("first prompt-too-long handling should succeed");

        assert!(!handler.can_attempt());

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("second prompt-too-long handling should succeed");
        assert!(!result.was_compacted);
    }

    // ---- Test 2: Handler preserves system messages ----

    #[test]
    fn handler_preserves_system_messages() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![
            make_system_message("system prompt"),
            make_user_message("msg 1"),
            make_user_message("msg 2"),
            make_user_message("msg 3"),
            make_user_message("msg 4"),
            make_user_message("msg 5"),
        ];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("prompt-too-long handling should preserve system messages");
        assert!(result.was_compacted);

        let has_system = result
            .messages
            .iter()
            .any(|m| matches!(m, Message::System(sys) if sys.text == "system prompt"));
        assert!(has_system, "System message should be preserved");
    }

    // ---- Test 3: Handler preserves recent turns ----

    #[test]
    fn handler_preserves_recent_turns() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![
            make_system_message("system"),
            make_user_message("old 1"),
            make_assistant_message("resp 1"),
            make_user_message("old 2"),
            make_assistant_message("resp 2"),
            make_user_message("recent 1"),
            make_assistant_message("recent resp 1"),
        ];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("prompt-too-long handling should preserve recent turns");
        assert!(result.was_compacted);

        // Recent messages should be preserved
        let has_recent = result
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(u) if u.text == "recent 1"));
        assert!(has_recent, "Recent user message should be preserved");
    }

    // ---- Test 4: Handler inserts tombstone for removed messages ----

    #[test]
    fn handler_inserts_tombstone_for_removed_messages() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![
            make_system_message("system"),
            make_user_message("old 1"),
            make_user_message("old 2"),
            make_user_message("old 3"),
            make_user_message("old 4"),
            make_user_message("old 5"),
            make_user_message("recent 1"),
        ];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("prompt-too-long handling should insert tombstone");
        assert!(result.was_compacted);
        assert!(result.messages_removed > 0);

        let has_tombstone = result
            .messages
            .iter()
            .any(|m| matches!(m, Message::System(sys) if sys.text.contains("[Reactive compact:")));
        assert!(has_tombstone, "Should contain a reactive compact tombstone");
    }

    // ---- Test 5: Handler is a no-op for short conversations ----

    #[test]
    fn handler_noop_for_short_conversations() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![make_system_message("system"), make_user_message("hello")];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("short conversation handling should succeed");
        // With only 2 messages and preserve_recent=3, nothing gets removed
        assert!(!result.was_compacted);
        assert_eq!(result.messages_removed, 0);
    }

    // ---- Test 6: Reset allows new attempts ----

    #[test]
    fn reset_allows_new_attempts() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![make_user_message("hello")];

        let _ = handler
            .handle_prompt_too_long(messages.clone())
            .expect("first prompt-too-long handling should consume attempt");
        assert!(!handler.can_attempt());

        handler.reset();
        assert!(handler.can_attempt());

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("prompt-too-long handling after reset should succeed");
        // Short conversation, so was_compacted is false but attempt was allowed
        assert!(!result.was_compacted);
    }

    // ---- Test 7: Handler preserves compact summaries ----

    #[test]
    fn handler_preserves_compact_summaries() {
        let mut handler = ReactiveCompactHandler::new();
        let messages = vec![
            make_system_message("system"),
            make_compact_summary("previous summary"),
            make_user_message("old 1"),
            make_user_message("old 2"),
            make_user_message("recent 1"),
        ];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("prompt-too-long handling should preserve compact summaries");

        let has_summary = result
            .messages
            .iter()
            .any(|m| matches!(m, Message::System(sys) if sys.text == "previous summary"));
        assert!(has_summary, "Compact summary should be preserved");
    }

    // ---- Test 8: Empty messages handled gracefully ----

    #[test]
    fn handler_handles_empty_messages() {
        let mut handler = ReactiveCompactHandler::new();
        let messages: Vec<Message> = vec![];

        let result = handler
            .handle_prompt_too_long(messages)
            .expect("empty message handling should succeed");
        // With 0 messages, attempt is consumed but nothing happens
        // Actually, can_attempt is true, so attempt happens
        assert!(!result.was_compacted);
        assert!(result.messages.is_empty());
    }

    // ---- Test 9: Custom max_attempts ----

    #[test]
    fn custom_max_attempts() {
        let handler = ReactiveCompactHandler::new().with_max_attempts(3);
        assert_eq!(handler.max_attempts, 3);
        assert!(handler.can_attempt());
    }
}
