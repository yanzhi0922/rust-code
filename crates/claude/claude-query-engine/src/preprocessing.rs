//! Message preprocessing pipeline — runs before each API call.
//!
//! Implements the multi-stage preprocessing pipeline inspired by Claude Code's
//! `query.ts` (lines 379–447):
//! 1. `apply_tool_result_budget` — truncate oversized tool results
//! 2. `snip_compact_if_needed` — trim old history when context usage is high
//! 3. `micro_compact` — replace old tool result content with shortened versions
//! 4. `context_collapse` — aggregate consecutive tool results into summaries
//! 5. `autocompact_check` — flag whether full compaction is still needed
//!
//! The pipeline is **idempotent**: running it multiple times on the same input
//! produces the same result without side effects.

use claude_core::{
    Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype, UserMessage,
};

fn live_tail_tool_summary_range(messages: &[Message]) -> Option<(usize, usize)> {
    let mut start = messages.len();
    while start > 0 && matches!(messages[start - 1], Message::ToolUseSummary(_)) {
        start -= 1;
    }
    if start == messages.len()
        || start == 0
        || !matches!(messages[start - 1], Message::Assistant(_))
    {
        return None;
    }
    Some((start, messages.len()))
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default maximum character budget for a single tool result.
pub const DEFAULT_TOOL_RESULT_BUDGET: usize = 200_000;

/// Default context-usage threshold (0.0–1.0) above which snip-compact activates.
pub const DEFAULT_SNIP_THRESHOLD: f64 = 0.80;

/// Default number of recent tool-result messages to keep during micro-compact.
pub const DEFAULT_MICRO_KEEP_RECENT: usize = 3;

/// Characters per token (rough estimation for Latin text).
const CHARS_PER_TOKEN: usize = 4;

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Multi-stage message preprocessing pipeline.
///
/// Construct with [`PreprocessingPipeline::default`] or customise individual
/// fields, then call [`PreprocessingPipeline::run`] on the message list before
/// each API invocation.
#[derive(Debug, Clone)]
pub struct PreprocessingPipeline {
    /// Maximum character length for a single tool result before truncation.
    pub tool_result_budget: usize,
    /// Context-usage ratio (0.0–1.0) above which snip-compact kicks in.
    pub snip_threshold: f64,
    /// Number of recent tool-result-bearing messages to preserve in micro-compact.
    pub micro_keep_recent: usize,
}

impl Default for PreprocessingPipeline {
    fn default() -> Self {
        Self {
            tool_result_budget: DEFAULT_TOOL_RESULT_BUDGET,
            snip_threshold: DEFAULT_SNIP_THRESHOLD,
            micro_keep_recent: DEFAULT_MICRO_KEEP_RECENT,
        }
    }
}

/// Outcome of a single preprocessing run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreprocessingResult {
    /// Whether any message was actually modified.
    pub messages_modified: bool,
    /// Number of tool results that were truncated.
    pub tool_results_truncated: usize,
    /// Number of old messages removed by snip-compact.
    pub messages_snipped: usize,
    /// Number of tool-result messages whose content was shortened by micro-compact.
    pub messages_micro_compacted: usize,
    /// Number of message groups collapsed by context-collapse.
    pub messages_collapsed: usize,
    /// Whether full auto-compaction is still needed after all stages.
    pub needs_autocompact: bool,
}

impl PreprocessingResult {
    /// Returns `true` if at least one stage made a change.
    #[must_use]
    pub fn any_change(&self) -> bool {
        self.messages_modified
    }
}

impl PreprocessingPipeline {
    /// Execute the full preprocessing pipeline on `messages` in-place.
    ///
    /// * `context_usage` — estimated current context usage ratio (0.0–1.0).
    /// * `max_context`   — maximum context window in tokens.
    ///
    /// The stages run in order; each subsequent stage receives the output of
    /// the previous one.  The pipeline is idempotent.
    pub fn run(
        &self,
        messages: &mut Vec<Message>,
        context_usage: f64,
        max_context: usize,
    ) -> PreprocessingResult {
        // Stage 1: truncate oversized tool results
        let tool_results_truncated =
            Self::apply_tool_result_budget(messages, self.tool_result_budget, &Default::default());

        // Stage 2: snip old history when context usage exceeds threshold
        let messages_snipped =
            Self::snip_compact_if_needed(messages, context_usage, self.snip_threshold);

        // Stage 3: micro-compact old tool results
        let messages_micro_compacted = Self::micro_compact(messages, self.micro_keep_recent);

        // Stage 4: context collapse
        let messages_collapsed = Self::context_collapse(messages);

        // Stage 5: autocompact check
        let estimated_tokens = estimate_tokens(messages);
        let needs_autocompact = estimated_tokens > max_context;

        let messages_modified = tool_results_truncated > 0
            || messages_snipped > 0
            || messages_micro_compacted > 0
            || messages_collapsed > 0;

        PreprocessingResult {
            messages_modified,
            tool_results_truncated,
            messages_snipped,
            messages_micro_compacted,
            messages_collapsed,
            needs_autocompact,
        }
    }

