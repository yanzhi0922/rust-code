//! Stream-JSON protocol for headless / machine-readable mode.
//!
//! Defines the line-delimited JSON protocol used between the CLI and external
//! consumers. [`ProtocolEmitter`] writes events; [`parse_input_line`] reads them.

use std::io::Write;

use anyhow::Result;
use claude_core::SessionState;
use claude_permissions::PermissionUpdate;
use claude_ui_bridge::{UiRuntimeStatusSnapshot, UiTaskNode};
pub use rc_engine_events::{
    DaemonPresenceState, MessageRole, RuntimeEventCreateRequest, RuntimeEventDetail,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// An input message parsed from the external consumer.
#[derive(Debug, Clone)]
pub enum ProtocolInput {
    /// User text input.
    User {
        /// The user's text content.
        content: String,
    },
    /// Response to a permission request.
    ControlResponse {
        /// ID of the original permission request.
        request_id: String,
        /// Whether the action is allowed.
        allow: bool,
        /// Optional message.
        message: Option<String>,
        /// Optional updated tool input supplied by the approver.
        updated_input: Option<Value>,
        /// Optional session/user permission updates supplied by the approver.
        permission_updates: Vec<PermissionUpdate>,
        /// Optional free-form approval/rejection feedback.
        feedback: Option<String>,
        /// Optional provider-facing content blocks attached to the decision.
        content_blocks: Vec<Value>,
    },
    /// Control request from the external consumer.
    ControlRequest {
        /// Optional ID of the control request, when the caller expects a response.
        request_id: Option<String>,
        /// Requested control operation.
        request: ControlRequest,
    },
}

/// Control operations accepted from the external consumer.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    /// SDK/session initialization handshake.
    Initialize {
        /// Optional custom system prompt supplied by the SDK host.
        system_prompt: Option<String>,
        /// Optional prompt suffix supplied by the SDK host.
        append_system_prompt: Option<String>,
        /// Optional structured-output JSON schema supplied by the SDK host.
        json_schema: Option<Value>,
    },
    /// Gracefully end the stream-json session.
    EndSession {
        /// Optional caller-provided reason for shutdown.
        reason: Option<String>,
    },
    /// Interrupt the currently running turn.
    Interrupt,
    /// Update the active permission mode for subsequent tool decisions.
    SetPermissionMode {
        /// Claude Code permission mode string.
        mode: String,
    },
    /// Update the active model for subsequent turns.
    SetModel {
        /// New model identifier. `None` clears the explicit model override.
        model: Option<String>,
    },
    /// Update the maximum thinking-token budget for subsequent turns.
    SetMaxThinkingTokens {
        /// Token budget, or `None` to clear it.
        max_thinking_tokens: Option<u32>,
    },
    /// Request a context-usage snapshot.
    GetContextUsage,
    /// Request MCP connection status.
    McpStatus,
    /// Unknown control subtype, retained so callers can ignore it deliberately.
    Unknown(String),
}

/// Writes line-delimited JSON protocol events to an underlying writer.
pub struct ProtocolEmitter<W: Write> {
    writer: W,
    session_id: Uuid,
}

impl<W: Write> ProtocolEmitter<W> {
    /// Create a new emitter writing to `writer` for the given session.
    pub fn new(writer: W, session_id: Uuid) -> Self {
        Self { writer, session_id }
    }

