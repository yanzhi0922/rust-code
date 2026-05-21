//! Permission synchronization via file system.
//!
//! Workers write permission requests to the team's permissions directory.
//! The lead agent reads and resolves them, writing responses back.
//!
//! Directory structure:
//! ```text
//! ~/.remote-code/teams/<team>/permissions/
//!   <request_id>.req.json   — request from worker
//!   <request_id>.resp.json  — response from lead
//! ```

use std::path::PathBuf;

use tokio::fs;

use crate::constants::{
    PERMISSION_POLL_INTERVAL_MS, PERMISSION_REQUEST_EXT, PERMISSION_REQUEST_TIMEOUT_SECS,
    PERMISSION_RESPONSE_EXT, PERMISSIONS_DIR_NAME,
};
use crate::error::{SwarmError, SwarmResult};
use crate::team_helpers::team_dir;
use crate::types::{PermissionDecision, SwarmPermissionRequest};

/// Get the permissions directory for a team.
fn permissions_dir(team_name: &str) -> PathBuf {
    team_dir(team_name).join(PERMISSIONS_DIR_NAME)
}

/// Get the request file path.
fn request_file_path(team_name: &str, request_id: &str) -> PathBuf {
    permissions_dir(team_name).join(format!("{}{}", request_id, PERMISSION_REQUEST_EXT))
}

/// Get the response file path.
fn response_file_path(team_name: &str, request_id: &str) -> PathBuf {
    permissions_dir(team_name).join(format!("{}{}", request_id, PERMISSION_RESPONSE_EXT))
}

/// Write a permission request to the file system.
pub async fn write_request(team_name: &str, request: &SwarmPermissionRequest) -> SwarmResult<()> {
    let dir = permissions_dir(team_name);
    fs::create_dir_all(&dir).await?;
    let path = request_file_path(team_name, &request.request_id);
    let json = serde_json::to_string_pretty(request)?;
    fs::write(&path, json).await?;
    Ok(())
}

/// Read a permission request from the file system.
pub async fn read_request(
    team_name: &str,
    request_id: &str,
) -> SwarmResult<SwarmPermissionRequest> {
    let path = request_file_path(team_name, request_id);
    if !path.exists() {
        return Err(SwarmError::PermissionRequestNotFound(request_id.to_owned()));
    }
    let content = fs::read_to_string(&path).await?;
    let request: SwarmPermissionRequest = serde_json::from_str(&content)?;
    Ok(request)
}

/// Write a permission response (decision) to the file system.
pub async fn write_response(
    team_name: &str,
    request_id: &str,
    decision: PermissionDecision,
    reason: Option<String>,
) -> SwarmResult<()> {
    let dir = permissions_dir(team_name);
    fs::create_dir_all(&dir).await?;

    // Read and update the request.
    let mut request = read_request(team_name, request_id).await?;
    request.resolve(decision, reason);

    // Write the response file.
    let resp_path = response_file_path(team_name, request_id);
    let json = serde_json::to_string_pretty(&request)?;
    fs::write(&resp_path, json).await?;

    Ok(())
}

/// Read a permission response from the file system.
pub async fn read_response(
    team_name: &str,
    request_id: &str,
) -> SwarmResult<SwarmPermissionRequest> {
    let path = response_file_path(team_name, request_id);
    if !path.exists() {
        return Err(SwarmError::PermissionRequestNotFound(format!(
            "response for {request_id}"
        )));
    }
    let content = fs::read_to_string(&path).await?;
    let request: SwarmPermissionRequest = serde_json::from_str(&content)?;
    Ok(request)
}

