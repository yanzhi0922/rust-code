//! Enhanced message types for the v2 runtime surface.
//!
//! This module defines [`NormalizedMessage`] — a unified, flat message type
//! that can represent any message flowing through the system. It provides
//! a consistent interface regardless of the original message source.
//!
//! # Message Origins
//!
//! Every normalized message carries a [`NormalizedOrigin`] indicating who
//! produced it: user, assistant, system, or tool.
//!
//! # Message Types (24 variants)
//!
//! The normalized message enum covers all event types in the system:
//! user input, assistant responses, tool calls/results, system notifications,
//! permission flows, agent lifecycle, cost tracking, and streaming events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ConversationEntry;

// ---------------------------------------------------------------------------
// NormalizedOrigin — who produced the message
// ---------------------------------------------------------------------------

/// Origin of a normalized message.
///
/// Simplified from the full [`crate::MessageOrigin`] to four categories
/// that map to the Claude Code parity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedOrigin {
    /// User input.
    User,
    /// Assistant / model response.
    Assistant,
    /// System-generated message.
    System,
    /// Tool execution output.
    Tool,
}

impl NormalizedOrigin {
    /// Return a lowercase string label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }
}

// ---------------------------------------------------------------------------
// NormalizedMessage — unified message format
// ---------------------------------------------------------------------------

