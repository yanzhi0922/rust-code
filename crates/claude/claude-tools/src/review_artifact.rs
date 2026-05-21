//! Artifact review tool: review_artifact.
//!
//! Provides tools for reviewing artifacts in a remote architecture context.
//! Supports diff viewing, comment addition, and status updates.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolExecutionContext;

/// Review status for an artifact.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Review has not started.
    Pending,
    /// Review is in progress.
    InProgress,
    /// Review is approved.
    Approved,
    /// Changes are requested.
    ChangesRequested,
    /// Review is rejected.
    Rejected,
}

impl ReviewStatus {
    /// Parse a review status from a string.
    ///
    /// # Errors
    /// Returns an error if the string is not a valid status.
    pub fn from_str_lossy(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "in_progress" | "in-progress" => Ok(Self::InProgress),
            "approved" => Ok(Self::Approved),
            "changes_requested" | "changes-requested" => Ok(Self::ChangesRequested),
            "rejected" => Ok(Self::Rejected),
            _ => Err(anyhow!("invalid review status: '{s}'")),
        }
    }
}

/// A review comment on an artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewComment {
    /// Unique comment identifier.
    pub id: String,
    /// Author of the comment.
    pub author: String,
    /// Comment text.
    pub text: String,
    /// Unix timestamp (milliseconds).
    pub timestamp: i64,
    /// Optional file path the comment refers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Optional line number the comment refers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Comment severity level.
    #[serde(default)]
    pub severity: CommentSeverity,
}

/// Severity level for review comments.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommentSeverity {
    /// Informational comment.
    #[default]
    Info,
    /// Minor suggestion.
    Suggestion,
    /// Important issue.
    Warning,
    /// Critical issue that must be addressed.
    Critical,
}

/// Review an artifact.
///
/// Supports multiple actions:
/// - `view_diff`: View the diff of an artifact
/// - `add_comment`: Add a review comment
/// - `update_status`: Update the review status
/// - `get_comments`: Get all comments for an artifact
/// - `summary`: Get a summary of the review state
///
/// # Errors
/// Returns an error if required parameters are missing or invalid.
pub fn review_artifact(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let action = input["action"].as_str().ok_or_else(|| {
        anyhow!("action is required (view_diff, add_comment, update_status, get_comments, summary)")
    })?;

    let artifact_id = input["artifact_id"]
        .as_str()
        .ok_or_else(|| anyhow!("artifact_id is required"))?;

    if artifact_id.trim().is_empty() {
        return Err(anyhow!("artifact_id cannot be empty"));
    }

    match action {
        "view_diff" => view_diff(artifact_id, input, context),
        "add_comment" => add_comment(artifact_id, input),
        "update_status" => update_status(artifact_id, input),
        "get_comments" => get_comments(artifact_id),
        "summary" => review_summary(artifact_id),
        _ => Err(anyhow!(
            "unknown action: '{action}'. Valid actions: view_diff, add_comment, update_status, get_comments, summary"
        )),
    }
}

/// View the diff of an artifact.
fn view_diff(artifact_id: &str, input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let from_version = input["from_version"].as_str().unwrap_or("HEAD~1");
    let to_version = input["to_version"].as_str().unwrap_or("HEAD");

    // Try to get actual diff from git.
    let diff_output = std::process::Command::new("git")
        .args(["diff", from_version, to_version, "--stat"])
        .current_dir(&context.cwd)
        .output();

    let diff_stat = match diff_output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => "Unable to retrieve diff. Git may not be available.".to_string(),
    };

    Ok(json!({
        "type": "review_diff",
        "artifact_id": artifact_id,
        "from_version": from_version,
        "to_version": to_version,
        "diff_stat": diff_stat.trim(),
        "message": format!("Diff for artifact '{artifact_id}' ({from_version}..{to_version})")
    })
    .to_string())
}

/// Add a review comment to an artifact.
fn add_comment(artifact_id: &str, input: &Value) -> Result<String> {
    let text = input["comment"]
        .as_str()
        .ok_or_else(|| anyhow!("comment text is required for add_comment action"))?;

    if text.trim().is_empty() {
        return Err(anyhow!("comment cannot be empty"));
    }

    let author = input["author"].as_str().unwrap_or("reviewer");

    let severity = input["severity"]
        .as_str()
        .map(|s| match s {
            "critical" => CommentSeverity::Critical,
            "warning" => CommentSeverity::Warning,
            "suggestion" => CommentSeverity::Suggestion,
            _ => CommentSeverity::Info,
        })
        .unwrap_or_default();

    let comment = ReviewComment {
        id: format!("comment-{}", uuid::Uuid::new_v4().as_simple()),
        author: author.to_string(),
        text: text.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        file_path: input["file_path"].as_str().map(String::from),
        line: input["line"].as_u64().map(|l| l as u32),
        severity,
    };

    Ok(json!({
        "type": "review_comment_added",
        "artifact_id": artifact_id,
        "comment": {
            "id": comment.id,
            "author": comment.author,
            "text": comment.text,
            "timestamp": comment.timestamp,
            "file_path": comment.file_path,
            "line": comment.line,
            "severity": serde_json::to_value(comment.severity).expect("severity serializes"),
        },
        "message": format!("Comment added to artifact '{artifact_id}'")
    })
    .to_string())
}