/// Wait for a permission response with timeout.
///
/// Polls the file system for the response file.
pub async fn wait_for_response(
    team_name: &str,
    request_id: &str,
) -> SwarmResult<SwarmPermissionRequest> {
    let timeout = std::time::Duration::from_secs(PERMISSION_REQUEST_TIMEOUT_SECS);
    let interval = std::time::Duration::from_millis(PERMISSION_POLL_INTERVAL_MS);
    let start = std::time::Instant::now();

    loop {
        match read_response(team_name, request_id).await {
            Ok(response) => return Ok(response),
            Err(SwarmError::PermissionRequestNotFound(_)) => {
                if start.elapsed() >= timeout {
                    return Err(SwarmError::PermissionTimeout {
                        request_id: request_id.to_owned(),
                    });
                }
                tokio::time::sleep(interval).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// List all pending (unresolved) permission requests.
pub async fn list_pending_requests(team_name: &str) -> SwarmResult<Vec<SwarmPermissionRequest>> {
    let dir = permissions_dir(team_name);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(&dir).await?;
    let mut requests = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string())
            && name.ends_with(PERMISSION_REQUEST_EXT)
        {
            let content = fs::read_to_string(&path).await?;
            let req: SwarmPermissionRequest = serde_json::from_str(&content)?;
            if !req.is_resolved() {
                requests.push(req);
            }
        }
    }

    Ok(requests)
}

/// Clean up old permission files.
pub async fn cleanup_permissions(team_name: &str, older_than_secs: i64) -> SwarmResult<usize> {
    let dir = permissions_dir(team_name);
    if !dir.exists() {
        return Ok(0);
    }

    let now = chrono::Utc::now().timestamp();
    let mut entries = fs::read_dir(&dir).await?;
    let mut removed = 0;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string())
            && (name.ends_with(PERMISSION_REQUEST_EXT) || name.ends_with(PERMISSION_RESPONSE_EXT))
        {
            let content = fs::read_to_string(&path).await?;
            if let Ok(req) = serde_json::from_str::<SwarmPermissionRequest>(&content)
                && now - req.created_at > older_than_secs
            {
                let _ = fs::remove_file(&path).await;
                removed += 1;
            }
        }
    }

    Ok(removed)
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
    async fn write_and_read_request() {
        let _td = TestDir::new();
        let request = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        write_request("test-team", &request)
            .await
            .expect("should write");
        let read = read_request("test-team", &request.request_id)
            .await
            .expect("should read");
        assert_eq!(read.request_id, request.request_id);
        assert_eq!(read.agent_name, "worker-1");
    }

    #[tokio::test]
    async fn read_nonexistent_request() {
        let _td = TestDir::new();
        let result = read_request("test-team", "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_and_read_response() {
        let _td = TestDir::new();
        let request = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        write_request("test-team", &request)
            .await
            .expect("should write");
        write_response(
            "test-team",
            &request.request_id,
            PermissionDecision::Allow,
            None,
        )
        .await
        .expect("should write response");

        let response = read_response("test-team", &request.request_id)
            .await
            .expect("should read response");
        assert_eq!(response.decision, Some(PermissionDecision::Allow));
        assert!(response.is_resolved());
    }

    #[tokio::test]
    async fn write_response_with_reason() {
        let _td = TestDir::new();
        let request = SwarmPermissionRequest::new(
            "test-team",
            "worker-1",
            "bash",
            serde_json::json!({"command": "rm -rf /"}),
        );
        write_request("test-team", &request)
            .await
            .expect("should write");
        write_response(
            "test-team",
            &request.request_id,
            PermissionDecision::Deny,
            Some("dangerous command".to_owned()),
        )
        .await
        .expect("should write response");

        let response = read_response("test-team", &request.request_id)
            .await
            .expect("should read response");
        assert_eq!(response.decision, Some(PermissionDecision::Deny));
        assert_eq!(response.reason.as_deref(), Some("dangerous command"));
    }

    #[tokio::test]
    async fn read_nonexistent_response() {
        let _td = TestDir::new();
        let result = read_response("test-team", "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_pending_requests() {
        let _td = TestDir::new();
        let req1 = SwarmPermissionRequest::new("test-team", "w1", "bash", serde_json::json!({}));
        let req2 = SwarmPermissionRequest::new("test-team", "w2", "bash", serde_json::json!({}));
        write_request("test-team", &req1).await.expect("ok");
        write_request("test-team", &req2).await.expect("ok");

        let pending = list_pending_requests("test-team")
            .await
            .expect("should list");
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn test_list_pending_requests_empty() {
        let _td = TestDir::new();
        let pending = list_pending_requests("test-team")
            .await
            .expect("should list");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_old_permissions() {
        let _td = TestDir::new();
        let mut request =
            SwarmPermissionRequest::new("test-team", "worker-1", "bash", serde_json::json!({}));
        // Set created_at to far past so it's definitely old enough.
        request.created_at = 0;
        write_request("test-team", &request).await.expect("ok");

        // Clean up files older than 0 seconds (everything with created_at=0).
        let removed = cleanup_permissions("test-team", 0)
            .await
            .expect("should cleanup");
        assert_eq!(removed, 1);
    }

    #[tokio::test]
    async fn test_cleanup_permissions_no_dir() {
        let _td = TestDir::new();
        let removed = cleanup_permissions("nonexistent-team", 0)
            .await
            .expect("ok");
        assert_eq!(removed, 0);
    }

    #[test]
    fn permissions_dir_contains_name() {
        let _td = TestDir::new();
        let dir = permissions_dir("my-team");
        assert!(dir.to_string_lossy().contains("permissions"));
    }

    #[test]
    fn request_file_path_format() {
        let _td = TestDir::new();
        let path = request_file_path("my-team", "req-123");
        assert!(path.to_string_lossy().ends_with("req-123.req.json"));
    }

    #[test]
    fn response_file_path_format() {
        let _td = TestDir::new();
        let path = response_file_path("my-team", "req-123");
        assert!(path.to_string_lossy().ends_with("req-123.resp.json"));
    }
}
