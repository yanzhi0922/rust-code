//! Bridge from [`UnifiedAgentEvent`] → [`RuntimeEventDetail`].
//!
//! This allows any Agent adapter (Roo, Codex, or future ones) to feed events
//! into the same control-plane timeline that Claude sessions already use.

use std::sync::Arc;

use rc_engine_events::{MessageRole, RuntimeEventDetail};

use crate::events::UnifiedAgentEvent;

/// Convert a [`UnifiedAgentEvent`] into a [`RuntimeEventDetail`] suitable for
/// posting to the control plane timeline. Returns `None` for lifecycle events
/// that have no timeline representation (Started, Ready, Stopped, Completed).
pub fn unified_event_to_runtime_detail(event: &UnifiedAgentEvent) -> Option<RuntimeEventDetail> {
    match event {
        UnifiedAgentEvent::MessageDelta { delta, .. } => Some(RuntimeEventDetail::MessageDelta {
            role: MessageRole::Assistant,
            delta: delta.clone(),
            message_id: None,
        }),

        UnifiedAgentEvent::ToolCallStarted {
            tool_name,
            tool_input,
            ..
        } => Some(RuntimeEventDetail::ToolStarted {
            tool_call_id: derive_tool_call_id(tool_name, tool_input).into(),
            tool_name: Arc::from(tool_name.as_str()),
        }),

        UnifiedAgentEvent::ToolCallProgress {
            tool_name,
            progress,
            ..
        } => {
            let name: Arc<str> = Arc::from(tool_name.as_str());
            Some(RuntimeEventDetail::ToolProgress {
                tool_call_id: Some(name.clone()),
                tool_name: Some(name),
                delta: Some(progress.clone()),
                elapsed_time_seconds: None,
            })
        }

        UnifiedAgentEvent::ToolCallCompleted {
            tool_name, result, ..
        } => {
            let is_error = result
                .get("success")
                .and_then(|v| v.as_bool())
                .is_none_or(|s| !s);
            Some(RuntimeEventDetail::ToolFinished {
                tool_call_id: derive_tool_call_id(tool_name, result).into(),
                tool_name: Arc::from(tool_name.as_str()),
                is_error,
                summary: Some(result.to_string()),
            })
        }

        UnifiedAgentEvent::PermissionRequest { tool_name, .. } => {
            let name: Arc<str> = Arc::from(tool_name.as_str());
            Some(RuntimeEventDetail::ToolStarted {
                tool_call_id: format!("approval-{name}").into(),
                tool_name: format!("approval:{name}").into(),
            })
        }

        UnifiedAgentEvent::SubtaskStarted {
            task_id,
            description,
            ..
        } => Some(RuntimeEventDetail::SubtaskStarted {
            task_id: Arc::from(task_id.as_str()),
            parent_task_id: None,
            description: description.clone(),
            depth: 0,
        }),

        UnifiedAgentEvent::SubtaskProgress {
            task_id, progress, ..
        } => Some(RuntimeEventDetail::SubtaskProgress {
            task_id: Arc::from(task_id.as_str()),
            status: "running".to_owned(),
            summary: progress.clone(),
        }),

        UnifiedAgentEvent::SubtaskCompleted {
            task_id, result, ..
        } => Some(RuntimeEventDetail::SubtaskCompleted {
            task_id: Arc::from(task_id.as_str()),
            status: "completed".to_owned(),
            summary: result.to_string(),
            turns_used: None,
        }),

        UnifiedAgentEvent::ContextUsage { used, total, .. } => {
            let ratio = if *total > 0 {
                *used as f64 / *total as f64
            } else {
                0.0
            };
            Some(RuntimeEventDetail::ContextUsage {
                estimated_tokens: *used as u64,
                max_input_tokens: *total as u64,
                threshold_tokens: (*total as f64 * 0.8) as u64,
                ratio,
            })
        }

        UnifiedAgentEvent::ContextOverflow { used, total, .. } => {
            let ratio = if *total > 0 {
                *used as f64 / *total as f64
            } else {
                1.0
            };
            Some(RuntimeEventDetail::ContextOverflow {
                estimated_tokens: *used as u64,
                max_input_tokens: *total as u64,
                threshold_tokens: (*total as f64 * 0.8) as u64,
                ratio,
            })
        }

        UnifiedAgentEvent::ContextCompacted {
            entries_removed,
            usage_ratio,
            ..
        } => Some(RuntimeEventDetail::ContextCompacted {
            entries_removed: *entries_removed as u32,
            usage_ratio: *usage_ratio,
        }),

        UnifiedAgentEvent::Error { message, .. } => Some(RuntimeEventDetail::RuntimeError {
            message: message.clone(),
        }),

        // Lifecycle events with no timeline representation
        UnifiedAgentEvent::Started(_)
        | UnifiedAgentEvent::Ready
        | UnifiedAgentEvent::Stopped
        | UnifiedAgentEvent::Completed { .. } => None,

        // Codex-specific notifications are opaque; pass through as runtime error
        // with structured metadata so the timeline shows something useful.
        UnifiedAgentEvent::CodexAppServerNotification { method, params, .. } => {
            Some(RuntimeEventDetail::RuntimeError {
                message: format!("[codex:{method}] {params}"),
            })
        }
    }
}