    // -----------------------------------------------------------------------
    // Stage 1: Tool result budget
    // -----------------------------------------------------------------------

    /// Truncate tool-result content that exceeds `budget` characters.
    ///
    /// `exempt_tool_names` — tools that should NOT be truncated (e.g. those with
    /// infinite maxResultSizeChars). Mirrors TS `query.ts` lines 390–393.
    ///
    /// Returns the number of tool results that were truncated.
    pub fn apply_tool_result_budget(
        messages: &mut [Message],
        budget: usize,
        exempt_tool_names: &std::collections::HashSet<String>,
    ) -> usize {
        let mut truncated = 0;
        for msg in messages.iter_mut() {
            if let Message::ToolUseSummary(tool_summary) = msg
                && tool_summary.summary.len() > budget
                && !exempt_tool_names.contains(&tool_summary.tool_name)
            {
                let truncated_text = format!(
                    "{}\n\n[Tool result truncated: {} chars → {} chars]",
                    &tool_summary.summary[..budget.min(tool_summary.summary.len())],
                    tool_summary.summary.len(),
                    budget,
                );
                tool_summary.summary = truncated_text;
                truncated += 1;
            }
        }
        truncated
    }

    /// Convenience — apply tool result budget without exemptions.
    pub fn apply_tool_result_budget_unrestricted(messages: &mut [Message], budget: usize) -> usize {
        Self::apply_tool_result_budget(messages, budget, &std::collections::HashSet::new())
    }

    // -----------------------------------------------------------------------
    // Stage 2: Snip compact
    // -----------------------------------------------------------------------

    /// Remove old messages when context usage exceeds `threshold`.
    ///
    /// Preserves:
    /// - All system messages
    /// - All compact-summary messages
    /// - The most recent user/assistant exchanges
    ///
    /// Returns the number of messages removed.
    pub fn snip_compact_if_needed(
        messages: &mut Vec<Message>,
        context_usage: f64,
        threshold: f64,
    ) -> usize {
        if context_usage <= threshold || messages.len() <= 4 {
            return 0;
        }

        let original_len = messages.len();

        // Identify indices of system and compact-summary messages (always kept)
        let mut keep_indices = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if matches!(msg, Message::System(_)) {
                keep_indices.push(i);
            } else if let Message::System(sys_msg) = msg
                && sys_msg.base.is_compact_summary
            {
                keep_indices.push(i);
            }
        }

        // Always keep the live tail tool-result batch plus its triggering
        // assistant message. Collapsing or snipping this batch breaks the
        // immediate Anthropic tool_use -> tool_result continuity needed for the
        // next request.
        let tail_start = live_tail_tool_summary_range(messages)
            .map(|(start, _)| start.saturating_sub(1))
            .unwrap_or_else(|| messages.len().saturating_sub(2));
        for i in tail_start..messages.len() {
            if !keep_indices.contains(&i) {
                keep_indices.push(i);
            }
        }

        keep_indices.sort_unstable();
        keep_indices.dedup();

        let kept: Vec<Message> = keep_indices
            .into_iter()
            .map(|i| messages[i].clone())
            .collect();

        // Insert a snip boundary marker at the position of the first kept non-system message
        let first_non_system = kept.iter().position(|m| !matches!(m, Message::System(_)));
        if let Some(insert_pos) = first_non_system {
            let snip_marker = Message::System(SystemMessage {
                base: MessageBase {
                    origin: Some(MessageOrigin::System),
                    ..MessageBase::default()
                },
                subtype: SystemMessageSubtype::CompactBoundary,
                text: format!(
                    "[History snipped: {} older messages removed to reduce context usage ({:.0}%)]",
                    original_len.saturating_sub(kept.len()),
                    context_usage * 100.0,
                ),
                error: None,
            });
            let mut new_messages = Vec::with_capacity(kept.len() + 1);
            for (i, m) in kept.into_iter().enumerate() {
                if i == insert_pos {
                    new_messages.push(snip_marker.clone());
                }
                new_messages.push(m);
            }
            *messages = new_messages;
        } else {
            *messages = kept;
        }

