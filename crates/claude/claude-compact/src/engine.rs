//! Compact main engine.
//!
//! Implements the [`FullCompactStrategy`] and [`PartialCompactStrategy`] that
//! orchestrate the full and partial compaction flows.  The engine accepts a
//! [`SummaryProvider`] callback to call the LLM — it does **not** depend on
//! any specific provider crate.

use claude_core::{
    Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype, UserMessage,
};

use crate::estimate_message_tokens;
use crate::grouping::group_messages_by_api_round;
use crate::prompt::{
    COMPACT_SYSTEM_PROMPT, PartialCompactDirection, build_compact_prompt,
    build_compact_user_summary_message, build_partial_compact_prompt, format_compact_summary,
    rough_token_count,
};
use crate::strategy::{
    CompactOptions, CompactProgressEvent, CompactStrategy, CompactStrategyType, CompactionResult,
    PreservedSegment, ProgressCallback, SummaryProvider,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Error message when there are not enough messages to compact.
pub const ERROR_MESSAGE_NOT_ENOUGH_MESSAGES: &str = "Not enough messages to compact.";

/// Error message when the conversation is too long even after retry.
pub const ERROR_MESSAGE_PROMPT_TOO_LONG: &str =
    "Conversation too long. Press esc twice to go up a few messages and try again.";

/// Error message when the user aborts the compaction.
pub const ERROR_MESSAGE_USER_ABORT: &str = "API Error: Request was aborted.";

/// Error message when the compaction response is incomplete.
pub const ERROR_MESSAGE_INCOMPLETE_RESPONSE: &str =
    "Compaction interrupted · This may be due to network issues — please try again.";

/// Maximum number of prompt-too-long retries.
const MAX_PTL_RETRIES: u32 = 3;

/// Marker inserted when truncating for a PTL retry.
const PTL_RETRY_MARKER: &str = "[earlier conversation truncated for compaction retry]";

/// Maximum consecutive auto-compact failures before the circuit breaker trips.
/// Mirrors `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` from the TS reference.
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: usize = 3;

// ---------------------------------------------------------------------------
// Session-level compact state (circuit breaker)
// ---------------------------------------------------------------------------

/// Mutable session state tracked across compaction attempts.
///
/// After each failed compaction, `consecutive_compact_failures` is incremented.
/// When it reaches [`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`], auto-compact is
/// disabled for the rest of the session (`auto_compact_disabled` is set to
/// `true`).  The counter is reset to 0 on any successful compaction.
///
/// Mirrors the circuit breaker pattern from the TS reference.
#[derive(Debug, Clone)]
pub struct CompactSessionState {
    /// Number of consecutive auto-compact failures.
    pub consecutive_compact_failures: usize,
    /// Whether auto-compact has been permanently disabled for this session.
    pub auto_compact_disabled: bool,
}

impl Default for CompactSessionState {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactSessionState {
    /// Create a new session state with no failures.
    pub fn new() -> Self {
        Self {
            consecutive_compact_failures: 0,
            auto_compact_disabled: false,
        }
    }

    /// Record a successful compaction — resets the failure counter.
    pub fn record_success(&mut self) {
        self.consecutive_compact_failures = 0;
    }

    /// Record a failed compaction — increments the counter and trips the
    /// circuit breaker if the threshold is reached.
    pub fn record_failure(&mut self) {
        self.consecutive_compact_failures += 1;
        if self.consecutive_compact_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
            self.auto_compact_disabled = true;
        }
    }

    /// Whether auto-compact should be allowed, considering the circuit breaker.
    pub fn is_auto_compact_allowed(&self) -> bool {
        !self.auto_compact_disabled
    }
}

// ---------------------------------------------------------------------------
// Full compact strategy
// ---------------------------------------------------------------------------

/// Full conversation compaction strategy.
///
/// Summarises the entire conversation (minus the most recent tail) into a
/// single summary message, then appends the preserved tail and attachments.
pub struct FullCompactStrategy;

#[async_trait::async_trait]
impl CompactStrategy for FullCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::Full
    }

    async fn compact(
        &self,
        messages: &[Message],
        options: &CompactOptions,
        provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        compact_conversation(messages, options, provider, progress).await
    }
}

// ---------------------------------------------------------------------------
// Partial compact strategy
// ---------------------------------------------------------------------------

/// Partial compaction strategy.
///
/// Compacts only one side of a pivot point, keeping the other side intact.
pub struct PartialCompactStrategy {
    /// Index of the pivot message.
    pub pivot_index: usize,
    /// Direction: `From` = summarise after pivot, `UpTo` = summarise before pivot.
    pub direction: PartialCompactDirection,
    /// Optional user feedback to include in the compact prompt.
    pub user_feedback: Option<String>,
}