/// Update the review status of an artifact.
fn update_status(artifact_id: &str, input: &Value) -> Result<String> {
    let status_str = input["status"]
        .as_str()
        .ok_or_else(|| anyhow!("status is required for update_status action"))?;

    let status = ReviewStatus::from_str_lossy(status_str)?;

    let reviewer = input["reviewer"].as_str().unwrap_or("reviewer");

    Ok(json!({
        "type": "review_status_updated",
        "artifact_id": artifact_id,
        "status": serde_json::to_value(status).expect("status serializes"),
        "reviewer": reviewer,
        "timestamp": chrono::Utc::now().timestamp_millis(),
        "message": format!("Artifact '{artifact_id}' status updated to {status_str} by {reviewer}")
    })
    .to_string())
}

/// Get all comments for an artifact.
fn get_comments(artifact_id: &str) -> Result<String> {
    // In a real implementation, this would fetch from a data store.
    Ok(json!({
        "type": "review_comments",
        "artifact_id": artifact_id,
        "comments": [],
        "total": 0,
        "message": format!("No comments found for artifact '{artifact_id}'. Comments require a persistent review store.")
    })
    .to_string())
}

/// Get a summary of the review state for an artifact.
fn review_summary(artifact_id: &str) -> Result<String> {
    Ok(json!({
        "type": "review_summary",
        "artifact_id": artifact_id,
        "status": "pending",
        "total_comments": 0,
        "critical_comments": 0,
        "warning_comments": 0,
        "reviewers": [],
        "message": format!("Review summary for artifact '{artifact_id}'. No active review found.")
    })
    .to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("/tmp"),
            original_cwd: PathBuf::from("/tmp"),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    #[test]
    fn review_status_from_str_valid() {
        assert_eq!(
            ReviewStatus::from_str_lossy("pending").expect("pending should parse"),
            ReviewStatus::Pending
        );
        assert_eq!(
            ReviewStatus::from_str_lossy("approved").expect("approved should parse"),
            ReviewStatus::Approved
        );
        assert_eq!(
            ReviewStatus::from_str_lossy("rejected").expect("rejected should parse"),
            ReviewStatus::Rejected
        );
        assert_eq!(
            ReviewStatus::from_str_lossy("in_progress").expect("in_progress should parse"),
            ReviewStatus::InProgress
        );
        assert_eq!(
            ReviewStatus::from_str_lossy("changes_requested")
                .expect("changes_requested should parse"),
            ReviewStatus::ChangesRequested
        );
    }

    #[test]
    fn review_status_from_str_invalid() {
        assert!(ReviewStatus::from_str_lossy("unknown").is_err());
    }

    #[test]
    fn review_status_from_str_alternate_formats() {
        assert_eq!(
            ReviewStatus::from_str_lossy("in-progress").expect("in-progress should parse"),
            ReviewStatus::InProgress
        );
        assert_eq!(
            ReviewStatus::from_str_lossy("changes-requested")
                .expect("changes-requested should parse"),
            ReviewStatus::ChangesRequested
        );
    }

    #[test]
    fn review_status_serializes() {
        assert_eq!(
            serde_json::to_string(&ReviewStatus::Approved).expect("review status should serialize"),
            "\"approved\""
        );
    }

    #[test]
    fn comment_severity_default_is_info() {
        assert_eq!(CommentSeverity::default(), CommentSeverity::Info);
    }

    #[test]
    fn review_artifact_requires_action() {
        let input = json!({"artifact_id": "test"});
        let context = test_context();
        let result = review_artifact(&input, &context);
        let error = result.expect_err("missing action should fail");
        assert!(error.to_string().contains("action"));
    }

    #[test]
    fn review_artifact_requires_artifact_id() {
        let input = json!({"action": "summary"});
        let context = test_context();
        let result = review_artifact(&input, &context);
        let error = result.expect_err("missing artifact_id should fail");
        assert!(error.to_string().contains("artifact_id"));
    }

    #[test]
    fn review_artifact_rejects_empty_artifact_id() {
        let input = json!({"action": "summary", "artifact_id": ""});
        let context = test_context();
        let result = review_artifact(&input, &context);
        assert!(result.is_err());
    }

    #[test]
    fn review_artifact_rejects_unknown_action() {
        let input = json!({"action": "unknown_action", "artifact_id": "test"});
        let context = test_context();
        let result = review_artifact(&input, &context);
        let error = result.expect_err("unknown action should fail");
        assert!(error.to_string().contains("unknown action"));
    }

    #[test]
    fn review_summary_returns_pending() {
        let input = json!({"action": "summary", "artifact_id": "my-artifact"});
        let context = test_context();
        let result = review_artifact(&input, &context).expect("summary should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["type"], "review_summary");
        assert_eq!(parsed["status"], "pending");
        assert_eq!(parsed["total_comments"], 0);
    }

    #[test]
    fn add_comment_requires_text() {
        let input = json!({"action": "add_comment", "artifact_id": "test"});
        let context = test_context();
        let result = review_artifact(&input, &context);
        let error = result.expect_err("missing comment should fail");
        assert!(error.to_string().contains("comment"));
    }

    #[test]
    fn add_comment_rejects_empty_text() {
        let input = json!({"action": "add_comment", "artifact_id": "test", "comment": ""});
        let context = test_context();
        let result = review_artifact(&input, &context);
        assert!(result.is_err());
    }

    #[test]
    fn add_comment_returns_comment_id() {
        let input = json!({
            "action": "add_comment",
            "artifact_id": "test-artifact",
            "comment": "This needs improvement",
            "author": "alice",
            "severity": "warning",
            "file_path": "src/main.rs",
            "line": 42
        });
        let context = test_context();
        let result = review_artifact(&input, &context).expect("add_comment should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["type"], "review_comment_added");
        let comment = &parsed["comment"];
        assert!(
            comment["id"]
                .as_str()
                .expect("comment id should be a string")
                .starts_with("comment-")
        );
        assert_eq!(comment["author"], "alice");
        assert_eq!(comment["text"], "This needs improvement");
        assert_eq!(comment["severity"], "warning");
        assert_eq!(comment["file_path"], "src/main.rs");
        assert_eq!(comment["line"], 42);
    }

    #[test]
    fn update_status_requires_status() {
        let input = json!({"action": "update_status", "artifact_id": "test"});
        let context = test_context();
        let result = review_artifact(&input, &context);
        assert!(result.is_err());
    }

    #[test]
    fn update_status_returns_updated_status() {
        let input = json!({
            "action": "update_status",
            "artifact_id": "test-artifact",
            "status": "approved",
            "reviewer": "bob"
        });
        let context = test_context();
        let result = review_artifact(&input, &context).expect("update_status should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["type"], "review_status_updated");
        assert_eq!(parsed["status"], "approved");
        assert_eq!(parsed["reviewer"], "bob");
    }

    #[test]
    fn get_comments_returns_empty() {
        let input = json!({"action": "get_comments", "artifact_id": "test"});
        let context = test_context();
        let result = review_artifact(&input, &context).expect("get_comments should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["type"], "review_comments");
        assert_eq!(parsed["total"], 0);
    }

    #[test]
    fn view_diff_returns_diff_info() {
        let input = json!({
            "action": "view_diff",
            "artifact_id": "test-artifact",
            "from_version": "HEAD~3",
            "to_version": "HEAD"
        });
        let context = test_context();
        let result = review_artifact(&input, &context).expect("view_diff should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["type"], "review_diff");
        assert_eq!(parsed["from_version"], "HEAD~3");
        assert_eq!(parsed["to_version"], "HEAD");
    }

    #[test]
    fn review_comment_round_trips_json() {
        let comment = ReviewComment {
            id: "comment-123".to_string(),
            author: "alice".to_string(),
            text: "Fix this".to_string(),
            timestamp: 1700000000,
            file_path: Some("src/main.rs".to_string()),
            line: Some(42),
            severity: CommentSeverity::Critical,
        };
        let json = serde_json::to_string(&comment).expect("serialize");
        let parsed: ReviewComment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, comment);
    }

    #[test]
    fn add_comment_default_author() {
        let input = json!({
            "action": "add_comment",
            "artifact_id": "test",
            "comment": "Looks good"
        });
        let context = test_context();
        let result = review_artifact(&input, &context).expect("add_comment should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["comment"]["author"], "reviewer");
    }

    #[test]
    fn add_comment_default_severity() {
        let input = json!({
            "action": "add_comment",
            "artifact_id": "test",
            "comment": "Note"
        });
        let context = test_context();
        let result = review_artifact(&input, &context).expect("add_comment should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["comment"]["severity"], "info");
    }
}