/// A unified, flat message type representing any event in the system.
///
/// This is the canonical format for cross-subsystem communication,
/// logging, and persistence. Each variant captures the essential data
/// for its event type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NormalizedMessage {
    // ── User messages ────────────────────────────────────────────────
    /// User submitted text input.
    UserText {
        /// Unique message identifier.
        id: String,
        /// The user's text.
        text: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// User submitted an attachment (image, PDF, etc.).
    UserAttachment {
        /// Unique message identifier.
        id: String,
        /// Attachment label or filename.
        label: String,
        /// Number of attachments.
        count: usize,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Assistant messages ───────────────────────────────────────────
    /// Assistant generated text (streaming delta or final).
    AssistantText {
        /// Unique message identifier.
        id: String,
        /// The assistant's text content.
        text: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Model thinking/reasoning content.
    Thinking {
        /// Unique message identifier.
        id: String,
        /// Thinking content.
        content: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Tool messages ────────────────────────────────────────────────
    /// A tool call was initiated.
    ToolCall {
        /// Unique message identifier.
        id: String,
        /// Tool call ID from the provider.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Tool input parameters (JSON).
        input: serde_json::Value,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// A tool execution completed.
    ToolResult {
        /// Unique message identifier.
        id: String,
        /// Tool call ID this result belongs to.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Tool output content.
        output: String,
        /// Whether the execution failed.
        #[serde(default)]
        is_error: bool,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Summary of a tool invocation (post-compaction).
    ToolSummary {
        /// Unique message identifier.
        id: String,
        /// Tool call ID.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Summary text.
        summary: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── System messages ──────────────────────────────────────────────
    /// Informational system message.
    SystemInfo {
        /// Unique message identifier.
        id: String,
        /// Message text.
        text: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Error system message.
    SystemError {
        /// Unique message identifier.
        id: String,
        /// Error message.
        text: String,
        /// Error details.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Compact boundary marker (indicates a compaction occurred).
    SystemCompactBoundary {
        /// Unique message identifier.
        id: String,
        /// Summary of what was compacted.
        summary: String,
        /// Number of messages removed.
        messages_removed: usize,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Micro-compact boundary marker.
    SystemMicroCompactBoundary {
        /// Unique message identifier.
        id: String,
        /// Description of what was micro-compacted.
        description: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Lifecycle messages ───────────────────────────────────────────
    /// Progress update while work is underway.
    Progress {
        /// Unique message identifier.
        id: String,
        /// Progress stage (e.g., "thinking", "executing").
        stage: String,
        /// Status message.
        status: String,
        /// Optional percentage (0-100).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// A message that was replaced (tombstone).
    Tombstone {
        /// Unique message identifier.
        id: String,
        /// IDs of replaced messages.
        replaced_ids: Vec<String>,
        /// Summary of the replaced content.
        summary: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Result of a hook execution.
    HookResult {
        /// Unique message identifier.
        id: String,
        /// Hook name.
        hook_name: String,
        /// Hook output.
        output: String,
        /// Whether the hook failed.
        #[serde(default)]
        is_error: bool,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Permission messages ──────────────────────────────────────────
    /// Permission request pending approval.
    PermissionRequest {
        /// Unique message identifier.
        id: String,
        /// Request ID.
        request_id: String,
        /// Tool name.
        tool_name: String,
        /// Action description.
        description: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Permission decision rendered.
    PermissionDecision {
        /// Unique message identifier.
        id: String,
        /// Request ID.
        request_id: String,
        /// Whether allowed.
        allowed: bool,
        /// Optional reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Status messages ──────────────────────────────────────────────
    /// Status update.
    StatusUpdate {
        /// Unique message identifier.
        id: String,
        /// Status message.
        message: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Agent messages ───────────────────────────────────────────────
    /// A sub-agent was dispatched.
    AgentDispatched {
        /// Unique message identifier.
        id: String,
        /// Agent identifier.
        agent_id: String,
        /// Task description.
        task: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// A sub-agent completed.
    AgentComplete {
        /// Unique message identifier.
        id: String,
        /// Agent identifier.
        agent_id: String,
        /// Whether successful.
        success: bool,
        /// Result summary.
        summary: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Subtask messages ─────────────────────────────────────────────
    /// A subtask started.
    SubtaskStarted {
        /// Unique message identifier.
        id: String,
        /// Task identifier.
        task_id: String,
        /// Parent task identifier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_task_id: Option<String>,
        /// Task description.
        description: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// A subtask made progress.
    SubtaskProgress {
        /// Unique message identifier.
        id: String,
        /// Task identifier.
        task_id: String,
        /// Current turn.
        turn: u32,
        /// Maximum turns.
        max_turns: u32,
        /// Summary of progress.
        summary: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// A subtask completed.
    SubtaskCompleted {
        /// Unique message identifier.
        id: String,
        /// Task identifier.
        task_id: String,
        /// Whether successful.
        success: bool,
        /// Output preview.
        output_preview: String,
        /// Turns used.
        turns_used: u32,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Tracking messages ────────────────────────────────────────────
    /// Cost tracking update.
    CostUpdate {
        /// Unique message identifier.
        id: String,
        /// Turn cost in USD.
        turn_cost_usd: f64,
        /// Total session cost in USD.
        total_cost_usd: f64,
        /// Input tokens.
        input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Context usage update.
    ContextUsage {
        /// Unique message identifier.
        id: String,
        /// Usage ratio (0.0-1.0).
        ratio: f64,
        /// Estimated token count.
        estimated_tokens: u64,
        /// Maximum context tokens.
        max_tokens: u64,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },

    // ── Streaming messages ───────────────────────────────────────────
    /// Streaming started.
    StreamStart {
        /// Unique message identifier.
        id: String,
        /// Provider protocol.
        protocol: String,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
    /// Streaming ended.
    StreamEnd {
        /// Unique message identifier.
        id: String,
        /// Total chunks received.
        chunks: u64,
        /// Duration in milliseconds.
        duration_ms: u64,
        /// When the message was created.
        timestamp: DateTime<Utc>,
    },
}

// ---------------------------------------------------------------------------
// Standalone message-type structs (Claude Code parity)
// ---------------------------------------------------------------------------

/// Thinking block content emitted by the model during extended reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemThinkingMessage {
    /// Unique message identifier.
    pub id: String,
    /// The thinking/reasoning content.
    pub content: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Compact boundary marker — inserted after a full or partial compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemCompactBoundaryMessage {
    /// Unique message identifier.
    pub id: String,
    /// Summary of what was compacted.
    pub summary: String,
    /// Number of messages removed during compaction.
    pub messages_removed: usize,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Microcompact boundary marker — inserted after a micro-compaction pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMicrocompactBoundaryMessage {
    /// Unique message identifier.
    pub id: String,
    /// Description of what was micro-compacted.
    pub description: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Permission retry notification — indicates a permission prompt was re-shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPermissionRetryMessage {
    /// Unique message identifier.
    pub id: String,
    /// The tool that required permission.
    pub tool_name: String,
    /// Reason for the retry.
    pub reason: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Memory saved notification — confirms that session memory was persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMemorySavedMessage {
    /// Unique message identifier.
    pub id: String,
    /// Memory topic files written by the extraction agent.
    pub written_paths: Vec<String>,
    /// Number of saved paths that belong to shared team memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_count: Option<usize>,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Stop hook summary — output from a stop-hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStopHookSummaryMessage {
    /// Unique message identifier.
    pub id: String,
    /// Name of the stop hook that ran.
    pub hook_name: String,
    /// Hook execution output.
    pub output: String,
    /// Whether the hook reported an error.
    #[serde(default)]
    pub is_error: bool,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Away summary — summarises activity while the user was away.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAwaySummaryMessage {
    /// Unique message identifier.
    pub id: String,
    /// Summary of what happened while away.
    pub summary: String,
    /// Number of messages that occurred.
    pub message_count: usize,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Agents killed notification — one or more sub-agents were terminated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAgentsKilledMessage {
    /// Unique message identifier.
    pub id: String,
    /// IDs of the killed agents.
    pub agent_ids: Vec<String>,
    /// Reason for termination.
    pub reason: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// API metrics — token usage and latency statistics for an API call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemApiMetricsMessage {
    /// Unique message identifier.
    pub id: String,
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens produced.
    pub output_tokens: u64,
    /// Latency in milliseconds.
    pub latency_ms: u64,
    /// Model identifier used.
    pub model: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// API error — an error returned by the model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAPIErrorMessage {
    /// Unique message identifier.
    pub id: String,
    /// Error code or category.
    pub error_code: String,
    /// Human-readable error message.
    pub error_message: String,
    /// Whether the error is retryable.
    #[serde(default)]
    pub retryable: bool,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// File snapshot — records the state of a file at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemFileSnapshotMessage {
    /// Unique message identifier.
    pub id: String,
    /// File path (relative to workspace).
    pub path: String,
    /// Hash of the file contents.
    pub content_hash: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Hook result — output from a hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResultMessageType {
    /// Unique message identifier.
    pub id: String,
    /// Hook name.
    pub hook_name: String,
    /// Hook output.
    pub output: String,
    /// Whether the hook failed.
    #[serde(default)]
    pub is_error: bool,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Tool use summary — condensed representation of a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseSummaryMessageType {
    /// Unique message identifier.
    pub id: String,
    /// Tool call ID.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Summary text.
    pub summary: String,
    /// Whether the tool invocation resulted in an error.
    #[serde(default)]
    pub is_error: bool,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Tombstone — marks a deleted/replaced message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TombstoneMessageType {
    /// Unique message identifier.
    pub id: String,
    /// IDs of the replaced messages.
    pub replaced_ids: Vec<String>,
    /// Summary of the replaced content.
    pub summary: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Progress indicator — shows ongoing work status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressMessageType {
    /// Unique message identifier.
    pub id: String,
    /// Progress stage.
    pub stage: String,
    /// Status message.
    pub status: String,
    /// Optional percentage (0-100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Attachment — a file attached to a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMessageType {
    /// Unique message identifier.
    pub id: String,
    /// File path of the attachment.
    pub path: String,
    /// MIME type of the attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

impl NormalizedMessage {
    /// Generate a new unique ID.
    fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Create a user text message.
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::UserText {
            id: Self::new_id(),
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    /// Create an assistant text message.
    #[must_use]
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self::AssistantText {
            id: Self::new_id(),
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    /// Create a tool result message.
    #[must_use]
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            id: Self::new_id(),
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output: output.into(),
            is_error,
            timestamp: Utc::now(),
        }
    }

    /// Create a system info message.
    #[must_use]
    pub fn system_info(text: impl Into<String>) -> Self {
        Self::SystemInfo {
            id: Self::new_id(),
            text: text.into(),
            timestamp: Utc::now(),
        }
    }

    /// Create a system error message.
    #[must_use]
    pub fn system_error(text: impl Into<String>, error: Option<String>) -> Self {
        Self::SystemError {
            id: Self::new_id(),
            text: text.into(),
            error,
            timestamp: Utc::now(),
        }
    }

    /// Create a compact boundary message.
    #[must_use]
    pub fn compact_boundary(summary: impl Into<String>, messages_removed: usize) -> Self {
        Self::SystemCompactBoundary {
            id: Self::new_id(),
            summary: summary.into(),
            messages_removed,
            timestamp: Utc::now(),
        }
    }

    /// Create a progress message.
    #[must_use]
    pub fn progress(stage: impl Into<String>, status: impl Into<String>) -> Self {
        Self::Progress {
            id: Self::new_id(),
            stage: stage.into(),
            status: status.into(),
            percent: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a status update message.
    #[must_use]
    pub fn status_update(message: impl Into<String>) -> Self {
        Self::StatusUpdate {
            id: Self::new_id(),
            message: message.into(),
            timestamp: Utc::now(),
        }
    }

    /// Return the origin of this message.
    #[must_use]
    pub fn origin(&self) -> NormalizedOrigin {
        match self {
            Self::UserText { .. } | Self::UserAttachment { .. } => NormalizedOrigin::User,
            Self::AssistantText { .. } | Self::Thinking { .. } => NormalizedOrigin::Assistant,
            Self::ToolCall { .. } | Self::ToolResult { .. } | Self::ToolSummary { .. } => {
                NormalizedOrigin::Tool
            }
            Self::SystemInfo { .. }
            | Self::SystemError { .. }
            | Self::SystemCompactBoundary { .. }
            | Self::SystemMicroCompactBoundary { .. }
            | Self::Progress { .. }
            | Self::Tombstone { .. }
            | Self::HookResult { .. }
            | Self::StatusUpdate { .. } => NormalizedOrigin::System,
            Self::PermissionRequest { .. } | Self::PermissionDecision { .. } => {
                NormalizedOrigin::System
            }
            Self::AgentDispatched { .. } | Self::AgentComplete { .. } => NormalizedOrigin::System,
            Self::SubtaskStarted { .. }
            | Self::SubtaskProgress { .. }
            | Self::SubtaskCompleted { .. } => NormalizedOrigin::System,
            Self::CostUpdate { .. } | Self::ContextUsage { .. } => NormalizedOrigin::System,
            Self::StreamStart { .. } | Self::StreamEnd { .. } => NormalizedOrigin::System,
        }
    }

    /// Return the message ID.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::UserText { id, .. }
            | Self::UserAttachment { id, .. }
            | Self::AssistantText { id, .. }
            | Self::Thinking { id, .. }
            | Self::ToolCall { id, .. }
            | Self::ToolResult { id, .. }
            | Self::ToolSummary { id, .. }
            | Self::SystemInfo { id, .. }
            | Self::SystemError { id, .. }
            | Self::SystemCompactBoundary { id, .. }
            | Self::SystemMicroCompactBoundary { id, .. }
            | Self::Progress { id, .. }
            | Self::Tombstone { id, .. }
            | Self::HookResult { id, .. }
            | Self::PermissionRequest { id, .. }
            | Self::PermissionDecision { id, .. }
            | Self::StatusUpdate { id, .. }
            | Self::AgentDispatched { id, .. }
            | Self::AgentComplete { id, .. }
            | Self::SubtaskStarted { id, .. }
            | Self::SubtaskProgress { id, .. }
            | Self::SubtaskCompleted { id, .. }
            | Self::CostUpdate { id, .. }
            | Self::ContextUsage { id, .. }
            | Self::StreamStart { id, .. }
            | Self::StreamEnd { id, .. } => id,
        }
    }

    /// Return the timestamp of this message.
    #[must_use]
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::UserText { timestamp, .. }
            | Self::UserAttachment { timestamp, .. }
            | Self::AssistantText { timestamp, .. }
            | Self::Thinking { timestamp, .. }
            | Self::ToolCall { timestamp, .. }
            | Self::ToolResult { timestamp, .. }
            | Self::ToolSummary { timestamp, .. }
            | Self::SystemInfo { timestamp, .. }
            | Self::SystemError { timestamp, .. }
            | Self::SystemCompactBoundary { timestamp, .. }
            | Self::SystemMicroCompactBoundary { timestamp, .. }
            | Self::Progress { timestamp, .. }
            | Self::Tombstone { timestamp, .. }
            | Self::HookResult { timestamp, .. }
            | Self::PermissionRequest { timestamp, .. }
            | Self::PermissionDecision { timestamp, .. }
            | Self::StatusUpdate { timestamp, .. }
            | Self::AgentDispatched { timestamp, .. }
            | Self::AgentComplete { timestamp, .. }
            | Self::SubtaskStarted { timestamp, .. }
            | Self::SubtaskProgress { timestamp, .. }
            | Self::SubtaskCompleted { timestamp, .. }
            | Self::CostUpdate { timestamp, .. }
            | Self::ContextUsage { timestamp, .. }
            | Self::StreamStart { timestamp, .. }
            | Self::StreamEnd { timestamp, .. } => *timestamp,
        }
    }

    /// Return a short summary of the message content.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::UserText { text, .. } => text.clone(),
            Self::UserAttachment { label, .. } => format!("[attachment: {label}]"),
            Self::AssistantText { text, .. } => text.clone(),
            Self::Thinking { content, .. } => format!("[thinking: {} chars]", content.len()),
            Self::ToolCall { tool_name, .. } => format!("[tool call: {tool_name}]"),
            Self::ToolResult {
                tool_name, output, ..
            } => {
                let truncated = if output.len() > 80 {
                    format!("{}...", &output[..76])
                } else {
                    output.clone()
                };
                format!("[tool result: {tool_name}] {truncated}")
            }
            Self::ToolSummary {
                tool_name, summary, ..
            } => format!("[tool summary: {tool_name}] {summary}"),
            Self::SystemInfo { text, .. } => text.clone(),
            Self::SystemError { text, .. } => format!("[error] {text}"),
            Self::SystemCompactBoundary {
                messages_removed, ..
            } => format!("[compact boundary: {messages_removed} messages removed]"),
            Self::SystemMicroCompactBoundary { description, .. } => {
                format!("[micro-compact: {description}]")
            }
            Self::Progress { stage, status, .. } => format!("[{stage}] {status}"),
            Self::Tombstone { summary, .. } => format!("[tombstone] {summary}"),
            Self::HookResult {
                hook_name, output, ..
            } => format!("[hook: {hook_name}] {output}"),
            Self::PermissionRequest {
                tool_name,
                description,
                ..
            } => format!("[permission: {tool_name}] {description}"),
            Self::PermissionDecision {
                allowed, reason, ..
            } => {
                let reason_text = reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default();
                format!(
                    "[permission: {}]{reason_text}",
                    if *allowed { "allowed" } else { "denied" }
                )
            }
            Self::StatusUpdate { message, .. } => message.clone(),
            Self::AgentDispatched { task, .. } => format!("[agent dispatched] {task}"),
            Self::AgentComplete {
                success, summary, ..
            } => {
                let status = if *success { "ok" } else { "failed" };
                format!("[agent complete: {status}] {summary}")
            }
            Self::SubtaskStarted { description, .. } => format!("[subtask started] {description}"),
            Self::SubtaskProgress {
                turn, max_turns, ..
            } => {
                format!("[subtask progress: turn {turn}/{max_turns}]")
            }
            Self::SubtaskCompleted {
                success,
                output_preview,
                ..
            } => {
                let status = if *success { "ok" } else { "failed" };
                format!("[subtask complete: {status}] {output_preview}")
            }
            Self::CostUpdate {
                turn_cost_usd,
                total_cost_usd,
                ..
            } => format!("[cost] turn=${turn_cost_usd:.4} total=${total_cost_usd:.4}"),
            Self::ContextUsage {
                ratio,
                estimated_tokens,
                ..
            } => format!(
                "[context] {:.0}% ({estimated_tokens} tokens)",
                ratio * 100.0
            ),
            Self::StreamStart { protocol, .. } => format!("[stream start: {protocol}]"),
            Self::StreamEnd {
                chunks,
                duration_ms,
                ..
            } => format!("[stream end: {chunks} chunks in {duration_ms}ms]"),
        }
    }

    /// Check if this message represents an error.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::SystemError { .. }
                | Self::ToolResult { is_error: true, .. }
                | Self::HookResult { is_error: true, .. }
        )
    }

    /// Convert from a legacy [`ConversationEntry`].
    #[must_use]
    pub fn from_conversation_entry(entry: &ConversationEntry) -> Option<Self> {
        match entry.role {
            crate::ConversationRole::User => Some(Self::user_text(&entry.text)),
            crate::ConversationRole::Assistant => Some(Self::assistant_text(&entry.text)),
            crate::ConversationRole::System => Some(Self::system_info(&entry.text)),
            crate::ConversationRole::Tool => {
                let name = entry.name.as_deref().unwrap_or("unknown");
                Some(Self::tool_result(
                    entry.tool_call_id.as_deref().unwrap_or("unknown"),
                    name,
                    &entry.text,
                    entry.is_error,
                ))
            }
        }
    }

    /// Count the total number of variants.
    #[must_use]
    pub fn variant_count() -> usize {
        24
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_origin_as_str() {
        assert_eq!(NormalizedOrigin::User.as_str(), "user");
        assert_eq!(NormalizedOrigin::Assistant.as_str(), "assistant");
        assert_eq!(NormalizedOrigin::System.as_str(), "system");
        assert_eq!(NormalizedOrigin::Tool.as_str(), "tool");
    }

    #[test]
    fn user_text_message_creation() {
        let msg = NormalizedMessage::user_text("hello world");
        assert_eq!(msg.origin(), NormalizedOrigin::User);
        assert!(!msg.id().is_empty());
        assert!(msg.summary().contains("hello world"));
        assert!(!msg.is_error());
    }

    #[test]
    fn assistant_text_message_creation() {
        let msg = NormalizedMessage::assistant_text("response text");
        assert_eq!(msg.origin(), NormalizedOrigin::Assistant);
        assert!(msg.summary().contains("response text"));
    }

    #[test]
    fn tool_result_message_creation() {
        let msg = NormalizedMessage::tool_result("tc-1", "bash", "output", false);
        assert_eq!(msg.origin(), NormalizedOrigin::Tool);
        assert!(!msg.is_error());

        let err_msg = NormalizedMessage::tool_result("tc-2", "bash", "error", true);
        assert!(err_msg.is_error());
    }

    #[test]
    fn system_info_message_creation() {
        let msg = NormalizedMessage::system_info("session started");
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.summary().contains("session started"));
        assert!(!msg.is_error());
    }

    #[test]
    fn system_error_message_creation() {
        let msg = NormalizedMessage::system_error("api failed", Some("timeout".to_owned()));
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.is_error());
        assert!(msg.summary().contains("api failed"));
    }

    #[test]
    fn compact_boundary_message() {
        let msg = NormalizedMessage::compact_boundary("compacted", 50);
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.summary().contains("50 messages removed"));
    }

    #[test]
    fn progress_message() {
        let msg = NormalizedMessage::progress("thinking", "processing...");
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.summary().contains("thinking"));
    }

    #[test]
    fn status_update_message() {
        let msg = NormalizedMessage::status_update("idle");
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.summary().contains("idle"));
    }

    #[test]
    fn tombstone_message() {
        let msg = NormalizedMessage::Tombstone {
            id: NormalizedMessage::new_id(),
            replaced_ids: vec!["msg-1".to_owned()],
            summary: "old content".to_owned(),
            timestamp: Utc::now(),
        };
        assert_eq!(msg.origin(), NormalizedOrigin::System);
        assert!(msg.summary().contains("tombstone"));
    }

    #[test]
    fn hook_result_message() {
        let msg = NormalizedMessage::HookResult {
            id: NormalizedMessage::new_id(),
            hook_name: "pre-commit".to_owned(),
            output: "passed".to_owned(),
            is_error: false,
            timestamp: Utc::now(),
        };
        assert!(!msg.is_error());

        let err_hook = NormalizedMessage::HookResult {
            id: NormalizedMessage::new_id(),
            hook_name: "pre-commit".to_owned(),
            output: "failed".to_owned(),
            is_error: true,
            timestamp: Utc::now(),
        };
        assert!(err_hook.is_error());
    }

    #[test]
    fn permission_messages() {
        let request = NormalizedMessage::PermissionRequest {
            id: NormalizedMessage::new_id(),
            request_id: "req-1".to_owned(),
            tool_name: "bash".to_owned(),
            description: "run command".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(request.summary().contains("bash"));

        let decision = NormalizedMessage::PermissionDecision {
            id: NormalizedMessage::new_id(),
            request_id: "req-1".to_owned(),
            allowed: true,
            reason: None,
            timestamp: Utc::now(),
        };
        assert!(decision.summary().contains("allowed"));
    }

    #[test]
    fn agent_messages() {
        let dispatched = NormalizedMessage::AgentDispatched {
            id: NormalizedMessage::new_id(),
            agent_id: "agent-1".to_owned(),
            task: "investigate".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(dispatched.summary().contains("investigate"));

        let complete = NormalizedMessage::AgentComplete {
            id: NormalizedMessage::new_id(),
            agent_id: "agent-1".to_owned(),
            success: true,
            summary: "done".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(complete.summary().contains("ok"));
    }

    #[test]
    fn subtask_messages() {
        let started = NormalizedMessage::SubtaskStarted {
            id: NormalizedMessage::new_id(),
            task_id: "task-1".to_owned(),
            parent_task_id: None,
            description: "fix bug".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(started.summary().contains("fix bug"));

        let progress = NormalizedMessage::SubtaskProgress {
            id: NormalizedMessage::new_id(),
            task_id: "task-1".to_owned(),
            turn: 3,
            max_turns: 10,
            summary: "working".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(progress.summary().contains("3/10"));

        let completed = NormalizedMessage::SubtaskCompleted {
            id: NormalizedMessage::new_id(),
            task_id: "task-1".to_owned(),
            success: true,
            output_preview: "fixed".to_owned(),
            turns_used: 5,
            timestamp: Utc::now(),
        };
        assert!(completed.summary().contains("fixed"));
    }

    #[test]
    fn cost_and_context_messages() {
        let cost = NormalizedMessage::CostUpdate {
            id: NormalizedMessage::new_id(),
            turn_cost_usd: 0.003,
            total_cost_usd: 0.015,
            input_tokens: 500,
            output_tokens: 200,
            timestamp: Utc::now(),
        };
        assert!(cost.summary().contains("$0.0030"));
        assert!(cost.summary().contains("$0.0150"));

        let ctx = NormalizedMessage::ContextUsage {
            id: NormalizedMessage::new_id(),
            ratio: 0.75,
            estimated_tokens: 150_000,
            max_tokens: 200_000,
            timestamp: Utc::now(),
        };
        assert!(ctx.summary().contains("150000"));
    }

    #[test]
    fn stream_messages() {
        let start = NormalizedMessage::StreamStart {
            id: NormalizedMessage::new_id(),
            protocol: "anthropic".to_owned(),
            timestamp: Utc::now(),
        };
        assert!(start.summary().contains("anthropic"));

        let end = NormalizedMessage::StreamEnd {
            id: NormalizedMessage::new_id(),
            chunks: 42,
            duration_ms: 1500,
            timestamp: Utc::now(),
        };
        assert!(end.summary().contains("42 chunks"));
    }

    #[test]
    fn from_conversation_entry_user() {
        let entry = ConversationEntry::user("hello");
        let msg =
            NormalizedMessage::from_conversation_entry(&entry).expect("should convert user entry");
        assert_eq!(msg.origin(), NormalizedOrigin::User);
    }

    #[test]
    fn from_conversation_entry_assistant() {
        let entry = ConversationEntry::assistant("response");
        let msg = NormalizedMessage::from_conversation_entry(&entry)
            .expect("should convert assistant entry");
        assert_eq!(msg.origin(), NormalizedOrigin::Assistant);
    }

    #[test]
    fn from_conversation_entry_system() {
        let entry = ConversationEntry::system("info");
        let msg = NormalizedMessage::from_conversation_entry(&entry)
            .expect("should convert system entry");
        assert_eq!(msg.origin(), NormalizedOrigin::System);
    }

    #[test]
    fn from_conversation_entry_tool() {
        let entry = ConversationEntry::tool("tc-1", "bash", "ok", false);
        let msg =
            NormalizedMessage::from_conversation_entry(&entry).expect("should convert tool entry");
        assert_eq!(msg.origin(), NormalizedOrigin::Tool);
    }

    #[test]
    fn variant_count_matches() {
        assert_eq!(NormalizedMessage::variant_count(), 24);
    }

    #[test]
    fn serialization_roundtrip_all_variants() {
        let messages: Vec<NormalizedMessage> = vec![
            NormalizedMessage::user_text("hi"),
            NormalizedMessage::assistant_text("hello"),
            NormalizedMessage::tool_result("tc-1", "bash", "ok", false),
            NormalizedMessage::system_info("started"),
            NormalizedMessage::system_error("failed", None),
            NormalizedMessage::compact_boundary("done", 10),
            NormalizedMessage::progress("thinking", "processing"),
            NormalizedMessage::status_update("idle"),
            NormalizedMessage::UserAttachment {
                id: NormalizedMessage::new_id(),
                label: "file.png".to_owned(),
                count: 1,
                timestamp: Utc::now(),
            },
            NormalizedMessage::Thinking {
                id: NormalizedMessage::new_id(),
                content: "hmm".to_owned(),
                timestamp: Utc::now(),
            },
        ];
        for msg in &messages {
            let json = serde_json::to_string(msg).expect("serialize should succeed");
            let parsed: NormalizedMessage =
                serde_json::from_str(&json).expect("deserialize should succeed");
            assert_eq!(
                serde_json::to_string(msg).expect("re-serialize"),
                serde_json::to_string(&parsed).expect("parsed re-serialize")
            );
        }
    }

    #[test]
    fn timestamp_is_set() {
        let msg = NormalizedMessage::user_text("test");
        let now = Utc::now();
        let diff = now.timestamp() - msg.timestamp().timestamp();
        assert!(diff.abs() < 5, "timestamp should be close to now");
    }

    // ── Standalone message-type serde round-trip tests ────────────────

    fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    #[test]
    fn system_thinking_message_serde_roundtrip() {
        let msg = SystemThinkingMessage {
            id: new_id(),
            content: "reasoning about the problem".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemThinkingMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.content, msg.content);
    }

    #[test]
    fn system_compact_boundary_message_serde_roundtrip() {
        let msg = SystemCompactBoundaryMessage {
            id: new_id(),
            summary: "compacted 50 messages".to_owned(),
            messages_removed: 50,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemCompactBoundaryMessage =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.messages_removed, 50);
    }

    #[test]
    fn system_microcompact_boundary_message_serde_roundtrip() {
        let msg = SystemMicrocompactBoundaryMessage {
            id: new_id(),
            description: "cleared old tool results".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemMicrocompactBoundaryMessage =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.description, "cleared old tool results");
    }

    #[test]
    fn system_permission_retry_message_serde_roundtrip() {
        let msg = SystemPermissionRetryMessage {
            id: new_id(),
            tool_name: "bash".to_owned(),
            reason: "user denied".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemPermissionRetryMessage =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.tool_name, "bash");
    }

    #[test]
    fn system_memory_saved_message_serde_roundtrip() {
        let msg = SystemMemorySavedMessage {
            id: new_id(),
            written_paths: vec!["user_role.md".to_owned(), "team/project.md".to_owned()],
            team_count: Some(1),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("writtenPaths"));
        assert!(json.contains("teamCount"));
        let parsed: SystemMemorySavedMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.written_paths, msg.written_paths);
        assert_eq!(parsed.team_count, Some(1));
    }

    #[test]
    fn system_memory_saved_message_omits_absent_team_count() {
        let msg = SystemMemorySavedMessage {
            id: new_id(),
            written_paths: vec!["feedback_testing.md".to_owned()],
            team_count: None,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(!json.contains("teamCount"));
    }

    #[test]
    fn system_stop_hook_summary_message_serde_roundtrip() {
        let msg = SystemStopHookSummaryMessage {
            id: new_id(),
            hook_name: "post-stop".to_owned(),
            output: "cleaned up".to_owned(),
            is_error: false,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemStopHookSummaryMessage =
            serde_json::from_str(&json).expect("deserialize");
        assert!(!parsed.is_error);
    }

    #[test]
    fn system_away_summary_message_serde_roundtrip() {
        let msg = SystemAwaySummaryMessage {
            id: new_id(),
            summary: "3 tasks completed".to_owned(),
            message_count: 15,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemAwaySummaryMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.message_count, 15);
    }

    #[test]
    fn system_agents_killed_message_serde_roundtrip() {
        let msg = SystemAgentsKilledMessage {
            id: new_id(),
            agent_ids: vec!["agent-1".to_owned(), "agent-2".to_owned()],
            reason: "timeout".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemAgentsKilledMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.agent_ids.len(), 2);
    }

    #[test]
    fn system_api_metrics_message_serde_roundtrip() {
        let msg = SystemApiMetricsMessage {
            id: new_id(),
            input_tokens: 5000,
            output_tokens: 1200,
            latency_ms: 3500,
            model: "claude-sonnet-4-20250514".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemApiMetricsMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.input_tokens, 5000);
    }

    #[test]
    fn system_api_error_message_serde_roundtrip() {
        let msg = SystemAPIErrorMessage {
            id: new_id(),
            error_code: "rate_limit".to_owned(),
            error_message: "Too many requests".to_owned(),
            retryable: true,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemAPIErrorMessage = serde_json::from_str(&json).expect("deserialize");
        assert!(parsed.retryable);
    }

    #[test]
    fn system_file_snapshot_message_serde_roundtrip() {
        let msg = SystemFileSnapshotMessage {
            id: new_id(),
            path: "src/main.rs".to_owned(),
            content_hash: "abc123".to_owned(),
            size_bytes: 4096,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: SystemFileSnapshotMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.path, "src/main.rs");
    }

    #[test]
    fn hook_result_message_type_serde_roundtrip() {
        let msg = HookResultMessageType {
            id: new_id(),
            hook_name: "pre-commit".to_owned(),
            output: "all checks passed".to_owned(),
            is_error: false,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: HookResultMessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.hook_name, "pre-commit");
    }

    #[test]
    fn tool_use_summary_message_type_serde_roundtrip() {
        let msg = ToolUseSummaryMessageType {
            id: new_id(),
            tool_call_id: "tc-1".to_owned(),
            tool_name: "bash".to_owned(),
            summary: "ran tests".to_owned(),
            is_error: false,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: ToolUseSummaryMessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.tool_call_id, "tc-1");
    }

    #[test]
    fn tombstone_message_type_serde_roundtrip() {
        let msg = TombstoneMessageType {
            id: new_id(),
            replaced_ids: vec!["msg-1".to_owned()],
            summary: "old content".to_owned(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: TombstoneMessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.replaced_ids.len(), 1);
    }

    #[test]
    fn progress_message_type_serde_roundtrip() {
        let msg = ProgressMessageType {
            id: new_id(),
            stage: "thinking".to_owned(),
            status: "processing".to_owned(),
            percent: Some(75),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: ProgressMessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.percent, Some(75));
    }

    #[test]
    fn attachment_message_type_serde_roundtrip() {
        let msg = AttachmentMessageType {
            id: new_id(),
            path: "image.png".to_owned(),
            mime_type: Some("image/png".to_owned()),
            size_bytes: Some(1024),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: AttachmentMessageType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.path, "image.png");
    }
}