#[async_trait::async_trait]
impl CompactStrategy for PartialCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::Partial
    }

    async fn compact(
        &self,
        messages: &[Message],
        options: &CompactOptions,
        provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        partial_compact_conversation(
            messages,
            self.pivot_index,
            self.direction,
            self.user_feedback.as_deref(),
            options,
            provider,
            progress,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Core compact implementation
// ---------------------------------------------------------------------------

/// Perform a full compaction of the conversation.
///
/// Mirrors `compactConversation()` from the TypeScript reference.
pub async fn compact_conversation(
    messages: &[Message],
    options: &CompactOptions,
    provider: &dyn SummaryProvider,
    progress: Option<&ProgressCallback>,
) -> Result<CompactionResult, anyhow::Error> {
    if messages.is_empty() {
        return Err(anyhow::anyhow!(ERROR_MESSAGE_NOT_ENOUGH_MESSAGES));
    }

    // Feature flag: DISABLE_COMPACT disables all compaction (manual and auto).
    if std::env::var("DISABLE_COMPACT")
        .ok()
        .as_deref()
        .is_some_and(is_env_truthy)
    {
        return Err(anyhow::anyhow!(
            "Compaction is disabled by DISABLE_COMPACT environment variable"
        ));
    }

    // Feature flag: DISABLE_AUTO_COMPACT disables automatic compaction only.
    if options.is_auto_compact
        && std::env::var("DISABLE_AUTO_COMPACT")
            .ok()
            .as_deref()
            .is_some_and(is_env_truthy)
    {
        return Err(anyhow::anyhow!(
            "Auto-compaction is disabled by DISABLE_AUTO_COMPACT environment variable"
        ));
    }

    // Runtime config / env var check for auto-compact enabled.
    // CLAUDE_CODE_AUTO_COMPACT defaults to "true"; set to "false" to disable.
    if options.is_auto_compact && !is_auto_compact_runtime_enabled() {
        return Err(anyhow::anyhow!(
            "Auto-compaction is disabled by CLAUDE_CODE_AUTO_COMPACT setting"
        ));
    }

    // Fire CompactHooks::pre_compact if configured.
    if let Some(hooks) = &options.compact_hooks {
        hooks.pre_compact(messages)?;
    }

    let pre_compact_token_count = estimate_message_tokens(messages);

    emit_progress(
        &progress,
        CompactProgressEvent::Started {
            strategy: CompactStrategyType::Full,
        },
    );

    // Fire PreCompact hooks (if configured)
    let trigger = if options.is_auto_compact {
        "auto"
    } else {
        "manual"
    };
    let (merged_instructions, pre_display_message) = if let Some(hook_provider) =
        options.pre_compact_hook_provider.as_ref()
    {
        let result = hook_provider(trigger.to_string(), options.custom_instructions.clone()).await;
        (result.new_custom_instructions, result.user_display_message)
    } else {
        (None, None)
    };

    // Merge hook instructions with existing custom instructions
    let effective_instructions = merge_hook_instructions(
        merged_instructions
            .as_deref()
            .or(options.custom_instructions.as_deref()),
        None,
    );

    // Build the compact prompt
    let user_prompt = build_compact_prompt(effective_instructions.as_deref());

    emit_progress(
        &progress,
        CompactProgressEvent::Summarizing {
            messages_processed: 0,
        },
    );

    // Call the LLM to generate the summary — strip images/documents and
    // reinjected skill attachments (skill_discovery / skill_listing) first.
    let mut messages_to_summarize = strip_media_from_messages_ex(messages, true);
    let mut summary = None;
    let mut ptl_attempts = 0;

    for _ in 0..=(MAX_PTL_RETRIES + 1) {
        let result = provider
            .generate_summary(&messages_to_summarize, COMPACT_SYSTEM_PROMPT, &user_prompt)
            .await;

        match result {
            Ok(text) => {
                if text.starts_with(ERROR_MESSAGE_PROMPT_TOO_LONG) {
                    ptl_attempts += 1;
                    if ptl_attempts <= MAX_PTL_RETRIES {
                        let truncated = truncate_head_for_ptl_retry(&messages_to_summarize);
                        if let Some(trunc) = truncated {
                            emit_telemetry(
                                options,
                                "tengu_compact_ptl_retry",
                                serde_json::json!({
                                    "attempt": ptl_attempts,
                                    "droppedMessages": messages_to_summarize.len().saturating_sub(trunc.len()),
                                    "remainingMessages": trunc.len(),
                                }),
                            );
                            messages_to_summarize = trunc;
                            continue;
                        }
                    }
                    emit_telemetry(
                        options,
                        "tengu_compact_failed",
                        serde_json::json!({
                            "reason": "prompt_too_long",
                            "preCompactTokenCount": pre_compact_token_count,
                            "ptlAttempts": ptl_attempts,
                        }),
                    );
                    return Err(anyhow::anyhow!(ERROR_MESSAGE_PROMPT_TOO_LONG));
                }
                if text.is_empty() {
                    emit_telemetry(
                        options,
                        "tengu_compact_failed",
                        serde_json::json!({
                            "reason": "no_summary",
                            "preCompactTokenCount": pre_compact_token_count,
                        }),
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to generate conversation summary - response did not contain valid text content"
                    ));
                }
                summary = Some(text);
                break;
            }
            Err(e) => {
                emit_telemetry(
                    options,
                    "tengu_compact_failed",
                    serde_json::json!({
                        "reason": "api_error",
                        "preCompactTokenCount": pre_compact_token_count,
                        "error": format!("{e:#}"),
                    }),
                );
                return Err(e);
            }
        }
    }

    let summary = summary.ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to generate conversation summary - response did not contain valid text content"
        )
    })?;

    emit_progress(
        &progress,
        CompactProgressEvent::Summarizing {
            messages_processed: messages.len(),
        },
    );

    // Determine how many messages to keep from the tail
    let preserve_count = options.preserve_recent_messages.min(messages.len());
    let messages_removed = messages.len().saturating_sub(preserve_count);

    // Build the compact boundary system message
    let last_pre_compact_uuid = messages.last().map(|m| m.uuid());
    let mut boundary_marker = create_compact_boundary_message(
        trigger,
        pre_compact_token_count,
        last_pre_compact_uuid,
        None,
    );

    // Build the summary user message
    let formatted_summary = format_compact_summary(&summary);
    let summary_text = build_compact_user_summary_message(
        &summary,
        true, // suppress follow-up questions
        None, // transcript path — caller can provide via options
        preserve_count > 0,
    );

    // Estimate post-compact tokens (boundary + summary + kept messages)
    // Mirrors the TS pattern of counting the full post-compact payload.
    let boundary_token_estimate = rough_token_count(
        &serde_json::to_string(
            &serde_json::json!({"trigger": trigger, "preTokens": pre_compact_token_count}),
        )
        .unwrap_or_default(),
    );
    let post_compact_token_count =
        boundary_token_estimate + rough_token_count(&summary_text) + preserve_count as u64 * 100;

    let tokens_saved = pre_compact_token_count.saturating_sub(post_compact_token_count);

    let summary_message = Message::User(UserMessage {
        base: {
            let mut base = MessageBase::with_origin(MessageOrigin::Compact);
            base.is_compact_summary = true;
            base.is_visible_in_transcript_only = true;
            base
        },
        text: summary_text,
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    });

    // Build preserved segments
    let preserved_segments = if preserve_count > 0 {
        let kept: Vec<&Message> = messages.iter().rev().take(preserve_count).collect();
        let seg = PreservedSegment {
            head_uuid: kept.last().map(|m| m.uuid()).unwrap_or_default(),
            anchor_uuid: summary_message.uuid(),
            tail_uuid: kept.first().map(|m| m.uuid()).unwrap_or_default(),
        };
        // Annotate boundary with preservedSegment (TS: annotateBoundaryWithPreservedSegment)
        boundary_marker = annotate_boundary_with_preserved_segment(
            &boundary_marker,
            seg.anchor_uuid,
            &messages
                .iter()
                .rev()
                .take(preserve_count)
                .cloned()
                .collect::<Vec<_>>(),
        );
        vec![seg]
    } else {
        Vec::new()
    };

    // Re-fire SessionStart hooks with source='compact' (TS: processSessionStartHooks)
    let hook_results = if let Some(hook_provider) = options.session_start_hook_provider.as_ref() {
        hook_provider().await
    } else {
        Vec::new()
    };

    // Re-inject deferred tools delta / agent listing / MCP instructions
    // (TS: getDeferredToolsDeltaAttachment with empty messages)
    let mut attachments = vec![boundary_marker, summary_message];
    if let Some(att_provider) = options.post_compact_attachment_provider.as_ref() {
        attachments.extend(att_provider().await);
    }

    let result = CompactionResult {
        summary: formatted_summary,
        messages_removed,
        tokens_saved,
        strategy_used: CompactStrategyType::Full,
        preserved_segments,
        pre_compact_token_count: Some(pre_compact_token_count),
        post_compact_token_count: Some(post_compact_token_count),
        messages_to_keep: messages
            .iter()
            .rev()
            .take(preserve_count)
            .cloned()
            .collect(),
        attachments,
        hook_results,
        user_display_message: pre_display_message.clone(),
    };

    // Fire PostCompact hooks (if configured)
    let post_display = if let Some(hook_provider) = options.post_compact_hook_provider.as_ref() {
        let hook_result = hook_provider(trigger.to_string(), result.summary.clone()).await;
        hook_result.user_display_message
    } else {
        None
    };

    // Merge pre and post display messages
    let final_display = match (pre_display_message, post_display) {
        (Some(pre), Some(post)) => Some(format!("{pre}\n{post}")),
        (Some(pre), None) => Some(pre),
        (None, Some(post)) => Some(post),
        (None, None) => None,
    };

    let mut result = result;
    result.user_display_message = final_display;

    // Post-compact cleanup (cache resets)
    if let Some(cleanup_provider) = options.post_compact_cleanup_provider.as_ref() {
        cleanup_provider().await;
    }

    // Telemetry: successful full compaction
    let ri = options.recompaction_info.as_ref();
    let query_source_for_event = ri
        .and_then(|r| r.query_source.as_deref())
        .unwrap_or("unknown");
    let auto_compact_threshold = ri.map(|r| r.auto_compact_threshold).unwrap_or(0);
    let true_post_compact = result.post_compact_token_count.unwrap_or(0);
    emit_telemetry(
        options,
        "tengu_compact",
        serde_json::json!({
            "preCompactTokenCount": result.pre_compact_token_count,
            "postCompactTokenCount": result.post_compact_token_count,
            "truePostCompactTokenCount": true_post_compact,
            "autoCompactThreshold": auto_compact_threshold,
            "willRetriggerNextTurn": ri.is_some() && true_post_compact >= auto_compact_threshold,
            "isAutoCompact": options.is_auto_compact,
            "querySource": query_source_for_event,
            "isRecompactionInChain": ri.map(|r| r.is_recompaction_in_chain).unwrap_or(false),
            "turnsSincePreviousCompact": ri.map(|r| r.turns_since_previous_compact).unwrap_or(-1),
            "previousCompactTurnId": ri.and_then(|r| r.previous_compact_turn_id.as_deref()).unwrap_or(""),
            "messagesRemoved": result.messages_removed,
            "tokensSaved": result.tokens_saved,
        }),
    );

    emit_progress(&progress, CompactProgressEvent::Completed(result.clone()));

    // Fire CompactHooks::post_compact if configured.
    if let Some(hooks) = &options.compact_hooks {
        let post_messages = build_post_compact_messages(&result);
        if let Err(e) = hooks.post_compact(&post_messages, result.messages_removed) {
            tracing::warn!("post_compact hook failed: {e:#}");
        }
    }

    Ok(result)
}

