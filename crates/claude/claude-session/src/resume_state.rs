use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Persisted snapshot of an unfinished tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Resume metadata reconstructed after an interrupted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeState {
    pub updated_at: DateTime<Utc>,
    pub pending_tool_calls: Vec<PendingToolCall>,
}

impl ResumeState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            updated_at: Utc::now(),
            pending_tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_pending_calls(pending_tool_calls: Vec<PendingToolCall>) -> Self {
        Self {
            updated_at: Utc::now(),
            pending_tool_calls,
        }
    }
}
