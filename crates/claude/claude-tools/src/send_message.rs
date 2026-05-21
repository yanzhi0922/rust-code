//! Multi-agent messaging tools.
//!
//! `send_message` is aligned to the research surface:
//! - `to`
//! - `summary`
//! - `message` (plain text or structured protocol object)
//!
//! We still keep `broadcast_message` as a legacy compatibility tool because
//! other runtime surfaces still expose it.

use anyhow::{Result, anyhow};
use chrono::Utc;
use claude_core::PermissionMode;
use claude_swarm::constants::{ENV_AGENT_NAME, ENV_PERMISSION_MODE, ENV_TEAM_NAME, TEAM_LEAD_NAME};
use claude_swarm::{MailboxMessage, MailboxMessageType, TeamFile, mailbox};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolExecutionContext;

const RESEARCH_TEAM_LEAD_NAME: &str = "team-lead";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StructuredMessageInput {
    ShutdownRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ShutdownResponse {
        request_id: String,
        approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    PlanApprovalResponse {
        request_id: String,
        approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
enum MessagePriority {
    Low,
    #[default]
    Normal,
    High,
}

/// Send a message to a specific agent or broadcast with `to: "*"`.
///
/// # Errors
/// Returns an error when required inputs are missing or invalid.
pub async fn send_message(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let raw_to = input
        .get("to")
        .and_then(Value::as_str)
        .or_else(|| input.get("recipient").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("to is required"))?;

    validate_target(raw_to)?;

    let explicit_team_name = super::team_runtime::team_name_from_input(input);
    let team_name = resolve_team_name(explicit_team_name.as_deref()).await?;
    let team = super::team_runtime::load_team(&team_name).await?;

    let sender = resolve_sender(input, &team);
    let resolved_to = normalize_lead_alias(raw_to, &team);
    let summary = read_summary(input);
    let message_value = input
        .get("message")
        .ok_or_else(|| anyhow!("message is required"))?;

    if let Some(content) = message_value.as_str() {
        let content = content.trim();
        if content.is_empty() {
            return Err(anyhow!("message cannot be empty"));
        }
        let summary =
            summary.ok_or_else(|| anyhow!("summary is required when message is a string"))?;

        if raw_to == "*" {
            return handle_broadcast_plain_text(&team_name, &team, &sender, content, &summary)
                .await;
        }

        ensure_recipient_exists(&team, &resolved_to)?;
        let mut mailbox_message = MailboxMessage::new(
            sender.clone(),
            resolved_to.clone(),
            MailboxMessageType::Text,
            content.to_owned(),
        );
        mailbox_message.summary = Some(summary.clone());
        mailbox::send_message(&team_name, &mailbox_message).await?;

        return Ok(json!({
            "success": true,
            "message": format!("Message sent to {}'s inbox", resolved_to),
            "routing": {
                "sender": sender,
                "target": format!("@{}", resolved_to),
                "summary": summary,
                "content": content,
            }
        })
        .to_string());
    }

    if raw_to == "*" {
        return Err(anyhow!(
            "structured messages cannot be broadcast (to: \"*\")"
        ));
    }

    let message: StructuredMessageInput = serde_json::from_value(message_value.clone())
        .map_err(|error| anyhow!("invalid structured message: {error}"))?;

    match message {
        StructuredMessageInput::ShutdownRequest { reason } => {
            ensure_recipient_exists(&team, &resolved_to)?;
            let request_id = generate_request_id("shutdown", &resolved_to);
            let payload = json!({
                "type": "shutdown_request",
                "requestId": request_id,
                "from": sender,
                "reason": reason,
                "timestamp": Utc::now().to_rfc3339(),
            });
            let mailbox_message = MailboxMessage::new(
                payload["from"].as_str().unwrap_or_default(),
                resolved_to.clone(),
                MailboxMessageType::Coordination,
                serde_json::to_string(&payload)?,
            );
            mailbox::send_message(&team_name, &mailbox_message).await?;

            Ok(json!({
                "success": true,
                "message": format!("Shutdown request sent to {}. Request ID: {}", resolved_to, request_id),
                "request_id": request_id,
                "target": resolved_to,
            })
            .to_string())
        }
        StructuredMessageInput::ShutdownResponse {
            request_id,
            approve,
            reason,
        } => {
            if !is_lead_address(raw_to, &team) {
                return Err(anyhow!(
                    "shutdown_response must be sent to \"{}\"",
                    RESEARCH_TEAM_LEAD_NAME
                ));
            }
            if !approve
                && reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
            {
                return Err(anyhow!(
                    "reason is required when rejecting a shutdown request"
                ));
            }

            let payload = if approve {
                json!({
                    "type": "shutdown_approved",
                    "requestId": request_id,
                    "from": sender,
                    "timestamp": Utc::now().to_rfc3339(),
                })
            } else {
                json!({
                    "type": "shutdown_rejected",
                    "requestId": request_id,
                    "from": sender,
                    "reason": reason.unwrap_or_default(),
                    "timestamp": Utc::now().to_rfc3339(),
                })
            };
            let mailbox_message = MailboxMessage::new(
                payload["from"].as_str().unwrap_or_default(),
                team.lead_agent_id.clone(),
                MailboxMessageType::Coordination,
                serde_json::to_string(&payload)?,
            );
            mailbox::send_message(&team_name, &mailbox_message).await?;

            let message = if approve {
                format!(
                    "Shutdown approved. Sent confirmation to {}. Agent {} is now exiting.",
                    RESEARCH_TEAM_LEAD_NAME, sender
                )
            } else {
                format!(
                    "Shutdown rejected. Reason: \"{}\". Continuing to work.",
                    payload["reason"].as_str().unwrap_or_default()
                )
            };

            Ok(json!({
                "success": true,
                "message": message,
                "request_id": request_id,
            })
            .to_string())
        }
        StructuredMessageInput::PlanApprovalResponse {
            request_id,
            approve,
            feedback,
        } => {
            if !sender_is_team_lead(&sender, &team) {
                return Err(anyhow!(
                    "Only the team lead can approve plans. Teammates cannot approve their own or other plans."
                ));
            }
            ensure_recipient_exists(&team, &resolved_to)?;

            let inherited_mode = inherited_permission_mode();
            let payload = if approve {
                json!({
                    "type": "plan_approval_response",
                    "requestId": request_id,
                    "approved": true,
                    "timestamp": Utc::now().to_rfc3339(),
                    "permissionMode": inherited_mode,
                })
            } else {
                json!({
                    "type": "plan_approval_response",
                    "requestId": request_id,
                    "approved": false,
                    "feedback": feedback.clone().unwrap_or_else(|| "Plan needs revision".to_owned()),
                    "timestamp": Utc::now().to_rfc3339(),
                })
            };

            let mailbox_message = MailboxMessage::new(
                team.lead_agent_id.clone(),
                resolved_to.clone(),
                MailboxMessageType::Coordination,
                serde_json::to_string(&payload)?,
            );
            mailbox::send_message(&team_name, &mailbox_message).await?;

            let message = if approve {
                format!(
                    "Plan approved for {}. They will receive the approval and can proceed with implementation.",
                    resolved_to
                )
            } else {
                format!(
                    "Plan rejected for {} with feedback: \"{}\"",
                    resolved_to,
                    payload["feedback"].as_str().unwrap_or_default()
                )
            };

            Ok(json!({
                "success": true,
                "message": message,
                "request_id": request_id,
            })
            .to_string())
        }
    }
}

/// Broadcast a message to multiple agents using the legacy compatibility
/// surface exposed elsewhere in the Rust runtime.
///
/// # Errors
/// Returns an error when required inputs are missing or invalid.
pub async fn broadcast_message(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let message = input
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("message is required for broadcast"))?;

    let explicit_team_name = super::team_runtime::team_name_from_input(input);
    let team_name = resolve_team_name(explicit_team_name.as_deref()).await?;
    let team = super::team_runtime::load_team(&team_name).await?;

    let sender = resolve_sender(input, &team);
    let priority = parse_priority(input.get("priority").and_then(Value::as_str));
    let recipients = requested_or_known_recipients(input, &team, &sender)?;

    let mut message_ids = Vec::with_capacity(recipients.len());
    for recipient in &recipients {
        let mut mailbox_message = MailboxMessage::new(
            sender.clone(),
            recipient.clone(),
            MailboxMessageType::Text,
            message.to_owned(),
        );
        mailbox_message.priority = Some(priority_label(priority).to_owned());
        mailbox::send_message(&team_name, &mailbox_message).await?;
        message_ids.push(mailbox_message.id);
    }

    Ok(json!({
        "type": "broadcast_message",
        "broadcast_id": format!("broadcast-{}", uuid::Uuid::new_v4().simple()),
        "team_name": team_name,
        "from": sender,
        "content": message,
        "priority": serde_json::to_value(priority)?,
        "recipients": recipients,
        "message_ids": message_ids,
        "timestamp": Utc::now().timestamp_millis(),
        "status": "queued",
        "delivery": "mailbox_written",
    })
    .to_string())
}

async fn handle_broadcast_plain_text(
    team_name: &str,
    team: &TeamFile,
    sender: &str,
    content: &str,
    summary: &str,
) -> Result<String> {
    let recipients = known_recipients(team, sender);
    if recipients.is_empty() {
        return Ok(json!({
            "success": true,
            "message": "No teammates to broadcast to (you are the only team member)",
            "recipients": [],
        })
        .to_string());
    }

    for recipient in &recipients {
        let mut mailbox_message = MailboxMessage::new(
            sender.to_owned(),
            recipient.clone(),
            MailboxMessageType::Text,
            content.to_owned(),
        );
        mailbox_message.summary = Some(summary.to_owned());
        mailbox::send_message(team_name, &mailbox_message).await?;
    }

    Ok(json!({
        "success": true,
        "message": format!(
            "Message broadcast to {} teammate(s): {}",
            recipients.len(),
            recipients.join(", ")
        ),
        "recipients": recipients,
        "routing": {
            "sender": sender,
            "target": "@team",
            "summary": summary,
            "content": content,
        }
    })
    .to_string())
}

async fn resolve_team_name(explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return super::team_runtime::resolve_single_team_name(Some(name)).await;
    }
    if let Ok(env_team) = std::env::var(ENV_TEAM_NAME)
        && !env_team.trim().is_empty()
    {
        return Ok(env_team);
    }
    super::team_runtime::resolve_single_team_name(None).await
}

fn validate_target(target: &str) -> Result<()> {
    if target.contains('@') {
        return Err(anyhow!(
            "to must be a bare teammate name or \"*\" — there is only one team per session"
        ));
    }
    Ok(())
}

fn resolve_sender(input: &Value, team: &TeamFile) -> String {
    input
        .get("sender")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            std::env::var(ENV_AGENT_NAME)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| team.lead_agent_id.clone())
}