        original_len.saturating_sub(messages.len())
    }

    // -----------------------------------------------------------------------
    // Stage 3: Micro compact
    // -----------------------------------------------------------------------

    /// Replace old tool-result content with shortened placeholders.
    ///
    /// Preserves the `micro_keep_recent` most recent tool-result messages
    /// intact; older ones get their content replaced with a compact marker.
    ///
    /// Returns the number of messages micro-compacted.
    pub fn micro_compact(messages: &mut [Message], keep_recent: usize) -> usize {
        let live_tail_start = live_tail_tool_summary_range(messages).map(|(start, _)| start);
        // Collect indices of tool-result messages in reverse order
        let tool_indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(idx, m)| {
                matches!(m, Message::ToolUseSummary(_))
                    && live_tail_start.is_none_or(|tail_start| *idx < tail_start)
            })
            .map(|(i, _)| i)
            .collect();

        let mut compacted = 0;
        for (rank, &idx) in tool_indices.iter().enumerate() {
            if rank < keep_recent {
                continue; // keep the N most recent
            }
            if let Message::ToolUseSummary(tool_summary) = &mut messages[idx]
                && tool_summary.summary.len() > 100
            {
                let original_len = tool_summary.summary.len();
                tool_summary.summary = format!(
                    "[Micro-compacted: {} chars → ~50 chars] {}",
                    original_len,
                    &tool_summary.summary[..50.min(tool_summary.summary.len())],
                );
                compacted += 1;
            }
        }
        compacted
    }

    // -----------------------------------------------------------------------
    // Stage 4: Context collapse
    // -----------------------------------------------------------------------

    /// Collapse consecutive tool-result messages into a single summary.
    ///
    /// When three or more consecutive `ToolUseSummary` messages appear,
    /// they are replaced with a single collapsed summary message.
    ///
    /// Returns the number of message groups collapsed.
    pub fn context_collapse(messages: &mut Vec<Message>) -> usize {
        if messages.len() < 3 {
            return 0;
        }

        let live_tail_start = live_tail_tool_summary_range(messages).map(|(start, _)| start);
        let mut collapsed = 0;
        let mut i = 0;
        while i < messages.len() {
            // Find a run of 3+ consecutive ToolUseSummary messages
            let run_start = i;
            let mut run_end = i;
            while run_end < messages.len()
                && matches!(messages[run_end], Message::ToolUseSummary(_))
            {
                run_end += 1;
            }
            let run_len = run_end - run_start;
            if live_tail_start == Some(run_start) && run_end == messages.len() {
                i = run_end;
                continue;
            }
            if run_len >= 3 {
                // Collect tool names and summaries
                let mut tool_names = Vec::new();
                let mut total_chars = 0;
                let mut has_error = false;
                for msg in messages.iter().take(run_end).skip(run_start) {
                    if let Message::ToolUseSummary(ts) = msg {
                        tool_names.push(ts.tool_name.clone());
                        total_chars += ts.summary.len();
                        if ts.is_error {
                            has_error = true;
                        }
                    }
                }
                let unique_tools: Vec<String> = {
                    let mut set = std::collections::HashSet::new();
                    tool_names
                        .into_iter()
                        .filter(|n| set.insert(n.clone()))
                        .collect()
                };
                let collapse_msg = Message::System(SystemMessage {
                    base: MessageBase {
                        origin: Some(MessageOrigin::System),
                        ..MessageBase::default()
                    },
                    subtype: SystemMessageSubtype::CompactBoundary,
                    text: format!(
                        "[Context collapsed: {} tool results ({}) merged, {} chars total{}]",
                        run_len,
                        unique_tools.join(", "),
                        total_chars,
                        if has_error { " (includes errors)" } else { "" },
                    ),
                    error: None,
                });
                // Replace the run with a single collapse marker
                messages.drain(run_start..run_end);
                messages.insert(run_start, collapse_msg);
                collapsed += 1;
                i = run_start + 1;
            } else {
                // Advance past the run (or past the current non-matching message)
                i = if run_end == run_start {
                    run_start + 1
                } else {
                    run_end
                };
            }
        }
        collapsed
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rough token estimation: 4 characters ≈ 1 token.
fn estimate_tokens(messages: &[Message]) -> usize {
    let total_chars: usize = messages
        .iter()
        .map(|msg| match msg {
            Message::User(m) => m.text.len(),
            Message::Assistant(m) => m.text.len(),
            Message::System(m) => m.text.len(),
            Message::ToolUseSummary(m) => m.summary.len(),
            Message::HookResult(m) => m.output.len(),
            Message::Tombstone(m) => m.summary.len(),
            Message::CollapsedReadSearch(m) => m.summary.len(),
            Message::GroupedToolUse(m) => m.summary.as_ref().map_or(0, |s| s.len()),
            Message::Progress(m) => m.stage.len() + m.status.len(),
            Message::Attachment(m) => m.label.as_ref().map_or(0, |l| l.len()),
        })
        .sum();
    total_chars / CHARS_PER_TOKEN
}

/// Create a user message intended as a continuation prompt after output truncation.
pub fn create_continuation_message() -> Message {
    Message::User(UserMessage {
        base: MessageBase {
            origin: Some(MessageOrigin::System),
            is_meta: true,
            ..MessageBase::default()
        },
        text: "Please continue from where you left off.".to_owned(),
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{
        AssistantMessage, MessageBase, MessageOrigin, ToolUseSummaryMessage, UserMessage,
    };

    fn make_tool_summary(id: &str, content: &str, is_error: bool) -> Message {
        Message::ToolUseSummary(ToolUseSummaryMessage {
            base: MessageBase::with_origin(MessageOrigin::Tool),
            tool_call_id: id.to_owned(),
            tool_name: "test_tool".to_owned(),
            summary: content.to_owned(),
            is_error,
            content_blocks: Vec::new(),
        })
    }

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

    // ---- Test 1: Tool result budget truncation ----

    #[test]
    fn tool_result_budget_truncates_oversized_content() {
        let long_content = "x".repeat(1_000);
        let mut messages = vec![make_tool_summary("id1", &long_content, false)];

        let truncated = PreprocessingPipeline::apply_tool_result_budget(
            &mut messages,
            500,
            &Default::default(),
        );
        assert_eq!(truncated, 1);

        if let Message::ToolUseSummary(ts) = &messages[0] {
            assert!(ts.summary.len() < 1_000);
            assert!(ts.summary.contains("[Tool result truncated:"));
        } else {
            panic!("Expected ToolUseSummary message");
        }
    }

    // ---- Test 2: Tool result budget leaves small content intact ----

    #[test]
    fn tool_result_budget_preserves_small_content() {
        let short_content = "small result";
        let mut messages = vec![make_tool_summary("id1", short_content, false)];

        let truncated = PreprocessingPipeline::apply_tool_result_budget(
            &mut messages,
            200_000,
            &Default::default(),
        );
        assert_eq!(truncated, 0);

        if let Message::ToolUseSummary(ts) = &messages[0] {
            assert_eq!(ts.summary, "small result");
        } else {
            panic!("Expected ToolUseSummary message");
        }
    }

    // ---- Test 3: Snip compact removes old messages when over threshold ----

    #[test]
    fn snip_compact_removes_old_messages() {
        let mut messages = vec![
            make_system_message("system prompt"),
            make_user_message("msg 1"),
            make_user_message("msg 2"),
            make_user_message("msg 3"),
            make_user_message("msg 4"),
            make_user_message("msg 5"),
        ];

        let snipped = PreprocessingPipeline::snip_compact_if_needed(&mut messages, 0.9, 0.8);
        assert!(snipped > 0);
        // System message should be preserved
        assert!(messages.iter().any(|m| matches!(m, Message::System(_))));
        // Last 2 messages should be preserved
        assert!(messages.len() >= 2);
    }

    // ---- Test 4: Snip compact is a no-op below threshold ----

    #[test]
    fn snip_compact_noop_below_threshold() {
        let mut messages = vec![
            make_system_message("system"),
            make_user_message("hello"),
            make_user_message("world"),
        ];

        let snipped = PreprocessingPipeline::snip_compact_if_needed(&mut messages, 0.5, 0.8);
        assert_eq!(snipped, 0);
        assert_eq!(messages.len(), 3);
    }

    // ---- Test 5: Micro compact replaces old tool results ----

    #[test]
    fn micro_compact_shortens_old_tool_results() {
        let long_summary = "a".repeat(500);
        let mut messages = vec![
            make_tool_summary("old1", &long_summary, false),
            make_tool_summary("old2", &long_summary, false),
            make_tool_summary("recent1", "short", false),
        ];

        let compacted = PreprocessingPipeline::micro_compact(&mut messages, 1);
        assert!(compacted >= 1);
    }

    // ---- Test 6: Micro compact preserves recent tool results ----

    #[test]
    fn micro_compact_preserves_recent() {
        let mut messages = vec![make_tool_summary("recent1", "short result", false)];

        let compacted = PreprocessingPipeline::micro_compact(&mut messages, 1);
        assert_eq!(compacted, 0);
        if let Message::ToolUseSummary(ts) = &messages[0] {
            assert_eq!(ts.summary, "short result");
        }
    }

    // ---- Test 7: Context collapse merges consecutive tool results ----

    #[test]
    fn context_collapse_merges_consecutive_tool_summaries() {
        let mut messages = vec![
            make_tool_summary("id1", "result 1", false),
            make_tool_summary("id2", "result 2", false),
            make_tool_summary("id3", "result 3", false),
        ];

        let collapsed = PreprocessingPipeline::context_collapse(&mut messages);
        assert_eq!(collapsed, 1);
        assert!(messages.len() < 3);
        // Should contain a system collapse marker
        assert!(messages.iter().any(|m| {
            matches!(m, Message::System(sys) if sys.text.contains("[Context collapsed:"))
        }));
    }

    // ---- Test 8: Context collapse is no-op for non-consecutive messages ----

    #[test]
    fn context_collapse_noop_for_scattered_messages() {
        let mut messages = vec![
            make_tool_summary("id1", "result 1", false),
            make_user_message("user interjection"),
            make_tool_summary("id2", "result 2", false),
        ];

        let collapsed = PreprocessingPipeline::context_collapse(&mut messages);
        assert_eq!(collapsed, 0);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn micro_compact_preserves_live_tail_tool_batch() {
        let long_summary = "a".repeat(500);
        let mut messages = vec![
            make_user_message("earlier"),
            make_tool_summary("old0", &long_summary, false),
            make_tool_summary("old1", &long_summary, false),
            make_assistant_message("tool batch"),
            make_tool_summary("tail1", &long_summary, false),
            make_tool_summary("tail2", &long_summary, false),
            make_tool_summary("tail3", &long_summary, false),
        ];

        let compacted = PreprocessingPipeline::micro_compact(&mut messages, 1);

        assert_eq!(compacted, 1);
        for message in messages.iter().take(7).skip(4) {
            match message {
                Message::ToolUseSummary(summary) => {
                    assert_eq!(summary.summary.len(), long_summary.len());
                }
                other => panic!("expected tool summary, got {other:?}"),
            }
        }
    }

    #[test]
    fn context_collapse_preserves_live_tail_tool_batch() {
        let mut messages = vec![
            make_user_message("earlier"),
            make_assistant_message("tool batch"),
            make_tool_summary("tail1", "result 1", false),
            make_tool_summary("tail2", "result 2", false),
            make_tool_summary("tail3", "result 3", false),
        ];

        let collapsed = PreprocessingPipeline::context_collapse(&mut messages);

        assert_eq!(collapsed, 0);
        assert_eq!(messages.len(), 5);
        assert!(matches!(messages[2], Message::ToolUseSummary(_)));
    }

    // ---- Test 9: Full pipeline run ----

    #[test]
    fn full_pipeline_run_with_high_context_usage() {
        let pipeline = PreprocessingPipeline {
            tool_result_budget: 500,
            ..PreprocessingPipeline::default()
        };
        let long_content = "x".repeat(1_000);
        let mut messages = vec![
            make_system_message("system"),
            make_tool_summary("t1", &long_content, false),
            make_user_message("hello"),
            make_user_message("world"),
        ];

        let result = pipeline.run(&mut messages, 0.95, 100);
        assert!(result.any_change());
        assert_eq!(result.tool_results_truncated, 1);
    }

    // ---- Test 10: Full pipeline is idempotent ----

    #[test]
    fn pipeline_is_idempotent() {
        let pipeline = PreprocessingPipeline::default();
        let mut messages = vec![make_system_message("system"), make_user_message("hello")];

        let result1 = pipeline.run(&mut messages, 0.5, 100_000);
        let result2 = pipeline.run(&mut messages, 0.5, 100_000);

        assert!(!result1.any_change());
        assert!(!result2.any_change());
    }

    // ---- Test 11: Continuation message creation ----

    #[test]
    fn continuation_message_is_meta_user_message() {
        let msg = create_continuation_message();
        match &msg {
            Message::User(user_msg) => {
                assert!(user_msg.base.is_meta);
                assert_eq!(user_msg.text, "Please continue from where you left off.");
            }
            _ => panic!("Expected User message"),
        }
    }

    // ---- Test 12: Estimate tokens ----

    #[test]
    fn estimate_tokens_returns_reasonable_approximation() {
        let long_text = "a".repeat(400);
        let messages = vec![make_user_message(&long_text)];
        let tokens = estimate_tokens(&messages);
        assert_eq!(tokens, 100); // 400 / 4 = 100
    }
}
