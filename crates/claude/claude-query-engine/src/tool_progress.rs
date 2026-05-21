//! Tool progress streaming data for real-time execution monitoring.
//!
//! These types represent progress events emitted during tool execution,
//! enabling UIs to show progress bars, status messages, and partial results.

use serde::{Deserialize, Serialize};

use crate::config::ToolRunResult;

/// A progress event emitted during tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolProgressEvent {
    /// A tool has started executing.
    Started {
        tool_call_id: String,
        tool_name: String,
    },
    /// A tool has reported progress.
    Progress {
        tool_call_id: String,
        progress: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A tool has completed successfully.
    Completed {
        tool_call_id: String,
        result: ToolProgressResult,
    },
    /// A tool has failed.
    Failed { tool_call_id: String, error: String },
}

impl ToolProgressEvent {
    /// Returns the tool call ID associated with this event.
    #[must_use]
    pub fn tool_call_id(&self) -> &str {
        match self {
            Self::Started { tool_call_id, .. }
            | Self::Progress { tool_call_id, .. }
            | Self::Completed { tool_call_id, .. }
            | Self::Failed { tool_call_id, .. } => tool_call_id,
        }
    }

    /// Returns true if this event represents a terminal state (completed or failed).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

/// A simplified tool result for progress events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgressResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolProgressResult {
    /// Create a successful result.
    #[must_use]
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result.
    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

impl From<&ToolRunResult> for ToolProgressResult {
    fn from(result: &ToolRunResult) -> Self {
        Self {
            content: result.result.content.clone(),
            is_error: result.result.is_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolProgressEvent, ToolProgressResult};

    #[test]
    fn started_event_extracts_tool_call_id() {
        let event = ToolProgressEvent::Started {
            tool_call_id: "tc-1".to_string(),
            tool_name: "bash".to_string(),
        };
        assert_eq!(event.tool_call_id(), "tc-1");
        assert!(!event.is_terminal());
    }

    #[test]
    fn progress_event_with_message() {
        let event = ToolProgressEvent::Progress {
            tool_call_id: "tc-2".to_string(),
            progress: 0.5,
            message: Some("halfway".to_string()),
        };
        assert_eq!(event.tool_call_id(), "tc-2");
        assert!(!event.is_terminal());
    }

    #[test]
    fn completed_event_is_terminal() {
        let event = ToolProgressEvent::Completed {
            tool_call_id: "tc-3".to_string(),
            result: ToolProgressResult::success("done"),
        };
        assert!(event.is_terminal());
    }

    #[test]
    fn failed_event_is_terminal() {
        let event = ToolProgressEvent::Failed {
            tool_call_id: "tc-4".to_string(),
            error: "timeout".to_string(),
        };
        assert!(event.is_terminal());
    }

    #[test]
    fn tool_progress_result_success() {
        let result = ToolProgressResult::success("output");
        assert_eq!(result.content, "output");
        assert!(!result.is_error);
    }

    #[test]
    fn tool_progress_result_error() {
        let result = ToolProgressResult::error("something went wrong");
        assert!(result.is_error);
    }
}
