//! Unified event model for all Agent adapters.
//!
//! Every Agent (Remote Code, Roo Code, Codex) translates its native events
//! into [`UnifiedAgentEvent`] variants so the rest of the system can handle
//! them uniformly.

use serde::{Deserialize, Serialize};

use crate::types::AgentInfo;

/// Unified Agent event — all Agent adapters translate their native events
/// into this common format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UnifiedAgentEvent {
    // ── Lifecycle ──
    /// Agent has finished starting up.
    Started(AgentInfo),
    /// Agent is ready to accept messages.
    Ready,

    // ── Message streaming ──
    /// Streaming text delta from the Agent.
    MessageDelta { session_id: String, delta: String },
    /// A tool invocation has started.
    ToolCallStarted {
        session_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
    },
    /// Progress update for an ongoing tool call.
    ToolCallProgress {
        session_id: String,
        tool_name: String,
        progress: String,
    },
    /// Native Codex app-server notification, preserved in its original
    /// method/params envelope for GUI features that need protocol parity.
    CodexAppServerNotification {
        session_id: String,
        method: String,
        params: serde_json::Value,
    },
    /// A tool invocation has completed.
    ToolCallCompleted {
        session_id: String,
        tool_name: String,
        result: serde_json::Value,
    },

    // ── Permissions ──
    /// Agent is requesting permission for an operation.
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
    },

    // ── Subtasks ──
    /// A subtask has been spawned.
    SubtaskStarted {
        session_id: String,
        task_id: String,
        description: String,
    },
    /// Progress update for a subtask.
    SubtaskProgress {
        session_id: String,
        task_id: String,
        progress: String,
    },
    /// A subtask has completed.
    SubtaskCompleted {
        session_id: String,
        task_id: String,
        result: serde_json::Value,
    },

    // ── Context management ──
    /// Context window usage report.
    ContextUsage {
        session_id: String,
        used: usize,
        total: usize,
    },
    /// Context window overflow detected.
    ContextOverflow {
        session_id: String,
        /// Estimated tokens at the time of overflow (0 if unknown).
        #[serde(default)]
        used: usize,
        /// Maximum context window size (0 if unknown).
        #[serde(default)]
        total: usize,
    },
    /// Context has been compacted to free up space.
    ContextCompacted {
        session_id: String,
        /// Number of conversation entries removed during compaction.
        #[serde(default)]
        entries_removed: usize,
        /// Usage ratio after compaction (0.0 if unknown).
        #[serde(default)]
        usage_ratio: f64,
    },

    // ── Terminal states ──
    /// An error occurred during Agent operation.
    Error {
        session_id: String,
        message: String,
        recoverable: bool,
    },
    /// Agent has completed its current task.
    Completed {
        session_id: String,
        result: AgentResult,
    },
    /// Agent has been stopped.
    Stopped,
}

/// Result returned when an Agent completes a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    /// The final response text from the Agent.
    pub response_text: String,
    /// Tool calls that were made during the task.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Token usage statistics.
    pub usage: UsageInfo,
    /// Estimated cost in USD.
    #[serde(default)]
    pub cost: Option<f64>,
}