/// Perform a partial compaction around a pivot message.
///
/// Mirrors `partialCompactConversation()` from the TypeScript reference.
pub async fn partial_compact_conversation(
    all_messages: &[Message],
    pivot_index: usize,
    direction: PartialCompactDirection,
    user_feedback: Option<&str>,
    options: &CompactOptions,
    provider: &dyn SummaryProvider,
    progress: Option<&ProgressCallback>,
) -> Result<CompactionResult, anyhow::Error> {
    if all_messages.is_empty() {
        return Err(anyhow::anyhow!(ERROR_MESSAGE_NOT_ENOUGH_MESSAGES));
    }

    let (messages_to_summarize_raw, messages_to_keep): (Vec<Message>, Vec<Message>) =
        match direction {
            PartialCompactDirection::UpTo => {
                let to_summarize: Vec<Message> =
                    all_messages.iter().take(pivot_index).cloned().collect();
                // Strip stale compact boundaries and old summaries from kept portion
                let to_keep: Vec<Message> = all_messages
                .iter()
                .skip(pivot_index)
                .filter(|m| {
                    // Filter out Progress messages
                    if matches!(m, Message::Progress(_)) {
                        return false;
                    }
                    // Filter out stale compact boundaries
                    if matches!(m, Message::System(s) if s.subtype == SystemMessageSubtype::CompactBoundary) {
                        return false;
                    }
                    // Filter out old compact summaries
                    if matches!(m, Message::User(u) if u.base.is_compact_summary) {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect();
                (to_summarize, to_keep)
            }
            PartialCompactDirection::From => {
                let to_summarize: Vec<Message> =
                    all_messages.iter().skip(pivot_index).cloned().collect();
                let to_keep: Vec<Message> = all_messages
                    .iter()
                    .take(pivot_index)
                    .filter(|m| !matches!(m, Message::Progress(_)))
                    .cloned()
                    .collect();
                (to_summarize, to_keep)
            }
        };

    // Strip media blocks and reinjected skill attachments before sending to the summarizer
    let messages_to_summarize = strip_media_from_messages_ex(&messages_to_summarize_raw, true);

    if messages_to_summarize.is_empty() {
        return Err(anyhow::anyhow!(
            "Nothing to summarize {} the selected message.",
            match direction {
                PartialCompactDirection::UpTo => "before",
                PartialCompactDirection::From => "after",
            }
        ));
    }

    let pre_compact_token_count = estimate_message_tokens(all_messages);

    emit_progress(
        &progress,
        CompactProgressEvent::Started {
            strategy: CompactStrategyType::Partial,
        },
    );

    // Build custom instructions from user feedback + hook instructions
    let custom_instructions = match (options.custom_instructions.as_deref(), user_feedback) {
        (Some(hook), Some(feedback)) => Some(format!("{hook}\n\nUser context: {feedback}")),
        (Some(hook), None) => Some(hook.to_string()),
        (None, Some(feedback)) => Some(format!("User context: {feedback}")),
        (None, None) => None,
    };

    let user_prompt = build_partial_compact_prompt(custom_instructions.as_deref(), direction);

    emit_progress(
        &progress,
        CompactProgressEvent::Summarizing {
            messages_processed: 0,
        },
    );

    // Call the LLM with PTL retry loop
    let mut messages_to_summarize = messages_to_summarize;
    let mut summary = None;
    let mut ptl_attempts = 0;

    for _ in 0..=(MAX_PTL_RETRIES + 1) {
        let result = provider
            .generate_summary(&messages_to_summarize, COMPACT_SYSTEM_PROMPT, &user_prompt)
            .await;

        match result {
            Ok(text) => {
                if text.starts_with(ERROR_MESSAGE_PROMPT_TOO_LONG) {
                    ptl_attempts += 1;
                    if ptl_attempts <= MAX_PTL_RETRIES {
                        let truncated = truncate_head_for_ptl_retry(&messages_to_summarize);
                        if let Some(trunc) = truncated {
                            emit_telemetry(
                                options,
                                "tengu_compact_ptl_retry",
                                serde_json::json!({
                                    "attempt": ptl_attempts,
                                    "droppedMessages": messages_to_summarize.len().saturating_sub(trunc.len()),
                                    "remainingMessages": trunc.len(),
                                    "path": "partial",
                                }),
                            );
                            messages_to_summarize = trunc;
                            continue;
                        }
                    }
                    emit_telemetry(
                        options,
                        "tengu_partial_compact_failed",
                        serde_json::json!({
                            "reason": "prompt_too_long",
                            "ptlAttempts": ptl_attempts,
                        }),
                    );
                    return Err(anyhow::anyhow!(ERROR_MESSAGE_PROMPT_TOO_LONG));
                }
                if text.is_empty() {
                    emit_telemetry(
                        options,
                        "tengu_partial_compact_failed",
                        serde_json::json!({
                            "reason": "no_summary",
                        }),
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to generate conversation summary - response did not contain valid text content"
                    ));
                }
                summary = Some(text);
                break;
            }
            Err(e) => {
                emit_telemetry(
                    options,
                    "tengu_partial_compact_failed",
                    serde_json::json!({
                        "reason": "api_error",
                        "error": format!("{e:#}"),
                    }),
                );
                return Err(e);
            }
        }
    }

    let summary = summary.ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to generate conversation summary - response did not contain valid text content"
        )
    })?;

    emit_progress(
        &progress,
        CompactProgressEvent::Summarizing {
            messages_processed: messages_to_summarize.len(),
        },
    );

    let formatted_summary = format_compact_summary(&summary);
    let summary_text =
        build_compact_user_summary_message(&summary, false, None, !messages_to_keep.is_empty());

    let has_kept = !messages_to_keep.is_empty();

    // Build summarizeMetadata for the summary message (TS: summarizeMetadata)
    let summarize_metadata = if has_kept {
        Some(serde_json::json!({
            "messagesSummarized": messages_to_summarize.len(),
            "userContext": user_feedback,
            "direction": match direction {
                PartialCompactDirection::UpTo => "up_to",
                PartialCompactDirection::From => "from",
            },
        }))
    } else {
        None
    };

    let summary_message = Message::User(UserMessage {
        base: {
            let mut base = MessageBase::with_origin(MessageOrigin::Compact);
            base.is_compact_summary = true;
            // TS: only set isVisibleInTranscriptOnly when there are NO messages to keep
            if !has_kept {
                base.is_visible_in_transcript_only = true;
            }
            base
        },
        text: summary_text.clone(),
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata,
    });

    // Build boundary marker with direction-dependent lastPreCompactMessageUuid
    // TS: direction==='up_to' → last non-progress message before pivot; 'from' → last kept message
    let trigger_str = if options.is_auto_compact {
        "auto"
    } else {
        "manual"
    };
    let last_pre_compact_uuid: Option<uuid::Uuid> = match direction {
        PartialCompactDirection::UpTo => all_messages
            .iter()
            .take(pivot_index)
            .rev()
            .find(|m| !matches!(m, Message::Progress(_)))
            .map(|m| m.uuid()),
        PartialCompactDirection::From => messages_to_keep.last().map(|m| m.uuid()),
    };
    let mut boundary_marker = create_compact_boundary_message(
        trigger_str,
        pre_compact_token_count,
        last_pre_compact_uuid,
        None,
    );
    // Enrich boundary with userContext and messagesSummarized
    if let Some(uf) = user_feedback {
        boundary_marker = enrich_boundary_metadata(&boundary_marker, |meta| {
            meta["userContext"] = serde_json::Value::String(uf.to_string());
        });
    }
    boundary_marker = enrich_boundary_metadata(&boundary_marker, |meta| {
        meta["messagesSummarized"] = serde_json::json!(messages_to_summarize.len());
    });

    let post_compact_token_count = rough_token_count(&formatted_summary)
        + rough_token_count(&summary_text)
        + estimate_message_tokens(&messages_to_keep);

    let tokens_saved = pre_compact_token_count.saturating_sub(post_compact_token_count);

    let preserved_segments = if !messages_to_keep.is_empty() {
        // TS: 'from' → prefix-preserving (anchor = boundary); 'up_to' → suffix (anchor = last summary)
        let anchor_uuid = match direction {
            PartialCompactDirection::UpTo => summary_message.uuid(),
            PartialCompactDirection::From => boundary_marker.uuid(),
        };
        let seg = PreservedSegment {
            head_uuid: messages_to_keep
                .first()
                .map(|m| m.uuid())
                .unwrap_or_default(),
            anchor_uuid,
            tail_uuid: messages_to_keep
                .last()
                .map(|m| m.uuid())
                .unwrap_or_default(),
        };
        // Annotate boundary with preservedSegment (TS: annotateBoundaryWithPreservedSegment)
        boundary_marker = annotate_boundary_with_preserved_segment(
            &boundary_marker,
            seg.anchor_uuid,
            &messages_to_keep,
        );
        vec![seg]
    } else {
        Vec::new()
    };

    // Re-fire SessionStart hooks with source='compact'
    let hook_results = if let Some(hook_provider) = options.session_start_hook_provider.as_ref() {
        hook_provider().await
    } else {
        Vec::new()
    };

    // Re-inject deferred tools delta / agent listing / MCP instructions
    let mut attachments = vec![boundary_marker, summary_message];
    if let Some(att_provider) = options.post_compact_attachment_provider.as_ref() {
        attachments.extend(att_provider().await);
    }

    let messages_kept_count = messages_to_keep.len();
    let messages_summarized_count = messages_to_summarize.len();

    let result = CompactionResult {
        summary: formatted_summary,
        messages_removed: messages_to_summarize.len(),
        tokens_saved,
        strategy_used: CompactStrategyType::Partial,
        preserved_segments,
        pre_compact_token_count: Some(pre_compact_token_count),
        post_compact_token_count: Some(post_compact_token_count),
        messages_to_keep,
        attachments,
        hook_results,
        user_display_message: None,
    };

    // Fire PostCompact hooks (if configured)
    let partial_trigger = if options.is_auto_compact {
        "auto"
    } else {
        "manual"
    };
    let post_display = if let Some(hook_provider) = options.post_compact_hook_provider.as_ref() {
        let hook_result = hook_provider(partial_trigger.to_string(), result.summary.clone()).await;
        hook_result.user_display_message
    } else {
        None
    };

    let mut result = result;
    result.user_display_message = post_display;

    // Post-compact cleanup (cache resets)
    if let Some(cleanup_provider) = options.post_compact_cleanup_provider.as_ref() {
        cleanup_provider().await;
    }

    // Telemetry: successful partial compaction
    emit_telemetry(
        options,
        "tengu_partial_compact",
        serde_json::json!({
            "preCompactTokenCount": result.pre_compact_token_count,
            "postCompactTokenCount": result.post_compact_token_count,
            "messagesKept": messages_kept_count,
            "messagesSummarized": messages_summarized_count,
            "direction": match direction {
                PartialCompactDirection::UpTo => "up_to",
                PartialCompactDirection::From => "from",
            },
            "hasUserFeedback": user_feedback.is_some(),
            "isAutoCompact": options.is_auto_compact,
            "messagesRemoved": result.messages_removed,
            "tokensSaved": result.tokens_saved,
        }),
    );

    emit_progress(&progress, CompactProgressEvent::Completed(result.clone()));

    Ok(result)
}

