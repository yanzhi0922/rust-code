//! Permission-related types for Agent interactions.
//!
//! When an Agent needs user approval for an operation (e.g. running a shell
//! command, writing a file), it emits a [`PermissionRequest`]. The user (or
//! an automated policy) responds with a [`PermissionDecision`].

use serde::{Deserialize, Serialize};

/// User's decision in response to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow this specific operation.
    Allow,
    /// Deny this specific operation.
    Deny,
    /// Allow this operation and all future operations of the same kind.
    AllowAll,
}

/// A permission request from an Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Unique identifier for this request.
    pub request_id: String,
    /// Session that produced this request.
    pub session_id: String,
    /// Name of the tool or operation requiring permission.
    pub tool_name: String,
    /// The input that would be passed to the tool.
    pub input: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_decision_serde_roundtrip() {
        let decisions = [
            PermissionDecision::Allow,
            PermissionDecision::Deny,
            PermissionDecision::AllowAll,
        ];
        for d in &decisions {
            let json = serde_json::to_string(d).expect("serialize");
            let back: PermissionDecision = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*d, back);
        }
    }

    #[test]
    fn permission_decision_values() {
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Allow).expect("s"),
            "\"allow\""
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Deny).expect("s"),
            "\"deny\""
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::AllowAll).expect("s"),
            "\"allow_all\""
        );
    }

    #[test]
    fn permission_request_serde() {
        let req = PermissionRequest {
            request_id: "req-001".into(),
            session_id: "sess-123".into(),
            tool_name: "bash".into(),
            input: serde_json::json!({"command": "rm -rf /tmp/test"}),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let back: PermissionRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(req.request_id, back.request_id);
        assert_eq!(req.session_id, back.session_id);
        assert_eq!(req.tool_name, back.tool_name);
    }
}