/// Derive a stable tool call ID from the tool name and its input.
/// Uses a hash of the input to disambiguate multiple calls to the same tool.
fn derive_tool_call_id(tool_name: &str, input: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.to_string().hash(&mut hasher);
    let hash = hasher.finish();
    format!("{tool_name}-{hash:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_delta_maps_correctly() {
        let event = UnifiedAgentEvent::MessageDelta {
            session_id: "s1".into(),
            delta: "hello".into(),
        };
        let detail = unified_event_to_runtime_detail(&event).unwrap();
        assert!(matches!(
            detail,
            RuntimeEventDetail::MessageDelta { role: MessageRole::Assistant, delta, .. }
            if delta == "hello"
        ));
    }

    #[test]
    fn tool_lifecycle_maps_correctly() {
        let started = UnifiedAgentEvent::ToolCallStarted {
            session_id: "s1".into(),
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({"path": "/tmp/a.rs"}),
        };
        let d = unified_event_to_runtime_detail(&started).unwrap();
        assert!(
            matches!(d, RuntimeEventDetail::ToolStarted { tool_name, .. } if &*tool_name == "read_file")
        );

        let completed = UnifiedAgentEvent::ToolCallCompleted {
            session_id: "s1".into(),
            tool_name: "read_file".into(),
            result: serde_json::json!({"success": true, "content": "ok"}),
        };
        let d = unified_event_to_runtime_detail(&completed).unwrap();
        assert!(matches!(
            d,
            RuntimeEventDetail::ToolFinished {
                is_error: false,
                ..
            }
        ));
    }

    #[test]
    fn lifecycle_events_map_to_none() {
        assert!(unified_event_to_runtime_detail(&UnifiedAgentEvent::Ready).is_none());
        assert!(unified_event_to_runtime_detail(&UnifiedAgentEvent::Stopped).is_none());
        assert!(
            unified_event_to_runtime_detail(&UnifiedAgentEvent::Started(crate::types::AgentInfo {
                name: "test".into(),
                version: "0.1".into(),
                capabilities: Default::default(),
                status: crate::types::AgentStatus::Ready,
            }))
            .is_none()
        );
    }

    #[test]
    fn context_usage_maps_with_ratio() {
        let event = UnifiedAgentEvent::ContextUsage {
            session_id: "s1".into(),
            used: 80_000,
            total: 200_000,
        };
        let detail = unified_event_to_runtime_detail(&event).unwrap();
        if let RuntimeEventDetail::ContextUsage { ratio, .. } = detail {
            assert!((ratio - 0.4).abs() < 0.01);
        } else {
            panic!("expected ContextUsage");
        }
    }

    #[test]
    fn subtask_lifecycle_maps() {
        let started = UnifiedAgentEvent::SubtaskStarted {
            session_id: "s1".into(),
            task_id: "t1".into(),
            description: "explore code".into(),
        };
        let d = unified_event_to_runtime_detail(&started).unwrap();
        assert!(
            matches!(d, RuntimeEventDetail::SubtaskStarted { task_id, description, .. }
            if &*task_id == "t1" && description == "explore code")
        );

        let completed = UnifiedAgentEvent::SubtaskCompleted {
            session_id: "s1".into(),
            task_id: "t1".into(),
            result: serde_json::json!("done"),
        };
        let d = unified_event_to_runtime_detail(&completed).unwrap();
        assert!(
            matches!(d, RuntimeEventDetail::SubtaskCompleted { status, .. } if status == "completed")
        );
    }
}