// ---------------------------------------------------------------------------
// Build post-compact messages
// ---------------------------------------------------------------------------

/// Build the ordered list of messages that replaces the conversation after
/// compaction.
///
/// Mirrors `buildPostCompactMessages()` from the TypeScript reference.
pub fn build_post_compact_messages(result: &CompactionResult) -> Vec<Message> {
    let mut messages = Vec::new();

    // Boundary marker (first attachment)
    for msg in &result.attachments {
        if matches!(msg, Message::System(s) if s.subtype == SystemMessageSubtype::CompactBoundary) {
            messages.push(msg.clone());
            break;
        }
    }

    // Summary messages
    for msg in &result.attachments {
        if matches!(msg, Message::User(u) if u.base.is_compact_summary) {
            messages.push(msg.clone());
        }
    }

    // Preserved messages
    messages.extend(result.messages_to_keep.clone());

    // Remaining attachments (non-boundary, non-summary)
    for msg in &result.attachments {
        let is_boundary =
            matches!(msg, Message::System(s) if s.subtype == SystemMessageSubtype::CompactBoundary);
        let is_summary = matches!(msg, Message::User(u) if u.base.is_compact_summary);
        if !is_boundary && !is_summary {
            messages.push(msg.clone());
        }
    }

    // Hook results
    messages.extend(result.hook_results.clone());

    messages
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Emit a progress event if a sink is provided.
fn emit_progress(sink: &Option<&ProgressCallback>, event: CompactProgressEvent) {
    if let Some(sink) = sink {
        sink(event);
    }
}

/// Attempt to truncate the oldest messages to recover from a prompt-too-long
/// error during compaction.
///
/// Mirrors `truncateHeadForPTLRetry()` from the TypeScript reference exactly:
/// 1. Strip any prior retry marker left from a previous attempt
/// 2. Group messages by API round (assistant UUID boundary)
/// 3. If a `token_gap` is provided, drop just enough groups to cover it
/// 4. Otherwise fall back to dropping 20% of groups
/// 5. Ensure the first message is a user message (prepend synthetic if needed)
pub fn truncate_head_for_ptl_retry(messages: &[Message]) -> Option<Vec<Message>> {
    truncate_head_for_ptl_retry_with_gap(messages, None)
}

/// Extended variant that accepts an optional token gap for precise truncation.
pub fn truncate_head_for_ptl_retry_with_gap(
    messages: &[Message],
    token_gap: Option<u64>,
) -> Option<Vec<Message>> {
    // Step 1: Strip any prior retry marker
    let input: Vec<Message> = if messages.len() > 1 {
        let first = &messages[0];
        if let Message::User(u) = first {
            if u.base.is_meta && u.text == PTL_RETRY_MARKER {
                messages[1..].to_vec()
            } else {
                messages.to_vec()
            }
        } else {
            messages.to_vec()
        }
    } else {
        messages.to_vec()
    };

    if input.len() < 2 {
        return None;
    }

    // Step 2: Group by API round
    let groups = group_messages_by_api_round(&input);
    if groups.len() < 2 {
        return None;
    }

    // Step 3: Decide how many groups to drop
    let drop_count = if let Some(gap) = token_gap {
        // Token-gap-based: accumulate group tokens until we cover the gap
        let mut acc: u64 = 0;
        let mut count = 0;
        for group in &groups {
            acc += estimate_message_tokens(group);
            count += 1;
            if acc >= gap {
                break;
            }
        }
        count
    } else {
        // Percentage fallback: drop 20% of groups
        std::cmp::max(1, groups.len() / 5)
    };

    // Step 4: Clamp — must keep at least 1 group
    let drop_count = std::cmp::min(drop_count, groups.len().saturating_sub(1));
    if drop_count < 1 {
        return None;
    }

    // Step 5: Slice and fix role ordering
    let remaining: Vec<Message> = groups.into_iter().skip(drop_count).flatten().collect();

    if remaining.is_empty() {
        return None;
    }

    if matches!(remaining.first(), Some(Message::Assistant(_))) {
        let mut result = vec![Message::User(UserMessage {
            base: {
                let mut base = MessageBase::with_origin(MessageOrigin::Compact);
                base.is_meta = true;
                base
            },
            text: PTL_RETRY_MARKER.to_string(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })];
        result.extend(remaining);
        Some(result)
    } else {
        Some(remaining)
    }
}

/// Create a compact boundary system message with structured metadata.
///
/// Mirrors `createCompactBoundaryMessage()` from the TS reference.
/// The boundary stores JSON metadata in the text field.
///
/// # Arguments
///
/// * `trigger` - "auto" or "manual".
/// * `pre_compact_token_count` - Token count before compaction.
/// * `last_message_uuid` - UUID of the last message before compaction.
/// * `discovered_tools` - Optional list of tool names discovered before compaction.
pub fn create_compact_boundary_message(
    trigger: &str,
    pre_compact_token_count: u64,
    last_message_uuid: Option<uuid::Uuid>,
    discovered_tools: Option<&[String]>,
) -> Message {
    let mut metadata = serde_json::json!({
        "trigger": trigger,
        "preTokens": pre_compact_token_count,
    });
    if let Some(uuid) = last_message_uuid {
        metadata["lastPreCompactMessageUuid"] = serde_json::Value::String(uuid.to_string());
    }
    if let Some(tools) = discovered_tools
        && !tools.is_empty()
    {
        metadata["preCompactDiscoveredTools"] = serde_json::Value::Array(
            tools
                .iter()
                .map(|t| serde_json::Value::String(t.clone()))
                .collect(),
        );
    }

    let mut base = MessageBase::with_origin(MessageOrigin::Compact);
    if let Some(uuid) = last_message_uuid {
        base.parent_uuid = Some(uuid);
    }

    Message::System(SystemMessage {
        base,
        subtype: SystemMessageSubtype::CompactBoundary,
        text: serde_json::to_string(&metadata).unwrap_or_else(|_| {
            format!("{{\"trigger\":\"{trigger}\",\"preTokens\":{pre_compact_token_count}}}")
        }),
        error: None,
    })
}

/// Annotate a compact boundary with preserved segment relink metadata.
///
/// If `messages_to_keep` is empty, returns the boundary unmodified.
/// Otherwise adds `preservedSegment: { headUuid, anchorUuid, tailUuid }`
/// to the boundary's `compactMetadata`.
///
/// Mirrors `annotateBoundaryWithPreservedSegment()` from the TS reference.
pub fn annotate_boundary_with_preserved_segment(
    boundary: &Message,
    anchor_uuid: uuid::Uuid,
    messages_to_keep: &[Message],
) -> Message {
    if messages_to_keep.is_empty() {
        return boundary.clone();
    }

    enrich_boundary_metadata(boundary, |meta| {
        meta["preservedSegment"] = serde_json::json!({
            "headUuid": messages_to_keep.first().map(|m| m.uuid()).unwrap_or_default().to_string(),
            "anchorUuid": anchor_uuid.to_string(),
            "tailUuid": messages_to_keep.last().map(|m| m.uuid()).unwrap_or_default().to_string(),
        });
    })
}

/// Enrich the JSON metadata of a compact boundary message.
///
/// Parses the `text` field as JSON, applies `f` to mutate the metadata
/// object, then re-serializes. If parsing fails, returns the boundary
/// unmodified.
fn enrich_boundary_metadata(boundary: &Message, f: impl FnOnce(&mut serde_json::Value)) -> Message {
    match boundary {
        Message::System(sys) if sys.subtype == SystemMessageSubtype::CompactBoundary => {
            let mut metadata: serde_json::Value =
                serde_json::from_str(&sys.text).unwrap_or_else(|_| serde_json::json!({}));
            f(&mut metadata);
            Message::System(SystemMessage {
                base: sys.base.clone(),
                subtype: sys.subtype.clone(),
                text: serde_json::to_string(&metadata).unwrap_or_else(|_| sys.text.clone()),
                error: sys.error.clone(),
            })
        }
        other => other.clone(),
    }
}

/// Merge user-supplied custom instructions with hook-provided instructions.
pub fn merge_hook_instructions(
    user_instructions: Option<&str>,
    hook_instructions: Option<&str>,
) -> Option<String> {
    match (user_instructions, hook_instructions) {
        (Some(user), Some(hook)) => {
            let trimmed_user = user.trim();
            let trimmed_hook = hook.trim();
            if trimmed_user.is_empty() && trimmed_hook.is_empty() {
                None
            } else if trimmed_user.is_empty() {
                Some(trimmed_hook.to_string())
            } else if trimmed_hook.is_empty() {
                Some(trimmed_user.to_string())
            } else {
                Some(format!("{trimmed_user}\n\n{trimmed_hook}"))
            }
        }
        (Some(user), None) => {
            let trimmed = user.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        (None, Some(hook)) => {
            let trimmed = hook.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        (None, None) => None,
    }
}

/// Strip image and document content blocks from messages before sending to the
/// summarizer LLM.
///
/// Mirrors `stripImagesFromMessages()` from the TypeScript reference.
/// Replaces `image` blocks with `{ type: "text", text: "[image]" }` and
/// `document` blocks with `{ type: "text", text: "[document]" }`. Also handles
/// nested media inside `tool_result` content arrays.
///
/// Only processes `User` messages — other message types pass through unchanged.
///
/// # Also strips reinjected attachments
///
/// If `strip_skill_attachments` is true, also removes `Attachment` messages
/// whose type is `skill_discovery` or `skill_listing`, as these are re-injected
/// post-compaction anyway. Mirrors `stripReinjectedAttachments()`.
pub fn strip_media_from_messages(messages: &[Message]) -> Vec<Message> {
    strip_media_from_messages_ex(messages, false)
}

/// Extended variant that can also strip skill-related attachment messages.
///
/// When `strip_skill_attachments` is true, removes `Message::Attachment`
/// messages whose label matches `"skill_discovery"` or `"skill_listing"`.
/// These are re-injected post-compaction anyway (via
/// `resetSentSkillNames` + the next turn's discovery signal), so feeding
/// them to the summarizer wastes tokens and pollutes the summary with
/// stale skill suggestions.
///
/// Mirrors `stripReinjectedAttachments()` from the TS reference.
pub fn strip_media_from_messages_ex(
    messages: &[Message],
    strip_skill_attachments: bool,
) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| {
            // Filter out skill attachment messages when requested.
            if strip_skill_attachments
                && let Message::Attachment(att) = message
                && (att.label.as_deref() == Some("skill_discovery")
                    || att.label.as_deref() == Some("skill_listing"))
            {
                return false;
            }
            true
        })
        .map(|message| {
            let Message::User(user_msg) = message else {
                return message.clone();
            };

            let content = user_msg.provider_content_blocks();
            let mut has_media = false;

            let new_content: Vec<serde_json::Value> = content
                .into_iter()
                .flat_map(|block| strip_media_from_block(block, &mut has_media))
                .collect();

            if !has_media {
                return message.clone();
            }

            let mut stripped = user_msg.clone();
            stripped.provider_content_blocks = new_content;
            stripped.attachments = Vec::new();
            Message::User(stripped)
        })
        .collect()
}

/// Process a single content block, replacing image/document with text
/// placeholders. Returns a Vec to match the TS `flatMap` behavior.
fn strip_media_from_block(
    block: serde_json::Value,
    has_media: &mut bool,
) -> Vec<serde_json::Value> {
    let block_type = block
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    match block_type {
        "image" => {
            *has_media = true;
            vec![serde_json::json!({"type": "text", "text": "[image]"})]
        }
        "document" => {
            *has_media = true;
            vec![serde_json::json!({"type": "text", "text": "[document]"})]
        }
        "tool_result" => {
            if let Some(content_arr) = block.get("content").and_then(serde_json::Value::as_array) {
                let mut tool_has_media = false;
                let new_tool_content: Vec<serde_json::Value> = content_arr
                    .iter()
                    .map(|item| {
                        let item_type = item
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        match item_type {
                            "image" => {
                                tool_has_media = true;
                                serde_json::json!({"type": "text", "text": "[image]"})
                            }
                            "document" => {
                                tool_has_media = true;
                                serde_json::json!({"type": "text", "text": "[document]"})
                            }
                            _ => item.clone(),
                        }
                    })
                    .collect();
                if tool_has_media {
                    *has_media = true;
                    let mut new_block = block;
                    new_block["content"] = serde_json::Value::Array(new_tool_content);
                    vec![new_block]
                } else {
                    vec![block]
                }
            } else {
                vec![block]
            }
        }
        _ => vec![block],
    }
}

/// Emit a telemetry event if a provider is configured.
fn emit_telemetry(options: &CompactOptions, event_name: &str, metadata: serde_json::Value) {
    if let Some(ref provider) = options.telemetry_provider {
        provider(event_name, metadata);
    }
}

/// Check whether a string value is truthy (mirrors TS `isEnvTruthy`).
fn is_env_truthy(val: &str) -> bool {
    matches!(val.to_lowercase().as_str(), "1" | "true" | "yes")
}

/// Check whether auto-compact is enabled at runtime.
///
/// Reads the `CLAUDE_CODE_AUTO_COMPACT` environment variable (default `"true"`).
/// When set to `"false"`, `"0"`, or `"no"`, auto-compact is disabled.
/// This complements the `DISABLE_AUTO_COMPACT` kill-switch.
fn is_auto_compact_runtime_enabled() -> bool {
    std::env::var("CLAUDE_CODE_AUTO_COMPACT")
        .ok()
        .as_deref()
        .is_none_or(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_hook_instructions_both() {
        let result = merge_hook_instructions(Some("user"), Some("hook"));
        assert_eq!(result.as_deref(), Some("user\n\nhook"));
    }

    #[test]
    fn merge_hook_instructions_user_only() {
        let result = merge_hook_instructions(Some("user"), None);
        assert_eq!(result.as_deref(), Some("user"));
    }

    #[test]
    fn merge_hook_instructions_empty() {
        let result = merge_hook_instructions(Some(""), Some(""));
        assert!(result.is_none());
    }

    #[test]
    fn truncate_head_for_ptl_retry_returns_none_for_single() {
        let msgs = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: "hello".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })];
        assert!(truncate_head_for_ptl_retry(&msgs).is_none());
    }

    #[test]
    fn truncate_head_strips_prior_retry_marker() {
        let marker = Message::User(UserMessage {
            base: MessageBase {
                is_meta: true,
                ..Default::default()
            },
            text: PTL_RETRY_MARKER.to_string(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let a1 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "response".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let u1 = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "followup".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let a2 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "response2".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let msgs = vec![marker, a1, u1, a2];
        let result = truncate_head_for_ptl_retry(&msgs);
        // Should succeed (2 API rounds after stripping marker)
        assert!(result.is_some());
    }

    #[test]
    fn truncate_head_with_token_gap_drops_precise_groups() {
        let a1 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "first response".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let u_big = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "x".repeat(4000), // ~1000 tokens
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let a2 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "second response".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let u_small = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "small".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let a3 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "third".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let msgs = vec![a1, u_big, a2, u_small, a3];
        // With a small token gap, should drop only the first group
        let result = truncate_head_for_ptl_retry_with_gap(&msgs, Some(500));
        assert!(result.is_some());
    }

    #[test]
    fn build_post_compact_messages_orders_correctly() {
        let boundary = create_compact_boundary_message("manual", 1000, None, None);
        let summary = Message::User(UserMessage {
            base: MessageBase {
                is_compact_summary: true,
                ..MessageBase::default()
            },
            text: "summary".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });

        let result = CompactionResult {
            summary: "summary".into(),
            messages_removed: 5,
            tokens_saved: 500,
            strategy_used: CompactStrategyType::Full,
            preserved_segments: Vec::new(),
            pre_compact_token_count: Some(1000),
            post_compact_token_count: Some(500),
            messages_to_keep: vec![Message::User(UserMessage {
                base: MessageBase::default(),
                text: "kept".into(),
                attachments: Vec::new(),
                provider_content_blocks: Vec::new(),
                summarize_metadata: None,
            })],
            attachments: vec![boundary.clone(), summary.clone()],
            hook_results: Vec::new(),
            user_display_message: None,
        };

        let built = build_post_compact_messages(&result);
        assert!(matches!(built.first(), Some(Message::System(_))));
        assert!(
            built
                .iter()
                .any(|m| matches!(m, Message::User(u) if u.base.is_compact_summary))
        );
        assert!(
            built
                .iter()
                .any(|m| matches!(m, Message::User(u) if u.text == "kept"))
        );
    }

    #[test]
    fn strip_media_replaces_image_blocks() {
        use serde_json::json;

        let msgs = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: "check this".into(),
            attachments: Vec::new(),
            provider_content_blocks: vec![
                json!({"type": "text", "text": "check this"}),
                json!({"type": "image", "source": {"type": "base64", "data": "abc123"}}),
            ],
            summarize_metadata: None,
        })];
        let stripped = strip_media_from_messages(&msgs);
        match &stripped[0] {
            Message::User(u) => {
                assert_eq!(u.provider_content_blocks.len(), 2);
                assert_eq!(u.provider_content_blocks[0]["type"], "text");
                assert_eq!(u.provider_content_blocks[1]["type"], "text");
                assert_eq!(u.provider_content_blocks[1]["text"], "[image]");
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn strip_media_replaces_document_blocks() {
        use serde_json::json;

        let msgs = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: "read this pdf".into(),
            attachments: Vec::new(),
            provider_content_blocks: vec![
                json!({"type": "text", "text": "read this pdf"}),
                json!({"type": "document", "source": {"type": "base64", "data": "pdfdata"}}),
            ],
            summarize_metadata: None,
        })];
        let stripped = strip_media_from_messages(&msgs);
        match &stripped[0] {
            Message::User(u) => {
                assert_eq!(u.provider_content_blocks[1]["text"], "[document]");
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn strip_media_handles_nested_tool_result() {
        use serde_json::json;

        let msgs = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: String::new(),
            attachments: Vec::new(),
            provider_content_blocks: vec![json!({
                "type": "tool_result",
                "tool_use_id": "call-1",
                "content": [
                    {"type": "text", "text": "output"},
                    {"type": "image", "source": {"type": "base64", "data": "xyz"}},
                ]
            })],
            summarize_metadata: None,
        })];
        let stripped = strip_media_from_messages(&msgs);
        match &stripped[0] {
            Message::User(u) => {
                let content = &u.provider_content_blocks[0]["content"];
                let arr = content.as_array().expect("content should be array");
                assert_eq!(arr[0]["type"], "text");
                assert_eq!(arr[0]["text"], "output");
                assert_eq!(arr[1]["type"], "text");
                assert_eq!(arr[1]["text"], "[image]");
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn strip_media_preserves_non_user_messages() {
        use serde_json::json;

        let msgs = vec![
            Message::System(SystemMessage {
                base: MessageBase::default(),
                subtype: SystemMessageSubtype::Informational,
                text: "system msg".into(),
                error: None,
            }),
            Message::User(UserMessage {
                base: MessageBase::default(),
                text: "hello".into(),
                attachments: Vec::new(),
                provider_content_blocks: vec![json!({"type": "text", "text": "hello"})],
                summarize_metadata: None,
            }),
        ];
        let stripped = strip_media_from_messages(&msgs);
        assert!(matches!(stripped[0], Message::System(_)));
        // User message without media should be unchanged
        match &stripped[1] {
            Message::User(u) => {
                assert_eq!(u.provider_content_blocks[0]["text"], "hello");
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn strip_media_clears_attachments() {
        use claude_core::{Attachment, AttachmentMediaType};

        let msgs = vec![Message::User(UserMessage {
            base: MessageBase::default(),
            text: "see pic".into(),
            attachments: vec![Attachment {
                media_type: AttachmentMediaType::ImagePng,
                data: "base64data".into(),
                filename: None,
            }],
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })];
        // Materialize and check it would have had image blocks
        let blocks = match &msgs[0] {
            Message::User(u) => u.provider_content_blocks(),
            _ => panic!(),
        };
        assert!(blocks.iter().any(|b| b["type"] == "image"));

        let stripped = strip_media_from_messages(&msgs);
        match &stripped[0] {
            Message::User(u) => {
                assert!(u.attachments.is_empty());
                assert!(
                    u.provider_content_blocks
                        .iter()
                        .all(|b| b["type"] != "image")
                );
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn strip_skill_attachments_filters_skill_discovery() {
        use claude_core::{AttachmentMessage, MessageBase};

        let msgs = vec![
            Message::User(UserMessage {
                base: MessageBase::default(),
                text: "hello".into(),
                attachments: Vec::new(),
                provider_content_blocks: Vec::new(),
                summarize_metadata: None,
            }),
            Message::Attachment(AttachmentMessage {
                base: MessageBase::default(),
                label: Some("skill_discovery".into()),
                attachments: Vec::new(),
            }),
            Message::Attachment(AttachmentMessage {
                base: MessageBase::default(),
                label: Some("skill_listing".into()),
                attachments: Vec::new(),
            }),
            Message::Attachment(AttachmentMessage {
                base: MessageBase::default(),
                label: Some("file_state".into()),
                attachments: Vec::new(),
            }),
        ];

        let stripped = strip_media_from_messages_ex(&msgs, true);
        assert_eq!(
            stripped.len(),
            2,
            "skill_discovery and skill_listing should be removed"
        );
        assert!(matches!(&stripped[0], Message::User(_)));
        assert!(
            matches!(&stripped[1], Message::Attachment(a) if a.label.as_deref() == Some("file_state")),
            "non-skill attachments should be preserved"
        );
    }

    #[test]
    fn strip_skill_attachments_noop_when_false() {
        use claude_core::{AttachmentMessage, MessageBase};

        let msgs = vec![Message::Attachment(AttachmentMessage {
            base: MessageBase::default(),
            label: Some("skill_discovery".into()),
            attachments: Vec::new(),
        })];

        let stripped = strip_media_from_messages_ex(&msgs, false);
        assert_eq!(
            stripped.len(),
            1,
            "strip_skill_attachments=false should preserve all"
        );
    }

    // -- Boundary metadata tests ------------------------------------------------

    #[test]
    fn create_compact_boundary_includes_last_pre_compact_uuid() {
        let uuid = uuid::Uuid::new_v4();
        let boundary = create_compact_boundary_message("manual", 5000, Some(uuid), None);
        match &boundary {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                assert_eq!(meta["trigger"], "manual");
                assert_eq!(meta["preTokens"], 5000);
                assert_eq!(meta["lastPreCompactMessageUuid"], uuid.to_string());
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn create_compact_boundary_without_uuid() {
        let boundary = create_compact_boundary_message("auto", 1000, None, None);
        match &boundary {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                assert_eq!(meta["trigger"], "auto");
                assert_eq!(meta["preTokens"], 1000);
                assert!(meta.get("lastPreCompactMessageUuid").is_none());
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn annotate_boundary_adds_preserved_segment() {
        let boundary = create_compact_boundary_message("manual", 5000, None, None);
        let head = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "head".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let tail = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "tail".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        let anchor_uuid = uuid::Uuid::new_v4();
        let annotated = annotate_boundary_with_preserved_segment(
            &boundary,
            anchor_uuid,
            &[head.clone(), tail.clone()],
        );
        match &annotated {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                let seg = &meta["preservedSegment"];
                assert_eq!(seg["headUuid"], head.uuid().to_string());
                assert_eq!(seg["anchorUuid"], anchor_uuid.to_string());
                assert_eq!(seg["tailUuid"], tail.uuid().to_string());
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn annotate_boundary_noop_for_empty_keep() {
        let boundary = create_compact_boundary_message("manual", 5000, None, None);
        let annotated =
            annotate_boundary_with_preserved_segment(&boundary, uuid::Uuid::new_v4(), &[]);
        match (&boundary, &annotated) {
            (Message::System(a), Message::System(b)) => assert_eq!(a.text, b.text),
            _ => panic!("unexpected message types"),
        }
    }

    #[test]
    fn enrich_boundary_metadata_adds_fields() {
        let boundary = create_compact_boundary_message("manual", 5000, None, None);
        let enriched = enrich_boundary_metadata(&boundary, |meta| {
            meta["userContext"] = serde_json::Value::String("test feedback".into());
            meta["messagesSummarized"] = serde_json::json!(42);
        });
        match &enriched {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                assert_eq!(meta["trigger"], "manual");
                assert_eq!(meta["userContext"], "test feedback");
                assert_eq!(meta["messagesSummarized"], 42);
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    // -- CompactSessionState (circuit breaker) tests ---------------------------

    #[test]
    fn session_state_new_is_clean() {
        let state = CompactSessionState::new();
        assert_eq!(state.consecutive_compact_failures, 0);
        assert!(!state.auto_compact_disabled);
        assert!(state.is_auto_compact_allowed());
    }

    #[test]
    fn session_state_record_success_resets() {
        let mut state = CompactSessionState::new();
        state.record_failure();
        state.record_failure();
        assert_eq!(state.consecutive_compact_failures, 2);
        state.record_success();
        assert_eq!(state.consecutive_compact_failures, 0);
        assert!(state.is_auto_compact_allowed());
    }

    #[test]
    fn session_state_circuit_breaker_trips_at_three() {
        let mut state = CompactSessionState::new();
        state.record_failure();
        assert!(state.is_auto_compact_allowed());
        state.record_failure();
        assert!(state.is_auto_compact_allowed());
        state.record_failure();
        assert!(!state.is_auto_compact_allowed());
        assert!(state.auto_compact_disabled);
    }

    #[test]
    fn session_state_circuit_breaker_stays_tripped() {
        let mut state = CompactSessionState::new();
        for _ in 0..3 {
            state.record_failure();
        }
        assert!(!state.is_auto_compact_allowed());
        // Even after success, the disabled flag is still set
        state.record_success();
        assert_eq!(state.consecutive_compact_failures, 0);
        assert!(state.auto_compact_disabled);
    }

    // -- preCompactDiscoveredTools in boundary marker tests --------------------

    #[test]
    fn boundary_marker_includes_discovered_tools() {
        let tools = vec!["Read".to_string(), "Write".to_string(), "Bash".to_string()];
        let boundary = create_compact_boundary_message("manual", 5000, None, Some(&tools));
        match &boundary {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                let arr = meta["preCompactDiscoveredTools"]
                    .as_array()
                    .expect("discovered tools should be an array");
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], "Read");
                assert_eq!(arr[1], "Write");
                assert_eq!(arr[2], "Bash");
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn boundary_marker_omits_discovered_tools_when_empty() {
        let tools: Vec<String> = vec![];
        let boundary = create_compact_boundary_message("manual", 5000, None, Some(&tools));
        match &boundary {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                assert!(meta.get("preCompactDiscoveredTools").is_none());
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn boundary_marker_omits_discovered_tools_when_none() {
        let boundary = create_compact_boundary_message("manual", 5000, None, None);
        match &boundary {
            Message::System(s) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&s.text).expect("boundary metadata should be JSON");
                assert!(meta.get("preCompactDiscoveredTools").is_none());
            }
            other => panic!("expected System, got {other:?}"),
        }
    }
}
