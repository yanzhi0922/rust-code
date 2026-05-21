//! Leader permission bridge.
//!
//! The lead agent acts as a bridge for permission requests from workers.
//! Workers write permission requests to the file system, and the lead
//! reads, evaluates, and responds to them.

use crate::error::SwarmResult;
use crate::permission_sync;
use crate::types::{PermissionDecision, SwarmPermissionRequest};

/// Process all pending permission requests as the team lead.
///
/// For each pending request, applies the given decision function
/// and writes the response.
pub async fn process_pending_requests<F>(
    team_name: &str,
    decide: F,
) -> SwarmResult<Vec<SwarmPermissionRequest>>
where
    F: Fn(&SwarmPermissionRequest) -> (PermissionDecision, Option<String>),
{
    let pending = permission_sync::list_pending_requests(team_name).await?;
    let mut resolved = Vec::new();

    for request in &pending {
        let (decision, reason) = decide(request);
        permission_sync::write_response(team_name, &request.request_id, decision, reason.clone())
            .await?;

        let mut resolved_req = request.clone();
        resolved_req.resolve(decision, reason);
        resolved.push(resolved_req);
    }

    Ok(resolved)
}

/// Auto-approve a permission request.
pub async fn auto_approve(team_name: &str, request_id: &str) -> SwarmResult<()> {
    permission_sync::write_response(team_name, request_id, PermissionDecision::Allow, None).await
}

/// Auto-deny a permission request with a reason.
pub async fn auto_deny(team_name: &str, request_id: &str, reason: &str) -> SwarmResult<()> {
    permission_sync::write_response(
        team_name,
        request_id,
        PermissionDecision::Deny,
        Some(reason.to_owned()),
    )
    .await
}

/// Check if a specific tool should be auto-approved.
///
/// Read-only tools are auto-approved; everything else requires
/// explicit decision.
#[must_use]
pub fn should_auto_approve(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read" | "search" | "glob" | "list" | "info" | "status"
    )
}

/// Default permission decision logic.
///
/// Auto-approves read-only tools, denies dangerous patterns,
/// and asks for everything else.
#[must_use]
pub fn default_decision(request: &SwarmPermissionRequest) -> (PermissionDecision, Option<String>) {
    if should_auto_approve(&request.tool_name) {
        (PermissionDecision::Allow, None)
    } else {
        // In a real implementation, this would prompt the user.
        // For now, deny with a reason.
        (
            PermissionDecision::Deny,
            Some(format!(
                "manual approval required for {}",
                request.tool_name
            )),
        )
    }
}

/// Bridge status for monitoring.
#[derive(Debug, Clone)]
pub struct BridgeStatus {
    /// Number of pending requests.
    pub pending_count: usize,
    /// Number of requests processed in this session.
    pub processed_count: usize,
    /// Whether the bridge is active.
    pub is_active: bool,
}

impl Default for BridgeStatus {
    fn default() -> Self {
        Self {
            pending_count: 0,
            processed_count: 0,
            is_active: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_helpers::set_base_dir_override;

    struct TestDir {
        _temp: tempfile::TempDir,
    }

    impl TestDir {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let path = temp.path().to_path_buf();
            set_base_dir_override(Some(path));
            Self { _temp: temp }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            set_base_dir_override(None);
        }
    }

    #[tokio::test]
    async fn process_pending_with_auto_approve() {
        let _td = TestDir::new();
        let req = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "read",
            serde_json::json!({"path": "/tmp"}),
        );
        permission_sync::write_request("test-team", &req)
            .await
            .expect("ok");

        let resolved = process_pending_requests("test-team", |r| {
            if should_auto_approve(&r.tool_name) {
                (PermissionDecision::Allow, None)
            } else {
                (PermissionDecision::Deny, Some("not approved".to_owned()))
            }
        })
        .await
        .expect("should process");

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].decision, Some(PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn process_pending_empty() {
        let _td = TestDir::new();
        let resolved =
            process_pending_requests("test-team", |_r| (PermissionDecision::Allow, None))
                .await
                .expect("ok");
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn auto_approve_request() {
        let _td = TestDir::new();
        let req = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        permission_sync::write_request("test-team", &req)
            .await
            .expect("ok");

        auto_approve("test-team", &req.request_id)
            .await
            .expect("should approve");

        let resp = permission_sync::read_response("test-team", &req.request_id)
            .await
            .expect("should read");
        assert_eq!(resp.decision, Some(PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn auto_deny_request() {
        let _td = TestDir::new();
        let req = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "bash",
            serde_json::json!({"command": "rm -rf /"}),
        );
        permission_sync::write_request("test-team", &req)
            .await
            .expect("ok");

        auto_deny("test-team", &req.request_id, "dangerous command")
            .await
            .expect("should deny");

        let resp = permission_sync::read_response("test-team", &req.request_id)
            .await
            .expect("should read");
        assert_eq!(resp.decision, Some(PermissionDecision::Deny));
        assert_eq!(resp.reason.as_deref(), Some("dangerous command"));
    }

    #[test]
    fn should_auto_approve_read_tools() {
        assert!(should_auto_approve("read"));
        assert!(should_auto_approve("search"));
        assert!(should_auto_approve("glob"));
        assert!(should_auto_approve("list"));
        assert!(should_auto_approve("info"));
        assert!(should_auto_approve("status"));
    }

    #[test]
    fn should_not_auto_approve_write_tools() {
        assert!(!should_auto_approve("write"));
        assert!(!should_auto_approve("bash"));
        assert!(!should_auto_approve("edit"));
        assert!(!should_auto_approve("delete"));
    }

    #[test]
    fn default_decision_read_tool() {
        let req = SwarmPermissionRequest::new("team", "worker", "read", serde_json::json!({}));
        let (decision, _) = default_decision(&req);
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn default_decision_write_tool() {
        let req = SwarmPermissionRequest::new("team", "worker", "write", serde_json::json!({}));
        let (decision, reason) = default_decision(&req);
        assert_eq!(decision, PermissionDecision::Deny);
        assert!(reason.is_some());
    }

    #[test]
    fn bridge_status_default() {
        let status = BridgeStatus::default();
        assert_eq!(status.pending_count, 0);
        assert_eq!(status.processed_count, 0);
        assert!(status.is_active);
    }
}