fn normalize_lead_alias(target: &str, team: &TeamFile) -> String {
    if is_lead_alias(target) {
        team.lead_agent_id.clone()
    } else {
        target.to_owned()
    }
}

fn is_lead_alias(value: &str) -> bool {
    value == TEAM_LEAD_NAME || value == RESEARCH_TEAM_LEAD_NAME
}

fn is_lead_address(target: &str, team: &TeamFile) -> bool {
    target == team.lead_agent_id || is_lead_alias(target)
}

fn sender_is_team_lead(sender: &str, team: &TeamFile) -> bool {
    sender == team.lead_agent_id || is_lead_alias(sender)
}

fn ensure_recipient_exists(team: &TeamFile, recipient: &str) -> Result<()> {
    if recipient == team.lead_agent_id || team.members.iter().any(|member| member.name == recipient)
    {
        Ok(())
    } else {
        Err(anyhow!(
            "recipient '{}' is not a member of team '{}'",
            recipient,
            team.name
        ))
    }
}

fn read_summary(input: &Value) -> Option<String> {
    input
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn generate_request_id(request_type: &str, target: &str) -> String {
    format!("{request_type}-{}@{target}", Utc::now().timestamp_millis())
}

fn inherited_permission_mode() -> String {
    let mode = std::env::var(ENV_PERMISSION_MODE)
        .ok()
        .and_then(|raw| match raw.as_str() {
            "default" => Some(PermissionMode::Default),
            "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "bypassPermissions" => Some(PermissionMode::BypassPermissions),
            "dontAsk" => Some(PermissionMode::DontAsk),
            "plan" => Some(PermissionMode::Plan),
            _ => None,
        })
        .unwrap_or(PermissionMode::Default);

    match mode {
        PermissionMode::Plan => PermissionMode::Default.as_legacy_str().to_owned(),
        other => other.as_legacy_str().to_owned(),
    }
}

fn known_recipients(team: &TeamFile, sender: &str) -> Vec<String> {
    std::iter::once(team.lead_agent_id.clone())
        .chain(team.members.iter().map(|member| member.name.clone()))
        .filter(|name| name != sender)
        .collect()
}

fn requested_or_known_recipients(
    input: &Value,
    team: &TeamFile,
    sender: &str,
) -> Result<Vec<String>> {
    let known = known_recipients(team, sender);
    if let Some(items) = input.get("recipients").and_then(Value::as_array) {
        let recipients = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| normalize_lead_alias(value, team))
            .collect::<Vec<_>>();
        if recipients.is_empty() {
            return Ok(known);
        }
        for recipient in &recipients {
            ensure_recipient_exists(team, recipient)?;
        }
        return Ok(recipients);
    }
    Ok(known)
}