    /// Emit an `init` system event with session metadata.
    pub fn emit_init(&mut self, payload: InitPayload) -> Result<()> {
        self.emit(json!({
            "type": "system",
            "subtype": "init",
            "apiKeySource": payload.api_key_source,
            "remote_code_version": payload.version,
            "cwd": payload.cwd,
            "tools": payload.tools,
            "mcp_servers": payload.mcp_servers,
            "model": payload.model,
            "permissionMode": payload.permission_mode,
            "slash_commands": payload.slash_commands,
            "output_style": payload.output_style,
            "skills": payload.skills,
            "plugins": payload.plugins,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a session state change event.
    pub fn emit_state(&mut self, state: SessionState) -> Result<()> {
        self.emit(json!({
            "type": "system",
            "subtype": "session_state_changed",
            "state": state.as_str(),
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a status message event.
    pub fn emit_status(&mut self, status: impl AsRef<str>) -> Result<()> {
        self.emit(json!({
            "type": "system",
            "subtype": "status",
            "status": status.as_ref(),
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a replayed user message acknowledgement for SDK stream-json consumers.
    pub fn emit_replayed_user_message(&mut self, content: impl AsRef<str>) -> Result<()> {
        self.emit(json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content.as_ref(),
            },
            "parent_tool_use_id": Value::Null,
            "isReplay": true,
            "uuid": Uuid::new_v4(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a memory-saved system message matching Claude Code's external surface.
    pub fn emit_memory_saved(
        &mut self,
        written_paths: &[String],
        team_count: Option<usize>,
    ) -> Result<()> {
        let mut event = json!({
            "type": "system",
            "subtype": "memory_saved",
            "writtenPaths": written_paths,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "uuid": Uuid::new_v4(),
            "isMeta": false,
            "session_id": self.session_id,
        });
        if let Some(team_count) = team_count {
            event["teamCount"] = json!(team_count);
        }
        self.emit(event)
    }

    /// Emit an assistant text message.
    pub fn emit_assistant(&mut self, text: impl AsRef<str>) -> Result<()> {
        self.emit(json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text.as_ref()}],
            },
            "parent_tool_use_id": Value::Null,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a normalized message delta event for remote consumers.
    pub fn emit_message_delta(
        &mut self,
        role: &str,
        delta: impl AsRef<str>,
        message_id: Option<&str>,
    ) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::MessageDelta {
            role: parse_message_role(role),
            delta: delta.as_ref().to_owned(),
            message_id: message_id.map(ToOwned::to_owned),
        })
    }

    /// Emit a normalized committed-message event for remote consumers.
    pub fn emit_message_committed(
        &mut self,
        role: &str,
        text: impl AsRef<str>,
        message_id: Option<&str>,
    ) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::MessageCommitted {
            role: parse_message_role(role),
            text: text.as_ref().to_owned(),
            message_id: message_id.map(ToOwned::to_owned),
        })
    }

    /// Emit a tool-started event.
    pub fn emit_tool_started(&mut self, tool_use_id: &str, tool_name: &str) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::ToolStarted {
            tool_call_id: tool_use_id.into(),
            tool_name: tool_name.into(),
        })
    }

    /// Emit a structured subtask-started event.
    pub fn emit_subtask_started(
        &mut self,
        task_id: &str,
        parent_task_id: Option<&str>,
        description: &str,
        depth: u32,
    ) -> Result<()> {
        self.emit(json!({
            "type": "subtask_started",
            "task_id": task_id,
            "parent_task_id": parent_task_id,
            "description": description,
            "depth": depth,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a structured subtask-progress event.
    pub fn emit_subtask_progress(
        &mut self,
        task_id: &str,
        turn: u32,
        max_turns: u32,
        summary: &str,
    ) -> Result<()> {
        self.emit(json!({
            "type": "subtask_progress",
            "task_id": task_id,
            "turn": turn,
            "max_turns": max_turns,
            "summary": summary,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a structured subtask-completed event.
    pub fn emit_subtask_completed(
        &mut self,
        task_id: &str,
        success: bool,
        output_preview: &str,
        turns_used: u32,
    ) -> Result<()> {
        self.emit(json!({
            "type": "subtask_completed",
            "task_id": task_id,
            "success": success,
            "output_preview": output_preview,
            "turns_used": turns_used,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a structured batch-progress event.
    pub fn emit_batch_progress(
        &mut self,
        total: usize,
        completed: usize,
        running: usize,
    ) -> Result<()> {
        self.emit(json!({
            "type": "batch_progress",
            "total": total,
            "completed": completed,
            "running": running,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a shared runtime status snapshot for statuslines and GUI consumers.
    pub fn emit_status_snapshot(&mut self, snapshot: &UiRuntimeStatusSnapshot) -> Result<()> {
        self.emit(json!({
            "type": "status_snapshot",
            "snapshot": snapshot,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a context-usage snapshot.
    pub fn emit_context_usage(
        &mut self,
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    ) -> Result<()> {
        self.emit(json!({
            "type": "context_usage",
            "estimated_tokens": estimated_tokens,
            "max_input_tokens": max_input_tokens,
            "threshold_tokens": threshold_tokens,
            "ratio": ratio,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a context-overflow warning before compaction.
    pub fn emit_context_overflow(
        &mut self,
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    ) -> Result<()> {
        self.emit(json!({
            "type": "context_overflow",
            "estimated_tokens": estimated_tokens,
            "max_input_tokens": max_input_tokens,
            "threshold_tokens": threshold_tokens,
            "ratio": ratio,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a context-compacted event.
    pub fn emit_context_compacted(
        &mut self,
        entries_removed: usize,
        usage_ratio: f64,
    ) -> Result<()> {
        self.emit(json!({
            "type": "context_compacted",
            "entries_removed": entries_removed,
            "usage_ratio": usage_ratio,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a snapshot of the current task tree for the active session.
    pub fn emit_task_snapshot(&mut self, tasks: Vec<UiTaskNode>) -> Result<()> {
        self.emit(json!({
            "type": "task_snapshot",
            "tasks": tasks,
            "uuid": Uuid::new_v4(),
            "session_id": self.session_id,
        }))
    }

    /// Emit a result event summarising the completed turn.
    pub fn emit_result(&mut self, payload: ResultPayload) -> Result<()> {
        self.emit(result_event_value(self.session_id, &payload))
    }

    /// Emit a permission request event for the external consumer.
    pub fn emit_permission_request(&mut self, payload: PermissionRequestPayload) -> Result<()> {
        let mut request = Map::new();
        request.insert("subtype".to_owned(), json!("can_use_tool"));
        request.insert("tool_name".to_owned(), json!(payload.tool_name));
        request.insert("input".to_owned(), payload.input);
        request.insert("tool_use_id".to_owned(), json!(payload.tool_use_id));
        if !payload.title.trim().is_empty() {
            request.insert("title".to_owned(), json!(payload.title));
        }
        if !payload.description.trim().is_empty() {
            request.insert("description".to_owned(), json!(payload.description));
        }
        if let Some(display_name) = payload
            .display_name
            .filter(|value| !value.trim().is_empty())
        {
            request.insert("display_name".to_owned(), json!(display_name));
        }
        if let Some(decision_reason) = payload.decision_reason {
            request.insert("decision_reason".to_owned(), decision_reason);
        }
        if let Some(agent_id) = payload.agent_id.filter(|value| !value.trim().is_empty()) {
            request.insert("agent_id".to_owned(), json!(agent_id));
        }
        if let Some(blocked_path) = payload
            .blocked_path
            .filter(|value| !value.trim().is_empty())
        {
            request.insert("blocked_path".to_owned(), json!(blocked_path));
        }
        if !payload.permission_suggestions.is_empty() {
            request.insert(
                "permission_suggestions".to_owned(),
                json!(payload.permission_suggestions),
            );
        }

        self.emit(json!({
            "type": "control_request",
            "request_id": payload.request_id,
            "request": request,
        }))
    }

    /// Emit a cancellation event for a previously sent permission request.
    pub fn emit_permission_cancelled(&mut self, request_id: &str) -> Result<()> {
        self.emit(json!({
            "type": "control_cancel_request",
            "request_id": request_id,
        }))
    }

    /// Emit a successful control response for SDK control requests.
    pub fn emit_control_success_response(
        &mut self,
        request_id: &str,
        response: Value,
    ) -> Result<()> {
        self.emit(json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        }))
    }

    /// Emit an error control response for unsupported or invalid SDK control requests.
    pub fn emit_control_error_response(&mut self, request_id: &str, error: &str) -> Result<()> {
        self.emit(json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": error,
            },
        }))
    }

    /// Emit a tool progress heartbeat event.
    pub fn emit_tool_progress(&mut self, tool_name: &str, elapsed_time_seconds: u64) -> Result<()> {
        self.emit_tool_progress_detail(ToolProgressPayload {
            tool_use_id: None,
            tool_name: Some(tool_name.to_owned()),
            input_delta: None,
            elapsed_time_seconds: Some(elapsed_time_seconds),
        })
    }

    /// Emit a detailed tool progress update for remote consumers.
    pub fn emit_tool_progress_detail(&mut self, payload: ToolProgressPayload) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::ToolProgress {
            tool_call_id: payload.tool_use_id.map(Into::into),
            tool_name: payload.tool_name.map(Into::into),
            delta: payload.input_delta,
            elapsed_time_seconds: payload.elapsed_time_seconds,
        })
    }

    /// Emit a tool-finished event.
    pub fn emit_tool_finished(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        is_error: bool,
        summary: Option<&str>,
    ) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::ToolFinished {
            tool_call_id: tool_use_id.into(),
            tool_name: tool_name.into(),
            is_error,
            summary: summary.map(ToOwned::to_owned),
        })
    }

    /// Emit a normalized runtime error event.
    pub fn emit_runtime_error(&mut self, message: impl AsRef<str>) -> Result<()> {
        self.emit_runtime_event(&RuntimeEventDetail::RuntimeError {
            message: message.as_ref().to_owned(),
        })
    }

    /// Emit a shared runtime event using the protocol's legacy wire format.
    pub fn emit_runtime_event(&mut self, detail: &RuntimeEventDetail) -> Result<()> {
        self.emit(ProtocolRuntimeEnvelope::new(self.session_id, detail))
    }

    fn emit<T: Serialize>(&mut self, event: T) -> Result<()> {
        serde_json::to_writer(&mut self.writer, &event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Payload for the `init` event emitted at session start.
#[derive(Debug, Clone)]
pub struct InitPayload {
    /// API key source description.
    pub api_key_source: String,
    /// Application version string.
    pub version: String,
    /// Current working directory.
    pub cwd: String,
    /// List of available tool names.
    pub tools: Vec<String>,
    /// List of configured MCP server names.
    pub mcp_servers: Vec<String>,
    /// Model identifier.
    pub model: Option<String>,
    /// Active permission mode.
    pub permission_mode: String,
    /// Available slash commands.
    pub slash_commands: Vec<String>,
    /// Output style setting.
    pub output_style: String,
    /// Available skill names.
    pub skills: Vec<String>,
    /// Available plugin names.
    pub plugins: Vec<String>,
}

/// Build the JSON object emitted for a completed turn.
#[must_use]
pub fn result_event_value(session_id: Uuid, payload: &ResultPayload) -> Value {
    let mut event = json!({
        "type": "result",
        "subtype": if payload.is_error { "error_during_execution" } else { "success" },
        "duration_ms": payload.duration_ms,
        "duration_api_ms": payload.duration_api_ms,
        "is_error": payload.is_error,
        "num_turns": payload.num_turns,
        "result": payload.result,
        "stop_reason": payload.stop_reason,
        "total_cost_usd": payload.total_cost_usd,
        "usage": {
            "input_tokens": payload.usage.input_tokens,
            "output_tokens": payload.usage.output_tokens,
            "cache_creation_input_tokens": payload.usage.cache_creation_input_tokens,
            "cache_read_input_tokens": payload.usage.cache_read_input_tokens,
            "service_tier": "standard",
        },
        "modelUsage": payload.model_usage,
        "permission_denials": payload.permission_denials,
        "uuid": Uuid::new_v4(),
        "session_id": session_id,
    });
    if !payload.errors.is_empty() {
        event["errors"] = json!(payload.errors);
    }
    event
}

/// Payload for the `result` event emitted at turn completion.
#[derive(Debug, Clone)]
pub struct ResultPayload {
    /// Whether the turn ended in error.
    pub is_error: bool,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// API duration in milliseconds.
    pub duration_api_ms: u64,
    /// Number of conversation turns.
    pub num_turns: u32,
    /// Final result text.
    pub result: String,
    /// Provider stop reason.
    pub stop_reason: String,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Token usage breakdown.
    pub usage: UsagePayload,
    /// Per-model usage data.
    pub model_usage: Value,
    /// Permission denial records.
    pub permission_denials: Vec<Value>,
    /// Error messages encountered during the turn.
    pub errors: Vec<String>,
}

/// Token usage payload included in result events.
#[derive(Debug, Clone, Default)]
pub struct UsagePayload {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
    /// Anthropic cache read tokens (tokens served from cache).
    pub cache_read_input_tokens: u64,
    /// Anthropic cache creation tokens (tokens written to cache).
    pub cache_creation_input_tokens: u64,
}

/// Payload for the `control_request` permission event.
#[derive(Debug, Clone)]
pub struct PermissionRequestPayload {
    /// Unique request identifier.
    pub request_id: String,
    /// Tool name requesting permission.
    pub tool_name: String,
    /// Tool use identifier.
    pub tool_use_id: String,
    /// Short human-readable title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Tool input JSON.
    pub input: Value,
    /// Affected path, if any.
    pub blocked_path: Option<String>,
    /// Suggested permission rules.
    pub permission_suggestions: Vec<Value>,
    /// Optional display name matching the SDK control permission envelope.
    pub display_name: Option<String>,
    /// Optional structured decision reason from permission pre-classification.
    pub decision_reason: Option<Value>,
    /// Optional agent identifier for subagent-routed permission prompts.
    pub agent_id: Option<String>,
}

/// Payload for normalized tool progress events.
#[derive(Debug, Clone, Default)]
pub struct ToolProgressPayload {
    /// Tool use identifier when available.
    pub tool_use_id: Option<String>,
    /// Tool name when available.
    pub tool_name: Option<String>,
    /// Incremental tool-input delta.
    pub input_delta: Option<String>,
    /// Tool execution elapsed seconds, if known.
    pub elapsed_time_seconds: Option<u64>,
}

#[derive(Serialize)]
struct ProtocolRuntimeEnvelope<'a> {
    #[serde(flatten)]
    detail: ProtocolRuntimeEventRef<'a>,
    uuid: Uuid,
    session_id: Uuid,
}

impl<'a> ProtocolRuntimeEnvelope<'a> {
    fn new(session_id: Uuid, detail: &'a RuntimeEventDetail) -> Self {
        Self {
            detail: ProtocolRuntimeEventRef::from(detail),
            uuid: Uuid::new_v4(),
            session_id,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ProtocolRuntimeEventRef<'a> {
    MessageDelta {
        role: &'a MessageRole,
        delta: &'a str,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<&'a str>,
    },
    MessageCommitted {
        role: &'a MessageRole,
        text: &'a str,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<&'a str>,
    },
    ToolStarted {
        #[serde(rename = "tool_use_id")]
        tool_call_id: &'a str,
        tool_name: &'a str,
    },
    ToolProgress {
        #[serde(
            rename = "tool_use_id",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        tool_call_id: Option<&'a str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<&'a str>,
        #[serde(
            rename = "input_delta",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        delta: Option<&'a str>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_time_seconds: Option<u64>,
    },
    ToolFinished {
        #[serde(rename = "tool_use_id")]
        tool_call_id: &'a str,
        tool_name: &'a str,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<&'a str>,
    },
    ArtifactManifest {
        artifact_ids: &'a [Uuid],
    },
    RuntimeError {
        message: &'a str,
    },
    DaemonPresenceChanged {
        state: &'a DaemonPresenceState,
    },
    SubtaskStarted {
        task_id: &'a str,
        parent_task_id: Option<&'a str>,
        description: &'a str,
        depth: u32,
    },
    SubtaskProgress {
        task_id: &'a str,
        status: &'a str,
        summary: &'a str,
    },
    SubtaskCompleted {
        task_id: &'a str,
        status: &'a str,
        summary: &'a str,
        turns_used: Option<u32>,
    },
    BatchProgress {
        total: u32,
        completed: u32,
        running: u32,
    },
    ContextUsage {
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    },
    ContextOverflow {
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    },
    ContextCompacted {
        entries_removed: u32,
        usage_ratio: f64,
    },
}

impl<'a> From<&'a RuntimeEventDetail> for ProtocolRuntimeEventRef<'a> {
    fn from(value: &'a RuntimeEventDetail) -> Self {
        match value {
            RuntimeEventDetail::MessageDelta {
                role,
                delta,
                message_id,
            } => Self::MessageDelta {
                role,
                delta,
                message_id: message_id.as_deref(),
            },
            RuntimeEventDetail::MessageCommitted {
                role,
                text,
                message_id,
            } => Self::MessageCommitted {
                role,
                text,
                message_id: message_id.as_deref(),
            },
            RuntimeEventDetail::ToolStarted {
                tool_call_id,
                tool_name,
            } => Self::ToolStarted {
                tool_call_id: tool_call_id.as_ref(),
                tool_name: tool_name.as_ref(),
            },
            RuntimeEventDetail::ToolProgress {
                tool_call_id,
                tool_name,
                delta,
                elapsed_time_seconds,
            } => Self::ToolProgress {
                tool_call_id: tool_call_id.as_deref(),
                tool_name: tool_name.as_deref(),
                delta: delta.as_deref(),
                elapsed_time_seconds: *elapsed_time_seconds,
            },
            RuntimeEventDetail::ToolFinished {
                tool_call_id,
                tool_name,
                is_error,
                summary,
            } => Self::ToolFinished {
                tool_call_id: tool_call_id.as_ref(),
                tool_name: tool_name.as_ref(),
                is_error: *is_error,
                summary: summary.as_deref(),
            },
            RuntimeEventDetail::ArtifactManifest { artifact_ids } => Self::ArtifactManifest {
                artifact_ids: artifact_ids.as_slice(),
            },
            RuntimeEventDetail::RuntimeError { message } => Self::RuntimeError { message },
            RuntimeEventDetail::DaemonPresenceChanged { state } => {
                Self::DaemonPresenceChanged { state }
            }
            RuntimeEventDetail::SubtaskStarted {
                task_id,
                parent_task_id,
                description,
                depth,
            } => Self::SubtaskStarted {
                task_id: task_id.as_ref(),
                parent_task_id: parent_task_id.as_deref(),
                description,
                depth: *depth,
            },
            RuntimeEventDetail::SubtaskProgress {
                task_id,
                status,
                summary,
            } => Self::SubtaskProgress {
                task_id: task_id.as_ref(),
                status,
                summary,
            },
            RuntimeEventDetail::SubtaskCompleted {
                task_id,
                status,
                summary,
                turns_used,
            } => Self::SubtaskCompleted {
                task_id: task_id.as_ref(),
                status,
                summary,
                turns_used: *turns_used,
            },
            RuntimeEventDetail::BatchProgress {
                total,
                completed,
                running,
            } => Self::BatchProgress {
                total: *total,
                completed: *completed,
                running: *running,
            },
            RuntimeEventDetail::ContextUsage {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextUsage {
                estimated_tokens: *estimated_tokens,
                max_input_tokens: *max_input_tokens,
                threshold_tokens: *threshold_tokens,
                ratio: *ratio,
            },
            RuntimeEventDetail::ContextOverflow {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextOverflow {
                estimated_tokens: *estimated_tokens,
                max_input_tokens: *max_input_tokens,
                threshold_tokens: *threshold_tokens,
                ratio: *ratio,
            },
            RuntimeEventDetail::ContextCompacted {
                entries_removed,
                usage_ratio,
            } => Self::ContextCompacted {
                entries_removed: *entries_removed,
                usage_ratio: *usage_ratio,
            },
        }
    }
}

fn parse_message_role(role: &str) -> MessageRole {
    if role.eq_ignore_ascii_case("assistant") {
        MessageRole::Assistant
    } else if role.eq_ignore_ascii_case("user") {
        MessageRole::User
    } else {
        MessageRole::System
    }
}

fn parse_message_content(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => trimmed_non_empty(text).map(ToOwned::to_owned),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(content_block_to_text)
                .collect::<Vec<_>>()
                .join("\n");
            trimmed_non_empty(&text).map(ToOwned::to_owned)
        }
        _ => None,
    }
}

fn content_block_to_text(block: &Value) -> Option<String> {
    let block_type = block.get("type").and_then(Value::as_str);
    match block_type {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .and_then(trimmed_non_empty)
            .map(ToOwned::to_owned),
        Some("image") => Some("[image]".to_owned()),
        Some("tool_result") => block
            .get("content")
            .and_then(parse_tool_result_content)
            .or_else(|| Some("[tool_result]".to_owned())),
        Some(other) => Some(format!("[{other}]")),
        None => serde_json::to_string(block).ok(),
    }
}

fn parse_tool_result_content(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => trimmed_non_empty(text).map(ToOwned::to_owned),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(content_block_to_text)
                .collect::<Vec<_>>()
                .join("\n");
            trimmed_non_empty(&text).map(ToOwned::to_owned)
        }
        other => serde_json::to_string(other).ok(),
    }
}

fn trimmed_non_empty(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn parse_control_request(request: &Value) -> Option<ControlRequest> {
    let subtype = request.get("subtype").and_then(Value::as_str)?;
    match subtype {
        "initialize" => Some(ControlRequest::Initialize {
            system_prompt: request
                .get("systemPrompt")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            append_system_prompt: request
                .get("appendSystemPrompt")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            json_schema: request.get("jsonSchema").cloned(),
        }),
        "end_session" => Some(ControlRequest::EndSession {
            reason: request
                .get("reason")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }),
        "interrupt" => Some(ControlRequest::Interrupt),
        "set_permission_mode" => Some(ControlRequest::SetPermissionMode {
            mode: request.get("mode")?.as_str()?.to_owned(),
        }),
        "set_model" => Some(ControlRequest::SetModel {
            model: request
                .get("model")
                .and_then(Value::as_str)
                .and_then(trimmed_non_empty)
                .map(ToOwned::to_owned),
        }),
        "set_max_thinking_tokens" => Some(ControlRequest::SetMaxThinkingTokens {
            max_thinking_tokens: request
                .get("max_thinking_tokens")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
        }),
        "get_context_usage" => Some(ControlRequest::GetContextUsage),
        "mcp_status" => Some(ControlRequest::McpStatus),
        other => Some(ControlRequest::Unknown(other.to_owned())),
    }
}

fn string_field(value: &Value, snake_case: &str, camel_case: &str) -> Option<String> {
    value
        .get(snake_case)
        .or_else(|| value.get(camel_case))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

/// Parse a single line of JSON input from the external consumer.
///
/// Returns `None` if the line cannot be parsed or is not a recognised event type.
pub fn parse_input_line(line: &str) -> Option<ProtocolInput> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let kind = value.get("type")?.as_str()?;
    match kind {
        "user" => {
            let content = parse_message_content(
                value
                    .get("message")
                    .and_then(|message| message.get("content"))?,
            )?;
            Some(ProtocolInput::User { content })
        }
        "control_response" => {
            let response = value.get("response")?;
            let request_id = string_field(response, "request_id", "requestId")
                .or_else(|| string_field(&value, "request_id", "requestId"))?;
            if response.get("subtype").and_then(Value::as_str) == Some("error") {
                return Some(ProtocolInput::ControlResponse {
                    request_id,
                    allow: false,
                    message: response
                        .get("error")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    updated_input: None,
                    permission_updates: Vec::new(),
                    feedback: None,
                    content_blocks: Vec::new(),
                });
            }
            let response_body = response.get("response");
            let behavior = response_body
                .and_then(|value| value.get("behavior"))
                .and_then(Value::as_str)
                .unwrap_or("deny");
            let message = response_body
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let updated_input = response_body
                .and_then(|value| value.get("updatedInput"))
                .cloned();
            let permission_updates = response_body
                .and_then(|value| {
                    value
                        .get("updatedPermissions")
                        .or_else(|| value.get("permissionUpdates"))
                })
                .and_then(|value| {
                    serde_json::from_value::<Vec<PermissionUpdate>>(value.clone()).ok()
                })
                .unwrap_or_default();
            let feedback = response_body
                .and_then(|value| {
                    value
                        .get("feedback")
                        .or_else(|| value.get("acceptFeedback"))
                })
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let content_blocks = response_body
                .and_then(|value| value.get("contentBlocks"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(ProtocolInput::ControlResponse {
                request_id,
                allow: behavior.eq_ignore_ascii_case("allow"),
                message,
                updated_input,
                permission_updates,
                feedback,
                content_blocks,
            })
        }
        "control_request" => {
            let request_value = value.get("request")?;
            Some(ProtocolInput::ControlRequest {
                request_id: string_field(&value, "request_id", "requestId"),
                request: parse_control_request(request_value)?,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_session_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("valid test UUID")
    }

    fn collect_lines(output: &[u8]) -> Vec<serde_json::Value> {
        let s = String::from_utf8_lossy(output);
        s.lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    #[test]
    fn emit_init_contains_required_fields() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_init(InitPayload {
                api_key_source: "env".to_owned(),
                version: "0.1.0".to_owned(),
                cwd: "/tmp".to_owned(),
                tools: vec!["bash".to_owned()],
                mcp_servers: vec![],
                model: Some("gpt-4".to_owned()),
                permission_mode: "default".to_owned(),
                slash_commands: vec![],
                output_style: "text".to_owned(),
                skills: vec![],
                plugins: vec![],
            })
            .expect("emit_init should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e["type"], "system");
        assert_eq!(e["subtype"], "init");
        assert_eq!(e["apiKeySource"], "env");
        assert_eq!(e["remote_code_version"], "0.1.0");
        assert_eq!(e["cwd"], "/tmp");
        assert_eq!(e["model"], "gpt-4");
        assert_eq!(e["session_id"], "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn emit_state_contains_session_state() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_state(SessionState::Running)
            .expect("emit_state should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "system");
        assert_eq!(events[0]["subtype"], "session_state_changed");
        assert_eq!(events[0]["state"], "running");
    }

    #[test]
    fn emit_replayed_user_message_matches_sdk_shape() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_replayed_user_message("hello")
            .expect("emit replayed user");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "user");
        assert_eq!(events[0]["message"]["role"], "user");
        assert_eq!(events[0]["message"]["content"], "hello");
        assert_eq!(events[0]["isReplay"], true);
        assert_eq!(events[0]["session_id"], test_session_id().to_string());
    }

    #[test]
    fn emit_assistant_contains_message() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_assistant("Hello world")
            .expect("emit_assistant should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "assistant");
        assert_eq!(events[0]["message"]["role"], "assistant");
        assert_eq!(events[0]["message"]["content"][0]["text"], "Hello world");
    }

    #[test]
    fn emit_normalized_message_events_for_remote_consumers() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_message_delta("assistant", "Hel", Some("msg-1"))
            .expect("emit_message_delta should succeed");
        emitter
            .emit_message_committed("assistant", "Hello world", Some("msg-1"))
            .expect("emit_message_committed should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "message_delta");
        assert_eq!(events[0]["role"], "assistant");
        assert_eq!(events[0]["delta"], "Hel");
        assert_eq!(events[0]["message_id"], "msg-1");
        assert_eq!(events[1]["type"], "message_committed");
        assert_eq!(events[1]["text"], "Hello world");
    }

    #[test]
    fn emit_normalized_tool_events_for_remote_consumers() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_tool_started("toolu-1", "bash_command")
            .expect("emit_tool_started should succeed");
        emitter
            .emit_tool_progress_detail(ToolProgressPayload {
                tool_use_id: Some("toolu-1".to_owned()),
                tool_name: None,
                input_delta: Some("{\"command\":\"ls\"}".to_owned()),
                elapsed_time_seconds: Some(2),
            })
            .expect("emit_tool_progress_detail should succeed");
        emitter
            .emit_tool_finished("toolu-1", "bash_command", false, Some("command completed"))
            .expect("emit_tool_finished should succeed");
        emitter
            .emit_runtime_error("simulated failure")
            .expect("emit_runtime_error should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["type"], "tool_started");
        assert_eq!(events[0]["tool_use_id"], "toolu-1");
        assert_eq!(events[1]["type"], "tool_progress");
        assert_eq!(events[1]["tool_use_id"], "toolu-1");
        assert_eq!(events[1]["input_delta"], "{\"command\":\"ls\"}");
        assert_eq!(events[1]["elapsed_time_seconds"], 2);
        assert_eq!(events[2]["type"], "tool_finished");
        assert_eq!(events[2]["summary"], "command completed");
        assert_eq!(events[3]["type"], "runtime_error");
        assert_eq!(events[3]["message"], "simulated failure");
    }

    #[test]
    fn emit_shared_runtime_event_preserves_protocol_field_names() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_runtime_event(&RuntimeEventDetail::ToolProgress {
                tool_call_id: Some("tool-call-1".into()),
                tool_name: Some("bash_command".into()),
                delta: Some("{\"command\":\"dir\"}".to_owned()),
                elapsed_time_seconds: Some(9),
            })
            .expect("emit_runtime_event should succeed");
        emitter
            .emit_runtime_event(&RuntimeEventDetail::DaemonPresenceChanged {
                state: DaemonPresenceState::Online,
            })
            .expect("emit_runtime_event should succeed");

        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "tool_progress");
        assert_eq!(events[0]["tool_use_id"], "tool-call-1");
        assert_eq!(events[0]["tool_name"], "bash_command");
        assert_eq!(events[0]["input_delta"], "{\"command\":\"dir\"}");
        assert_eq!(events[0]["elapsed_time_seconds"], 9);
        assert!(events[0].get("tool_call_id").is_none());
        assert!(events[0].get("delta").is_none());
        assert_eq!(events[1]["type"], "daemon_presence_changed");
        assert_eq!(events[1]["state"], "online");
    }

    #[test]
    fn emit_message_delta_helper_uses_shared_runtime_role_encoding() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_message_delta("USER", "hello", None)
            .expect("emit_message_delta should succeed");

        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message_delta");
        assert_eq!(events[0]["role"], "user");
        assert_eq!(events[0]["delta"], "hello");
        assert!(events[0].get("message_id").is_none());
    }

    #[test]
    fn emit_subtask_events_for_remote_consumers() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_subtask_started("task-1", Some("root-1"), "Investigate bug", 1)
            .expect("emit_subtask_started should succeed");
        emitter
            .emit_subtask_progress("task-1", 2, 5, "Called read_file")
            .expect("emit_subtask_progress should succeed");
        emitter
            .emit_subtask_completed("task-1", true, "done", 3)
            .expect("emit_subtask_completed should succeed");
        emitter
            .emit_batch_progress(4, 2, 2)
            .expect("emit_batch_progress should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["type"], "subtask_started");
        assert_eq!(events[0]["parent_task_id"], "root-1");
        assert_eq!(events[1]["type"], "subtask_progress");
        assert_eq!(events[1]["turn"], 2);
        assert_eq!(events[2]["type"], "subtask_completed");
        assert_eq!(events[2]["success"], true);
        assert_eq!(events[3]["type"], "batch_progress");
        assert_eq!(events[3]["completed"], 2);
    }

    #[test]
    fn emit_context_and_task_snapshot_events() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_context_usage(48_000, 100_000, 80_000, 0.48)
            .expect("emit_context_usage should succeed");
        emitter
            .emit_context_overflow(82_000, 100_000, 80_000, 0.82)
            .expect("emit_context_overflow should succeed");
        emitter
            .emit_context_compacted(6, 0.41)
            .expect("emit_context_compacted should succeed");
        emitter
            .emit_task_snapshot(vec![UiTaskNode {
                id: "task-1".to_owned(),
                parent_task_id: None,
                title: "Investigate".to_owned(),
                status: claude_ui_bridge::UiTaskStatus::Running,
                kind: claude_ui_bridge::UiTaskKind::Delegation,
                depth: 0,
                summary: "Working".to_owned(),
                turns_used: Some(1),
                output_path: None,
                created_at: "1".to_owned(),
                updated_at: "2".to_owned(),
            }])
            .expect("emit_task_snapshot should succeed");

        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["type"], "context_usage");
        assert_eq!(events[0]["estimated_tokens"], 48_000);
        assert_eq!(events[1]["type"], "context_overflow");
        assert_eq!(events[1]["threshold_tokens"], 80_000);
        assert_eq!(events[2]["type"], "context_compacted");
        assert_eq!(events[2]["entries_removed"], 6);
        assert_eq!(events[3]["type"], "task_snapshot");
        assert_eq!(events[3]["tasks"][0]["id"], "task-1");
    }

    #[test]
    fn emit_status_snapshot_for_shared_status_surfaces() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_status_snapshot(&claude_ui_bridge::UiRuntimeStatusSnapshot {
                session_name: Some("Parity".to_owned()),
                provider: claude_ui_bridge::UiProviderStatusSnapshot {
                    name: "glm-coding".to_owned(),
                    model: Some("glm-5.1".to_owned()),
                    protocol: "anthropic".to_owned(),
                    base_url: Some("https://open.bigmodel.cn/api/anthropic/v1/messages".to_owned()),
                    auth_source: Some("env:REMOTE_CODE_API_KEY".to_owned()),
                    effort: Some("medium".to_owned()),
                    fallback_model: Some("glm-5-turbo".to_owned()),
                },
                permission_mode: "default".to_owned(),
                output_style: Some("Explanatory".to_owned()),
                language: Some("zh-CN".to_owned()),
                brief_enabled: true,
                proactive_active: true,
                setting_sources: vec!["env:REMOTE_CODE_MODEL".to_owned()],
                allowed_setting_sources: vec!["user".to_owned(), "project".to_owned()],
                allowed_tools: vec!["read_file".to_owned()],
                disallowed_tools: vec!["bash_command".to_owned()],
                mcp: claude_ui_bridge::UiRuntimeMcpInventorySummary {
                    total_servers: 2,
                    enabled_servers: 1,
                    disabled_servers: 1,
                    unique_server_names: 2,
                    ambiguous_server_names: 0,
                    warning_count: 0,
                    origins: claude_ui_bridge::UiRuntimeMcpOriginCounts {
                        cwd: 1,
                        profile: 1,
                        explicit: 0,
                        plugin: 0,
                    },
                    status_counts: claude_ui_bridge::UiRuntimeMcpStatusCounts {
                        connected: 0,
                        failed: 0,
                        needs_auth: 0,
                        pending: 1,
                        disabled: 1,
                    },
                },
            })
            .expect("emit_status_snapshot should succeed");

        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "status_snapshot");
        assert_eq!(events[0]["snapshot"]["provider"]["name"], "glm-coding");
        assert_eq!(events[0]["snapshot"]["allowed_setting_sources"][0], "user");
        assert_eq!(events[0]["snapshot"]["permission_mode"], "default");
        assert_eq!(events[0]["snapshot"]["mcp"]["enabled_servers"], 1);
        assert_eq!(events[0]["snapshot"]["mcp"]["status_counts"]["pending"], 1);
    }

    #[test]
    fn emit_result_summarizes_turn() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_result(ResultPayload {
                is_error: false,
                duration_ms: 1234,
                duration_api_ms: 1000,
                num_turns: 3,
                result: "done".to_owned(),
                stop_reason: "end_turn".to_owned(),
                total_cost_usd: 0.005,
                usage: UsagePayload {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                },
                model_usage: json!({}),
                permission_denials: vec![],
                errors: vec![],
            })
            .expect("emit_result should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "result");
        assert_eq!(events[0]["subtype"], "success");
        assert_eq!(events[0]["duration_ms"], 1234);
        assert_eq!(events[0]["num_turns"], 3);
        assert_eq!(events[0]["usage"]["input_tokens"], 100);
    }

    #[test]
    fn emit_memory_saved_matches_system_message_surface() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_memory_saved(
                &["user_role.md".to_owned(), "team/project.md".to_owned()],
                Some(1),
            )
            .expect("emit_memory_saved should succeed");

        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "system");
        assert_eq!(events[0]["subtype"], "memory_saved");
        assert_eq!(events[0]["writtenPaths"][0], "user_role.md");
        assert_eq!(events[0]["teamCount"], 1);
        assert_eq!(events[0]["isMeta"], false);
        assert!(events[0]["timestamp"].is_string());
        assert!(events[0]["uuid"].is_string());
    }

    #[test]
    fn emit_permission_request_has_control_request_type() {
        let mut buf = Cursor::new(Vec::new());
        let mut emitter = ProtocolEmitter::new(&mut buf, test_session_id());
        emitter
            .emit_permission_request(PermissionRequestPayload {
                request_id: "req-1".to_owned(),
                tool_name: "bash_command".to_owned(),
                tool_use_id: "tu-1".to_owned(),
                title: "Run command".to_owned(),
                description: "ls -la".to_owned(),
                input: json!({"command": "ls -la"}),
                blocked_path: None,
                permission_suggestions: vec![],
                display_name: Some("Bash".to_owned()),
                decision_reason: Some(json!({"type": "mode", "mode": "default"})),
                agent_id: Some("agent-1".to_owned()),
            })
            .expect("emit_permission_request should succeed");
        let events = collect_lines(&buf.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "control_request");
        assert_eq!(events[0]["request_id"], "req-1");
        assert_eq!(events[0]["request"]["tool_name"], "bash_command");
        assert_eq!(events[0]["request"]["display_name"], "Bash");
        assert_eq!(events[0]["request"]["agent_id"], "agent-1");
        assert_eq!(events[0]["request"]["decision_reason"]["type"], "mode");
        assert!(events[0]["request"].get("blocked_path").is_none());
        assert!(events[0]["request"].get("permission_suggestions").is_none());
    }

    #[test]
    fn parse_input_line_user_message() {
        let input = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#;
        let result = parse_input_line(input).expect("should parse user message");
        match result {
            ProtocolInput::User { content } => assert_eq!(content, "hello"),
            _ => panic!("expected User variant"),
        }
    }

    #[test]
    fn parse_input_line_user_message_content_blocks() {
        let input = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}},{"type":"tool_result","content":[{"type":"text","text":"tool output"}]},{"type":"custom_block","payload":1}]}}"#;
        let result = parse_input_line(input).expect("should parse user content blocks");
        match result {
            ProtocolInput::User { content } => {
                assert_eq!(content, "hello\n[image]\ntool output\n[custom_block]");
            }
            _ => panic!("expected User variant"),
        }
    }

    #[test]
    fn parse_input_line_rejects_flat_user_content() {
        let input = r#"{"type":"user","content":"hello"}"#;
        assert!(parse_input_line(input).is_none());
    }

    #[test]
    fn parse_input_line_control_response_allow() {
        let input = r#"{"type":"control_response","response":{"request_id":"req-1","response":{"behavior":"allow","message":"ok","updatedInput":{"plan":"edited"},"permissionUpdates":[{"type":"setMode","destination":"session","mode":"acceptEdits"}],"feedback":"ship it","contentBlocks":[{"type":"text","text":"extra"}]}}}"#;
        let result = parse_input_line(input).expect("should parse control response");
        match result {
            ProtocolInput::ControlResponse {
                request_id,
                allow,
                message,
                updated_input,
                permission_updates,
                feedback,
                content_blocks,
            } => {
                assert_eq!(request_id, "req-1");
                assert!(allow);
                assert_eq!(message.as_deref(), Some("ok"));
                assert_eq!(updated_input, Some(json!({"plan":"edited"})));
                assert_eq!(permission_updates.len(), 1);
                assert_eq!(feedback.as_deref(), Some("ship it"));
                assert_eq!(content_blocks.len(), 1);
            }
            _ => panic!("expected ControlResponse variant"),
        }
    }

    #[test]
    fn parse_input_line_control_response_accepts_reference_aliases() {
        let input = r#"{"type":"control_response","response":{"requestId":"req-2","response":{"behavior":"allow","updatedInput":{},"updatedPermissions":[{"type":"setMode","destination":"session","mode":"acceptEdits"}]}}}"#;
        let result = parse_input_line(input).expect("should parse alias response");
        match result {
            ProtocolInput::ControlResponse {
                request_id,
                allow,
                permission_updates,
                ..
            } => {
                assert_eq!(request_id, "req-2");
                assert!(allow);
                assert_eq!(permission_updates.len(), 1);
            }
            _ => panic!("expected ControlResponse variant"),
        }
    }

    #[test]
    fn parse_input_line_control_response_error_is_deny() {
        let input = r#"{"type":"control_response","requestId":"req-err","response":{"subtype":"error","error":"bad request"}}"#;
        let result = parse_input_line(input).expect("should parse error response");
        match result {
            ProtocolInput::ControlResponse {
                request_id,
                allow,
                message,
                ..
            } => {
                assert_eq!(request_id, "req-err");
                assert!(!allow);
                assert_eq!(message.as_deref(), Some("bad request"));
            }
            _ => panic!("expected ControlResponse variant"),
        }
    }

    #[test]
    fn parse_input_line_interrupt() {
        let input = r#"{"type":"control_request","request":{"subtype":"interrupt"}}"#;
        let result = parse_input_line(input).expect("should parse interrupt");
        match result {
            ProtocolInput::ControlRequest {
                request_id: None,
                request: ControlRequest::Interrupt,
            } => {}
            _ => panic!("expected interrupt control request"),
        }
    }

    #[test]
    fn parse_input_line_control_request_variants() {
        let initialize =
            r#"{"type":"control_request","request_id":"ctl-1","request":{"subtype":"initialize"}}"#;
        let end_session = r#"{"type":"control_request","request_id":"ctl-2","request":{"subtype":"end_session"}}"#;
        let unknown =
            r#"{"type":"control_request","requestId":"ctl-3","request":{"subtype":"future_type"}}"#;

        assert!(matches!(
            parse_input_line(initialize),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::Initialize { .. },
            }) if id == "ctl-1"
        ));
        assert!(matches!(
            parse_input_line(end_session),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::EndSession { .. },
            }) if id == "ctl-2"
        ));
        assert!(matches!(
            parse_input_line(unknown),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::Unknown(subtype),
            }) if id == "ctl-3" && subtype == "future_type"
        ));
    }

    #[test]
    fn parse_input_line_mutating_control_requests() {
        let initialize = r#"{"type":"control_request","request_id":"ctl-init","request":{"subtype":"initialize","systemPrompt":"sys","appendSystemPrompt":"append","jsonSchema":{"type":"object"}}}"#;
        let set_mode = r#"{"type":"control_request","request_id":"ctl-mode","request":{"subtype":"set_permission_mode","mode":"acceptEdits"}}"#;
        let set_model = r#"{"type":"control_request","request_id":"ctl-model","request":{"subtype":"set_model","model":"claude-sonnet-4-6"}}"#;
        let thinking = r#"{"type":"control_request","request_id":"ctl-thinking","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":4096}}"#;

        assert!(matches!(
            parse_input_line(initialize),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::Initialize {
                    system_prompt: Some(system_prompt),
                    append_system_prompt: Some(append_system_prompt),
                    json_schema: Some(_),
                },
            }) if id == "ctl-init" && system_prompt == "sys" && append_system_prompt == "append"
        ));
        assert!(matches!(
            parse_input_line(set_mode),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::SetPermissionMode { mode },
            }) if id == "ctl-mode" && mode == "acceptEdits"
        ));
        assert!(matches!(
            parse_input_line(set_model),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::SetModel { model: Some(model) },
            }) if id == "ctl-model" && model == "claude-sonnet-4-6"
        ));
        assert!(matches!(
            parse_input_line(thinking),
            Some(ProtocolInput::ControlRequest {
                request_id: Some(id),
                request: ControlRequest::SetMaxThinkingTokens {
                    max_thinking_tokens: Some(4096),
                },
            }) if id == "ctl-thinking"
        ));
    }

    #[test]
    fn parse_input_line_invalid_json_returns_none() {
        assert!(parse_input_line("not json").is_none());
        assert!(parse_input_line("{}").is_none());
        assert!(parse_input_line(r#"{"type":"unknown"}"#).is_none());
    }
}
