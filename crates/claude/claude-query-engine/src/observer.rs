use anyhow::Result;
use async_trait::async_trait;
use claude_core::{ConversationEntry, Message, SessionId, ToolCall, ToolResult};
use rc_engine_events::Usage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Checkpoint categories surfaced by the compat engine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryCheckpointKind {
    ResumeBoundary,
    ToolBatch,
}

/// Durable checkpoint marker for host adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryCheckpoint {
    pub kind: QueryCheckpointKind,
    pub session_id: SessionId,
    pub turn: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<Uuid>,
    #[serde(default)]
    pub tool_use_ids: Vec<String>,
    #[serde(default)]
    pub message_count: usize,
}

impl QueryCheckpoint {
    #[must_use]
    pub fn new(
        kind: QueryCheckpointKind,
        session_id: SessionId,
        turn: u32,
        assistant_message_id: Option<Uuid>,
        tool_use_ids: Vec<String>,
        message_count: usize,
    ) -> Self {
        Self {
            kind,
            session_id,
            turn,
            assistant_message_id,
            tool_use_ids,
            message_count,
        }
    }
}

/// Budget status exposed to host observers before each provider round.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryBudgetState {
    pub turn: u32,
    pub total_tokens: u64,
    pub max_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
}

/// Context-window snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryContextBudgetState {
    pub estimated_tokens: u64,
    pub max_input_tokens: u64,
    pub threshold_tokens: u64,
    pub usage_ratio: f64,
    pub needs_compaction: bool,
}

/// Structured result event — mirrors TS `result` SDK message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryResultEvent {
    /// Subtype: success, error_max_turns, error_max_budget_usd,
    /// error_during_execution, error_max_structured_output_retries.
    pub subtype: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u32,
    pub result: Option<String>,
    pub stop_reason: Option<String>,
    pub session_id: SessionId,
    #[serde(default)]
    pub total_cost_usd: f64,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_usage: Option<serde_json::Value>,
    #[serde(default)]
    pub permission_denials: Vec<serde_json::Value>,
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    #[serde(default)]
    pub fast_mode_state: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<String>,
    pub uuid: Uuid,
}

/// API retry notification — mirrors TS `system/api_retry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRetryEvent {
    pub attempt: u32,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub error_status: Option<u32>,
    pub error: String,
    pub session_id: SessionId,
    pub uuid: Uuid,
}

/// Tool use summary — mirrors TS `tool_use_summary` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseSummaryEvent {
    pub summary: String,
    pub preceding_tool_use_ids: Vec<String>,
    pub session_id: SessionId,
    pub uuid: Uuid,
}

/// Progress event — mirrors TS `progress` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub tool_use_id: String,
    pub stage: String,
    pub status: String,
    pub turn: u32,
    pub session_id: SessionId,
    pub uuid: Uuid,
}

/// Attachment event — mirrors TS `attachment` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "attachment_type", rename_all = "snake_case")]
pub enum AttachmentEvent {
    /// Structured output from a synthetic-output tool.
    StructuredOutput {
        data: serde_json::Value,
        session_id: SessionId,
        uuid: Uuid,
    },
    /// Max turns reached signal.
    MaxTurnsReached {
        max_turns: u32,
        turn_count: u32,
        session_id: SessionId,
        uuid: Uuid,
    },
    /// Tool-specific attachment (e.g., edited_text_file).
    ToolAttachment {
        label: Option<String>,
        tool_use_id: String,
        session_id: SessionId,
        uuid: Uuid,
    },
    /// Hook stopped continuation.
    HookStoppedContinuation {
        message: String,
        hook_name: String,
        tool_use_id: String,
        session_id: SessionId,
        uuid: Uuid,
    },
}

/// Token budget continuation event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetContinuationEvent {
    pub nudge_message: String,
    pub continuation_count: usize,
    pub pct: u32,
    pub turn_tokens: u64,
    pub budget: u64,
    pub session_id: SessionId,
}

/// Stop-hook blocking error event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookBlockingEvent {
    pub blocking_errors_count: usize,
    pub turn: u32,
    pub session_id: SessionId,
}