fn parse_priority(priority: Option<&str>) -> MessagePriority {
    match priority.unwrap_or("normal") {
        "low" => MessagePriority::Low,
        "high" => MessagePriority::High,
        _ => MessagePriority::Normal,
    }
}

fn priority_label(priority: MessagePriority) -> &'static str {
    match priority {
        MessagePriority::Low => "low",
        MessagePriority::Normal => "normal",
        MessagePriority::High => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Arc;

    use claude_swarm::{TeamMember, team_helpers};
    use tempfile::TempDir;

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

    struct TeamDirGuard;

    impl Drop for TeamDirGuard {
        fn drop(&mut self) {
            team_helpers::set_base_dir_override(None);
        }
    }

    async fn setup_team(temp: &TempDir, team_name: &str) -> TeamDirGuard {
        team_helpers::set_base_dir_override(Some(temp.path().to_path_buf()));
        let mut team = TeamFile::new(team_name, "lead");
        team.description = Some("test objective".to_owned());
        team.members
            .push(TeamMember::new("agent-1-id", "agent-1", "pane-1", "/tmp"));
        team.members
            .push(TeamMember::new("agent-2-id", "agent-2", "pane-2", "/tmp"));
        team_helpers::create_team(&team)
            .await
            .expect("create test team");
        TeamDirGuard
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_message_requires_to() {
        let result = send_message(&json!({"message": "hello"}), &test_context()).await;
        let error = result.expect_err("missing to should fail");
        assert!(error.to_string().contains("to"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_message_requires_summary_for_plain_text() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "summary-team").await;
        let result = send_message(
            &json!({
                "team_name": "summary-team",
                "to": "agent-1",
                "message": "hello"
            }),
            &test_context(),
        )
        .await;
        let error = result.expect_err("summary should be required");
        assert!(error.to_string().contains("summary"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_message_plain_text_writes_mailbox_summary() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "direct-team").await;
        let result = send_message(
            &json!({
                "team_name": "direct-team",
                "to": "agent-1",
                "summary": "assign task",
                "message": "start on task #1"
            }),
            &test_context(),
        )
        .await
        .expect("send message");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["routing"]["target"], "@agent-1");
        assert_eq!(parsed["routing"]["summary"], "assign task");

        let stored = mailbox::read_messages("direct-team", "agent-1")
            .await
            .expect("read mailbox");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, "start on task #1");
        assert_eq!(stored[0].summary.as_deref(), Some("assign task"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_message_broadcasts_with_to_star() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "broadcast-star-team").await;
        let result = send_message(
            &json!({
                "team_name": "broadcast-star-team",
                "to": "*",
                "summary": "sync status",
                "message": "check in"
            }),
            &test_context(),
        )
        .await
        .expect("broadcast send_message");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["recipients"].as_array().map(Vec::len), Some(2));

        let agent_1 = mailbox::read_messages("broadcast-star-team", "agent-1")
            .await
            .expect("agent-1 mailbox");
        let agent_2 = mailbox::read_messages("broadcast-star-team", "agent-2")
            .await
            .expect("agent-2 mailbox");
        assert_eq!(agent_1.len(), 1);
        assert_eq!(agent_2.len(), 1);
        assert_eq!(agent_1[0].summary.as_deref(), Some("sync status"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn structured_messages_cannot_broadcast() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "structured-broadcast-team").await;
        let result = send_message(
            &json!({
                "team_name": "structured-broadcast-team",
                "to": "*",
                "message": {"type": "shutdown_request"}
            }),
            &test_context(),
        )
        .await;
        let error = result.expect_err("structured broadcast should fail");
        assert!(error.to_string().contains("cannot be broadcast"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_request_writes_research_payload() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "shutdown-team").await;
        let result = send_message(
            &json!({
                "team_name": "shutdown-team",
                "to": "agent-1",
                "message": {"type": "shutdown_request", "reason": "work complete"},
            }),
            &test_context(),
        )
        .await
        .expect("shutdown request");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["target"], "agent-1");
        assert!(
            parsed["request_id"]
                .as_str()
                .unwrap_or_default()
                .starts_with("shutdown-")
        );

        let stored = mailbox::read_messages("shutdown-team", "agent-1")
            .await
            .expect("read mailbox");
        let content: Value = serde_json::from_str(&stored[0].content).expect("structured payload");
        assert_eq!(content["type"], "shutdown_request");
        assert_eq!(content["reason"], "work complete");
        assert_eq!(content["from"], "lead");
        assert_eq!(content["requestId"], parsed["request_id"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_response_writes_shutdown_approved_for_lead() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "shutdown-response-team").await;
        let result = send_message(
            &json!({
                "team_name": "shutdown-response-team",
                "sender": "agent-1",
                "to": "team-lead",
                "message": {"type": "shutdown_response", "request_id": "req-1", "approve": true},
            }),
            &test_context(),
        )
        .await
        .expect("shutdown approval");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["request_id"], "req-1");

        let stored = mailbox::read_messages("shutdown-response-team", "lead")
            .await
            .expect("lead mailbox");
        let content: Value = serde_json::from_str(&stored[0].content).expect("structured payload");
        assert_eq!(content["type"], "shutdown_approved");
        assert_eq!(content["requestId"], "req-1");
        assert_eq!(content["from"], "agent-1");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_approval_response_requires_team_lead_sender() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "plan-team").await;
        let result = send_message(
            &json!({
                "team_name": "plan-team",
                "sender": "agent-1",
                "to": "agent-2",
                "message": {"type": "plan_approval_response", "request_id": "req-2", "approve": true},
            }),
            &test_context(),
        )
        .await;
        let error = result.expect_err("non-lead should be rejected");
        assert!(error.to_string().contains("Only the team lead"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_approval_response_writes_permission_mode() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "plan-approve-team").await;
        let result = send_message(
            &json!({
                "team_name": "plan-approve-team",
                "sender": "lead",
                "to": "agent-1",
                "message": {"type": "plan_approval_response", "request_id": "req-2", "approve": true},
            }),
            &test_context(),
        )
        .await
        .expect("plan approval");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["request_id"], "req-2");

        let stored = mailbox::read_messages("plan-approve-team", "agent-1")
            .await
            .expect("worker mailbox");
        let content: Value = serde_json::from_str(&stored[0].content).expect("structured payload");
        assert_eq!(content["type"], "plan_approval_response");
        assert_eq!(content["requestId"], "req-2");
        assert_eq!(content["approved"], true);
        assert_eq!(content["permissionMode"], "default");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_broadcast_message_keeps_priority_behavior() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = setup_team(&temp, "legacy-broadcast-team").await;
        let result = broadcast_message(
            &json!({
                "team_name": "legacy-broadcast-team",
                "sender": "lead",
                "message": "urgent",
                "priority": "high"
            }),
            &test_context(),
        )
        .await
        .expect("legacy broadcast");

        let parsed: Value = serde_json::from_str(&result).expect("json result");
        assert_eq!(parsed["priority"], "high");
        assert_eq!(parsed["recipients"].as_array().map(Vec::len), Some(2));

        let stored = mailbox::read_messages("legacy-broadcast-team", "agent-1")
            .await
            .expect("read mailbox");
        assert_eq!(stored[0].priority.as_deref(), Some("high"));
    }
}