/// Information about a single tool call invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Name of the tool that was invoked.
    pub name: String,
    /// Input parameters passed to the tool.
    pub input: serde_json::Value,
    /// Output returned by the tool.
    pub output: serde_json::Value,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    /// Number of input (prompt) tokens consumed.
    #[serde(default)]
    pub input_tokens: u64,
    /// Number of output (completion) tokens generated.
    #[serde(default)]
    pub output_tokens: u64,
    /// Number of tokens read from cache.
    #[serde(default)]
    pub cache_read: u64,
    /// Number of tokens written to cache.
    #[serde(default)]
    pub cache_write: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentCapability, AgentStatus};

    #[test]
    fn event_started_roundtrip() {
        let mut caps = std::collections::HashSet::new();
        caps.insert(AgentCapability::Streaming);
        let info = AgentInfo {
            name: "Remote Code".into(),
            version: "0.1.0".into(),
            capabilities: caps,
            status: AgentStatus::Ready,
        };
        let event = UnifiedAgentEvent::Started(info);
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::Started(info) => {
                assert_eq!(info.name, "Remote Code");
                assert_eq!(info.version, "0.1.0");
            }
            other => panic!("expected Started, got {other:?}"),
        }
    }

    #[test]
    fn event_message_delta_roundtrip() {
        let event = UnifiedAgentEvent::MessageDelta {
            session_id: "sess-123".into(),
            delta: "Hello, ".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::MessageDelta { session_id, delta } => {
                assert_eq!(session_id, "sess-123");
                assert_eq!(delta, "Hello, ");
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn event_tool_call_started_roundtrip() {
        let event = UnifiedAgentEvent::ToolCallStarted {
            session_id: "sess-456".into(),
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({"path": "/tmp/test.rs"}),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::ToolCallStarted {
                session_id,
                tool_name,
                tool_input,
            } => {
                assert_eq!(session_id, "sess-456");
                assert_eq!(tool_name, "read_file");
                assert_eq!(tool_input["path"], "/tmp/test.rs");
            }
            other => panic!("expected ToolCallStarted, got {other:?}"),
        }
    }

    #[test]
    fn codex_app_server_notification_roundtrip() {
        let event = UnifiedAgentEvent::CodexAppServerNotification {
            session_id: "sess-codex".into(),
            method: "model/verification".into(),
            params: serde_json::json!({"model": "gpt-5"}),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::CodexAppServerNotification {
                session_id,
                method,
                params,
            } => {
                assert_eq!(session_id, "sess-codex");
                assert_eq!(method, "model/verification");
                assert_eq!(params["model"], "gpt-5");
            }
            other => panic!("expected CodexAppServerNotification, got {other:?}"),
        }
    }

    #[test]
    fn event_error_roundtrip() {
        let event = UnifiedAgentEvent::Error {
            session_id: "sess-789".into(),
            message: "timeout".into(),
            recoverable: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::Error {
                session_id,
                message,
                recoverable,
            } => {
                assert_eq!(session_id, "sess-789");
                assert_eq!(message, "timeout");
                assert!(recoverable);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn event_completed_with_result() {
        let result = AgentResult {
            response_text: "Done!".into(),
            tool_calls: vec![ToolCallInfo {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
                output: serde_json::json!({"stdout": "file1.rs\nfile2.rs"}),
            }],
            usage: UsageInfo {
                input_tokens: 100,
                output_tokens: 50,
                cache_read: 30,
                cache_write: 0,
            },
            cost: Some(0.003),
        };
        let event = UnifiedAgentEvent::Completed {
            session_id: "sess-abc".into(),
            result,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::Completed { session_id, result } => {
                assert_eq!(session_id, "sess-abc");
                assert_eq!(result.response_text, "Done!");
                assert_eq!(result.tool_calls.len(), 1);
                assert_eq!(result.tool_calls[0].name, "bash");
                assert_eq!(result.usage.input_tokens, 100);
                assert_eq!(result.cost, Some(0.003));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn event_context_usage_roundtrip() {
        let event = UnifiedAgentEvent::ContextUsage {
            session_id: "s1".into(),
            used: 80_000,
            total: 200_000,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: UnifiedAgentEvent = serde_json::from_str(&json).expect("deserialize");

        match back {
            UnifiedAgentEvent::ContextUsage { used, total, .. } => {
                assert_eq!(used, 80_000);
                assert_eq!(total, 200_000);
            }
            other => panic!("expected ContextUsage, got {other:?}"),
        }
    }

    #[test]
    fn usage_info_default() {
        let usage = UsageInfo::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
    }

    #[test]
    fn event_tagged_serialization() {
        // Verify the #[serde(tag = "type")] produces the expected JSON shape.
        let event = UnifiedAgentEvent::Ready;
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(
            json.contains("\"type\":\"ready\""),
            "unexpected JSON: {json}"
        );
    }
}