/// Local observer event surface for host-side compat adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryObserverEvent {
    // -- lifecycle --
    QueryStarted {
        session_id: SessionId,
        existing_messages: usize,
        new_messages: usize,
    },
    MessagesAppended {
        session_id: SessionId,
        appended: Vec<Message>,
        total_messages: usize,
    },
    QueryFinished {
        stop_reason: String,
        turns: u32,
        final_text: Option<String>,
        usage: Usage,
    },
    QueryFailed {
        error: String,
        turns: u32,
        consecutive_failures: usize,
        usage: Usage,
    },

    // -- result (mirrors TS `result` message) --
    QueryResult {
        result: QueryResultEvent,
    },

    // -- budget --
    BudgetEvaluated {
        budget: QueryBudgetState,
    },
    BudgetExceeded {
        budget: QueryBudgetState,
        reason: String,
    },
    /// Token budget continuation was issued.
    TokenBudgetContinuation {
        event: TokenBudgetContinuationEvent,
    },

    // -- context / compaction --
    ContextBudgetEvaluated {
        turn: u32,
        context: QueryContextBudgetState,
        message_count: usize,
    },
    ContextCompactionApplied {
        turn: u32,
        before_messages: usize,
        after_messages: usize,
        compacted_conversation: Vec<ConversationEntry>,
        max_input_tokens: u64,
        threshold_tokens: u64,
        usage_ratio_before: f64,
        usage_ratio_after: f64,
        estimated_tokens_before: u64,
        estimated_tokens_after: u64,
    },
    /// Reactive compact was applied.
    ReactiveCompactApplied {
        turn: u32,
        before_messages: usize,
        after_messages: usize,
    },

    // -- streaming --
    StreamingTextDelta {
        turn: u32,
        delta: String,
        accumulated_text: String,
    },
    StreamingToolCallStarted {
        turn: u32,
        tool_call_id: String,
        tool_name: String,
    },
    StreamingToolCallDelta {
        turn: u32,
        tool_call_id: String,
        delta: String,
    },
    StreamingUsageUpdated {
        turn: u32,
        usage: Usage,
    },
    StreamingThinkingDelta {
        turn: u32,
        delta: String,
        accumulated_thinking: String,
    },

    // -- assistant / response --
    AssistantMessageCommitted {
        message: Message,
        stop_reason: String,
        turn: u32,
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    // -- tool execution --
    ToolCallStarted {
        tool_call: ToolCall,
        turn: u32,
        batch_size: usize,
        batch_index: usize,
    },
    ToolResultCommitted {
        tool_call: ToolCall,
        result: ToolResult,
        turn: u32,
        total_messages: usize,
    },

    // -- tool use summary --
    ToolUseSummary {
        event: ToolUseSummaryEvent,
    },

    // -- progress --
    Progress {
        event: ProgressEvent,
    },

    // -- attachments --
    Attachment {
        event: AttachmentEvent,
    },

    // -- API retry --
    ApiRetry {
        event: ApiRetryEvent,
    },

    // -- stop hooks --
    StopHookBlocking {
        event: StopHookBlockingEvent,
    },
    /// Stop hook prevented query continuation.
    StopHookPrevented {
        reason: String,
        turn: u32,
        session_id: SessionId,
    },

    // -- checkpoints --
    CheckpointCreated {
        checkpoint: QueryCheckpoint,
    },
    CheckpointCleared {
        checkpoint: QueryCheckpoint,
    },

    // -- recovery transitions --
    MaxTokensEscalate {
        turn: u32,
        from_max_tokens: usize,
        to_max_tokens: usize,
    },
    MaxTokensRecovery {
        turn: u32,
        attempt: usize,
        max_tokens: usize,
    },
    ModelFallbackTriggered {
        original_model: String,
        fallback_model: String,
        turn: u32,
    },
    /// Collapse drain retry (context collapse drained before retry).
    CollapseDrainRetry {
        turn: u32,
        committed: usize,
    },
    /// Reactive compact retry.
    ReactiveCompactRetry {
        turn: u32,
    },
    /// Max-output-tokens recovery exhausted.
    MaxTokensRecoveryExhausted {
        turn: u32,
    },
    /// Image error recovery — problematic image blocks stripped.
    ImageErrorRecovery {
        turn: u32,
        images_stripped: usize,
    },
    /// Media size error recovery — oversized media blocks stripped.
    MediaSizeErrorRecovery {
        turn: u32,
        media_blocks_stripped: usize,
    },
    /// Context collapse recovery for HTTP 413 "request too large".
    ContextCollapseRecovery {
        turn: u32,
        before_messages: usize,
        after_messages: usize,
    },
}

impl QueryObserverEvent {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::QueryStarted { .. } => "query_started",
            Self::MessagesAppended { .. } => "messages_appended",
            Self::QueryFinished { .. } => "query_finished",
            Self::QueryFailed { .. } => "query_failed",
            Self::QueryResult { .. } => "query_result",
            Self::BudgetEvaluated { .. } => "budget_evaluated",
            Self::BudgetExceeded { .. } => "budget_exceeded",
            Self::TokenBudgetContinuation { .. } => "token_budget_continuation",
            Self::ContextBudgetEvaluated { .. } => "context_budget_evaluated",
            Self::ContextCompactionApplied { .. } => "context_compaction_applied",
            Self::ReactiveCompactApplied { .. } => "reactive_compact_applied",
            Self::StreamingTextDelta { .. } => "streaming_text_delta",
            Self::StreamingToolCallStarted { .. } => "streaming_tool_call_started",
            Self::StreamingToolCallDelta { .. } => "streaming_tool_call_delta",
            Self::StreamingUsageUpdated { .. } => "streaming_usage_updated",
            Self::StreamingThinkingDelta { .. } => "streaming_thinking_delta",
            Self::AssistantMessageCommitted { .. } => "assistant_message_committed",
            Self::ToolCallStarted { .. } => "tool_call_started",
            Self::ToolResultCommitted { .. } => "tool_result_committed",
            Self::ToolUseSummary { .. } => "tool_use_summary",
            Self::Progress { .. } => "progress",
            Self::Attachment { .. } => "attachment",
            Self::ApiRetry { .. } => "api_retry",
            Self::StopHookBlocking { .. } => "stop_hook_blocking",
            Self::StopHookPrevented { .. } => "stop_hook_prevented",
            Self::CheckpointCreated { .. } => "checkpoint_created",
            Self::CheckpointCleared { .. } => "checkpoint_cleared",
            Self::MaxTokensEscalate { .. } => "max_tokens_escalate",
            Self::MaxTokensRecovery { .. } => "max_tokens_recovery",
            Self::ModelFallbackTriggered { .. } => "model_fallback_triggered",
            Self::CollapseDrainRetry { .. } => "collapse_drain_retry",
            Self::ReactiveCompactRetry { .. } => "reactive_compact_retry",
            Self::MaxTokensRecoveryExhausted { .. } => "max_tokens_recovery_exhausted",
            Self::ImageErrorRecovery { .. } => "image_error_recovery",
            Self::MediaSizeErrorRecovery { .. } => "media_size_error_recovery",
            Self::ContextCollapseRecovery { .. } => "context_collapse_recovery",
        }
    }
}

/// Observer seam for compat adapters.
#[async_trait]
pub trait QueryObserver: Send + Sync {
    async fn on_event(&self, event: QueryObserverEvent) -> Result<()>;
}

pub type QueryStreamingEventSink = Arc<dyn Fn(QueryStreamingEvent) + Send + Sync>;

/// Sync-friendly streaming event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryStreamingEvent {
    AssistantMessageDelta {
        delta: String,
    },
    ThinkingDelta {
        delta: String,
    },
    ToolCallStarted {
        tool_call_id: String,
        tool_name: String,
    },
    ToolCallDelta {
        tool_call_id: String,
        delta: String,
    },
}

/// Default no-op observer.
#[derive(Debug, Default)]
pub struct NoopQueryObserver;

#[async_trait]
impl QueryObserver for NoopQueryObserver {
    async fn on_event(&self, _event: QueryObserverEvent) -> Result<()> {
        Ok(())
    }
}
