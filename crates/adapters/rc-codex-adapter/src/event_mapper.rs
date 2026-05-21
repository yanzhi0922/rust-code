//! Event mapping from Codex `AppServerEvent` → `UnifiedAgentEvent`.
//!
//! The Codex app-server protocol uses a rich notification system with many
//! specialized event types. This module translates the subset relevant to
//! the unified agent protocol into [`UnifiedAgentEvent`] variants.

use rc_agent_protocol::events::{AgentResult, ToolCallInfo, UnifiedAgentEvent, UsageInfo};
use serde::Serialize;

use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::{
    CommandExecutionStatus, DynamicToolCallStatus, McpToolCallStatus, PatchApplyStatus,
    ServerNotification, ServerRequest, ThreadItem,
};

use crate::types::request_id_to_string;

fn progress_event(
    session_id: &str,
    tool_name: impl Into<String>,
    progress: impl Into<String>,
) -> Vec<UnifiedAgentEvent> {
    vec![UnifiedAgentEvent::ToolCallProgress {
        session_id: session_id.to_owned(),
        tool_name: tool_name.into(),
        progress: progress.into(),
    }]
}

fn json_progress_event(
    session_id: &str,
    tool_name: impl Into<String>,
    payload: impl serde::Serialize,
) -> Vec<UnifiedAgentEvent> {
    progress_event(
        session_id,
        tool_name,
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned()),
    )
}

fn to_json_value(context: &'static str, payload: impl Serialize) -> serde_json::Value {
    serde_json::to_value(payload).unwrap_or_else(|error| {
        tracing::warn!(context, %error, "failed to serialize Codex protocol payload");
        serde_json::Value::Null
    })
}

fn official_event(
    session_id: &str,
    method: &'static str,
    payload: impl serde::Serialize,
) -> Vec<UnifiedAgentEvent> {
    vec![UnifiedAgentEvent::CodexAppServerNotification {
        session_id: session_id.to_owned(),
        method: method.to_owned(),
        params: to_json_value(method, payload),
    }]
}

fn raw_server_notification(
    session_id: &str,
    notification: &ServerNotification,
) -> UnifiedAgentEvent {
    UnifiedAgentEvent::CodexAppServerNotification {
        session_id: session_id.to_owned(),
        method: notification.to_string(),
        params: notification
            .clone()
            .to_params()
            .unwrap_or(serde_json::Value::Null),
    }
}

fn non_negative_i64_to_usize(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn thread_item_tool_call(item: &ThreadItem) -> Option<ToolCallInfo> {
    match item {
        ThreadItem::CommandExecution {
            id,
            command,
            cwd,
            process_id,
            source,
            status,
            command_actions,
            aggregated_output,
            exit_code,
            duration_ms,
        } if *status != CommandExecutionStatus::InProgress => Some(ToolCallInfo {
            id: id.clone(),
            name: "command_execution".to_owned(),
            input: serde_json::json!({
                "command": command,
                "cwd": cwd,
                "processId": process_id,
                "source": source,
                "commandActions": command_actions,
            }),
            output: serde_json::json!({
                "status": status,
                "aggregatedOutput": aggregated_output,
                "exitCode": exit_code,
                "durationMs": duration_ms,
            }),
        }),
        ThreadItem::FileChange {
            id,
            changes,
            status,
        } if *status != PatchApplyStatus::InProgress => Some(ToolCallInfo {
            id: id.clone(),
            name: "file_change".to_owned(),
            input: serde_json::json!({
                "changes": changes,
            }),
            output: serde_json::json!({
                "status": status,
            }),
        }),
        ThreadItem::McpToolCall {
            id,
            server,
            tool,
            status,
            arguments,
            mcp_app_resource_uri,
            result,
            error,
            duration_ms,
        } if *status != McpToolCallStatus::InProgress => Some(ToolCallInfo {
            id: id.clone(),
            name: format!("mcp/{server}/{tool}"),
            input: serde_json::json!({
                "server": server,
                "tool": tool,
                "arguments": arguments,
                "mcpAppResourceUri": mcp_app_resource_uri,
            }),
            output: serde_json::json!({
                "status": status,
                "result": result,
                "error": error,
                "durationMs": duration_ms,
            }),
        }),
        ThreadItem::DynamicToolCall {
            id,
            namespace,
            tool,
            arguments,
            status,
            content_items,
            success,
            duration_ms,
        } if *status != DynamicToolCallStatus::InProgress => Some(ToolCallInfo {
            id: id.clone(),
            name: namespace
                .as_ref()
                .map(|namespace| format!("dynamic/{namespace}/{tool}"))
                .unwrap_or_else(|| format!("dynamic/{tool}")),
            input: serde_json::json!({
                "namespace": namespace,
                "tool": tool,
                "arguments": arguments,
            }),
            output: serde_json::json!({
                "status": status,
                "contentItems": content_items,
                "success": success,
                "durationMs": duration_ms,
            }),
        }),
        ThreadItem::CollabAgentToolCall {
            id,
            tool,
            status,
            sender_thread_id,
            receiver_thread_ids,
            prompt,
            model,
            reasoning_effort,
            agents_states,
        } => Some(ToolCallInfo {
            id: id.clone(),
            name: "collab_agent".to_owned(),
            input: serde_json::json!({
                "tool": tool,
                "senderThreadId": sender_thread_id,
                "receiverThreadIds": receiver_thread_ids,
                "prompt": prompt,
                "model": model,
                "reasoningEffort": reasoning_effort,
            }),
            output: serde_json::json!({
                "status": status,
                "agentsStates": agents_states,
            }),
        }),
        ThreadItem::WebSearch { id, query, action } => Some(ToolCallInfo {
            id: id.clone(),
            name: "web_search".to_owned(),
            input: serde_json::json!({ "query": query }),
            output: serde_json::json!({ "action": action }),
        }),
        ThreadItem::ImageView { id, path } => Some(ToolCallInfo {
            id: id.clone(),
            name: "image_view".to_owned(),
            input: serde_json::json!({ "path": path }),
            output: serde_json::json!({ "path": path }),
        }),
        ThreadItem::ImageGeneration {
            id,
            status,
            revised_prompt,
            result,
            saved_path,
        } => Some(ToolCallInfo {
            id: id.clone(),
            name: "image_generation".to_owned(),
            input: serde_json::json!({
                "revisedPrompt": revised_prompt,
            }),
            output: serde_json::json!({
                "status": status,
                "result": result,
                "savedPath": saved_path,
            }),
        }),
        _ => None,
    }
}

/// Map a Codex [`AppServerEvent`] into zero or more [`UnifiedAgentEvent`]s.
///
/// Most Codex events map 1:1 to a unified event, but some (like `Lagged`)
/// are silently consumed, and some may produce multiple unified events in
/// the future.
pub fn map_app_server_event(event: AppServerEvent, session_id: &str) -> Vec<UnifiedAgentEvent> {
    match event {
        AppServerEvent::Lagged { skipped } => {
            tracing::debug!(session_id, skipped, "Codex event lag — backpressure signal");
            Vec::new()
        }

        AppServerEvent::ServerNotification(notification) => {
            let method = notification.to_string();
            tracing::debug!(session_id, %method, "Codex server notification");
            let mut events = vec![raw_server_notification(session_id, &notification)];
            events.extend(map_server_notification(notification, session_id));
            events
        }

        AppServerEvent::ServerRequest(request) => {
            let req_id = request_id_to_string(request.id());
            tracing::debug!(session_id, %req_id, "Codex server request (permission)");
            map_server_request(request, session_id)
        }

        AppServerEvent::Disconnected { message } => {
            vec![UnifiedAgentEvent::Error {
                session_id: session_id.to_owned(),
                message: format!("Codex server disconnected: {message}"),
                recoverable: false,
            }]
        }
    }
}

/// Derive a human-readable tool name from a [`ThreadItem`] variant.
fn thread_item_kind(item: &ThreadItem) -> &'static str {
    match item {
        ThreadItem::UserMessage { .. } => "user_message",
        ThreadItem::HookPrompt { .. } => "hook_prompt",
        ThreadItem::AgentMessage { .. } => "agent_message",
        ThreadItem::Plan { .. } => "plan",
        ThreadItem::Reasoning { .. } => "reasoning",
        ThreadItem::CommandExecution { .. } => "command_execution",
        ThreadItem::FileChange { .. } => "file_change",
        ThreadItem::McpToolCall { .. } => "mcp_tool_call",
        ThreadItem::DynamicToolCall { .. } => "dynamic_tool_call",
        ThreadItem::CollabAgentToolCall { .. } => "collab_agent_tool_call",
        ThreadItem::WebSearch { .. } => "web_search",
        ThreadItem::ImageView { .. } => "image_view",
        ThreadItem::ImageGeneration { .. } => "image_generation",
        ThreadItem::EnteredReviewMode { .. } => "entered_review_mode",
        ThreadItem::ExitedReviewMode { .. } => "exited_review_mode",
        ThreadItem::ContextCompaction { .. } => "context_compaction",
    }
}

/// Map a Codex [`ServerNotification`] into unified events.
fn map_server_notification(
    notification: ServerNotification,
    session_id: &str,
) -> Vec<UnifiedAgentEvent> {
    match notification {
        // ── Streaming text ──
        ServerNotification::AgentMessageDelta(delta) => {
            vec![UnifiedAgentEvent::MessageDelta {
                session_id: session_id.to_owned(),
                delta: delta.delta,
            }]
        }

        ServerNotification::PlanDelta(delta) => progress_event(session_id, "plan", delta.delta),

        ServerNotification::ReasoningSummaryTextDelta(delta) => {
            progress_event(session_id, "reasoning_summary", delta.delta)
        }

        ServerNotification::ReasoningTextDelta(delta) => {
            progress_event(session_id, "reasoning", delta.delta)
        }

        // ── Tool / item lifecycle ──
        ServerNotification::ItemStarted(item) => {
            let tool_name = thread_item_kind(&item.item).to_owned();
            let tool_input = to_json_value("item/started", &item.item);
            let mut events = vec![UnifiedAgentEvent::ToolCallStarted {
                session_id: session_id.to_owned(),
                tool_name,
                tool_input,
            }];
            // CollabAgentToolCall maps to SubtaskStarted for parity with Claude/Roo.
            if let ThreadItem::CollabAgentToolCall {
                id,
                prompt,
                receiver_thread_ids,
                ..
            } = &item.item
            {
                let desc = prompt.clone().unwrap_or_else(|| format!("Agent task {id}"));
                for rid in receiver_thread_ids {
                    events.push(UnifiedAgentEvent::SubtaskStarted {
                        session_id: session_id.to_owned(),
                        task_id: rid.clone(),
                        description: desc.clone(),
                    });
                }
            }
            events
        }

        ServerNotification::ItemCompleted(item) => {
            let tool_name = thread_item_kind(&item.item).to_owned();
            let result = to_json_value("item/completed", &item.item);
            let mut events = vec![UnifiedAgentEvent::ToolCallCompleted {
                session_id: session_id.to_owned(),
                tool_name,
                result,
            }];
            // CollabAgentToolCall completion maps to SubtaskCompleted.
            if let ThreadItem::CollabAgentToolCall {
                id,
                status,
                agents_states,
                ..
            } = &item.item
            {
                let task_result = serde_json::json!({
                    "id": id,
                    "status": status,
                    "agentsStates": agents_states,
                });
                // Emit SubtaskCompleted for the primary receiver.
                events.push(UnifiedAgentEvent::SubtaskCompleted {
                    session_id: session_id.to_owned(),
                    task_id: id.clone(),
                    result: task_result,
                });
            }
            events
        }

        // ── Command output streaming ──
        ServerNotification::CommandExecutionOutputDelta(delta) => {
            progress_event(session_id, "command_execution", delta.delta)
        }

        ServerNotification::CommandExecOutputDelta(delta) => {
            json_progress_event(session_id, "command_exec", delta)
        }

        ServerNotification::TerminalInteraction(notification) => {
            json_progress_event(session_id, "terminal_interaction", notification)
        }

        // ── File change output ──
        ServerNotification::FileChangeOutputDelta(delta) => {
            progress_event(session_id, "file_change", delta.delta)
        }

        ServerNotification::FileChangePatchUpdated(notification) => {
            json_progress_event(session_id, "file_change_patch", notification)
        }

        ServerNotification::McpToolCallProgress(notification) => {
            progress_event(session_id, "mcp_tool_call", notification.message)
        }

        ServerNotification::ServerRequestResolved(notification) => {
            json_progress_event(session_id, "server_request_resolved", notification)
        }

        // ── Turn lifecycle ──
        ServerNotification::ThreadStarted(notification) => {
            json_progress_event(session_id, "thread_started", notification.thread)
        }

        ServerNotification::ThreadStatusChanged(notification) => {
            json_progress_event(session_id, "thread_status", notification)
        }

        ServerNotification::ThreadArchived(notification) => {
            json_progress_event(session_id, "thread_archived", notification)
        }

        ServerNotification::ThreadUnarchived(notification) => {
            json_progress_event(session_id, "thread_unarchived", notification)
        }

        ServerNotification::ThreadClosed(notification) => {
            json_progress_event(session_id, "thread_closed", notification)
        }

        ServerNotification::SkillsChanged(notification) => {
            official_event(session_id, "skills/changed", notification)
        }

        ServerNotification::ThreadNameUpdated(notification) => {
            json_progress_event(session_id, "thread_name_updated", notification)
        }

        ServerNotification::ThreadGoalUpdated(notification) => {
            official_event(session_id, "thread/goal/updated", notification)
        }

        ServerNotification::ThreadGoalCleared(notification) => {
            official_event(session_id, "thread/goal/cleared", notification)
        }

        ServerNotification::TurnStarted(notification) => {
            json_progress_event(session_id, "turn_started", notification)
        }

        ServerNotification::HookStarted(notification) => {
            official_event(session_id, "hook/started", notification)
        }

        ServerNotification::TurnPlanUpdated(notification) => {
            json_progress_event(session_id, "turn_plan", notification)
        }

        ServerNotification::TurnDiffUpdated(notification) => {
            progress_event(session_id, "turn_diff", notification.diff)
        }

        ServerNotification::TurnCompleted(notification) => {
            let response_text = notification
                .turn
                .items
                .iter()
                .filter_map(|item| match item {
                    ThreadItem::AgentMessage { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let tool_calls: Vec<_> = notification
                .turn
                .items
                .iter()
                .filter_map(thread_item_tool_call)
                .collect();

            tracing::debug!(
                session_id,
                turn_id = %notification.turn.id,
                text_len = response_text.len(),
                tool_call_count = tool_calls.len(),
                "Codex turn completed"
            );

            vec![UnifiedAgentEvent::Completed {
                session_id: session_id.to_owned(),
                result: AgentResult {
                    response_text,
                    tool_calls,
                    usage: UsageInfo::default(),
                    cost: None,
                },
            }]
        }

        ServerNotification::HookCompleted(notification) => {
            official_event(session_id, "hook/completed", notification)
        }

        // ── Context management ──
        ServerNotification::ThreadTokenUsageUpdated(notification) => {
            let used = non_negative_i64_to_usize(notification.token_usage.total.total_tokens);
            let total = notification
                .token_usage
                .model_context_window
                .map(non_negative_i64_to_usize)
                .unwrap_or(0);
            let mut events = vec![UnifiedAgentEvent::ContextUsage {
                session_id: session_id.to_owned(),
                used,
                total,
            }];
            events.extend(json_progress_event(
                session_id,
                "codex_token_usage",
                notification,
            ));
            events
        }

        ServerNotification::ContextCompacted(_notification) => {
            // Codex protocol's ContextCompactedNotification only carries
            // thread_id and turn_id — no count of entries removed or usage
            // ratio. Emit the event with defaults so consumers know compaction
            // happened even if details are unavailable.
            vec![UnifiedAgentEvent::ContextCompacted {
                session_id: session_id.to_owned(),
                entries_removed: 0,
                usage_ratio: 0.0,
            }]
        }

        ServerNotification::McpServerStatusUpdated(notification) => {
            json_progress_event(session_id, "mcp_server_status", notification)
        }

        ServerNotification::McpServerOauthLoginCompleted(notification) => {
            json_progress_event(session_id, "mcp_oauth_login", notification)
        }

        ServerNotification::AccountUpdated(notification) => {
            official_event(session_id, "account/updated", notification)
        }

        ServerNotification::AccountRateLimitsUpdated(notification) => {
            official_event(session_id, "account/rateLimits/updated", notification)
        }

        ServerNotification::AppListUpdated(notification) => {
            official_event(session_id, "app/list/updated", notification)
        }

        ServerNotification::ExternalAgentConfigImportCompleted(notification) => official_event(
            session_id,
            "externalAgentConfig/import/completed",
            notification,
        ),

        ServerNotification::FsChanged(notification) => {
            official_event(session_id, "fs/changed", notification)
        }

        ServerNotification::ReasoningSummaryPartAdded(notification) => {
            official_event(session_id, "item/reasoning/summaryPartAdded", notification)
        }

        ServerNotification::RawResponseItemCompleted(notification) => {
            official_event(session_id, "rawResponseItem/completed", notification)
        }

        ServerNotification::ItemGuardianApprovalReviewStarted(notification) => {
            official_event(session_id, "item/autoApprovalReview/started", notification)
        }

        ServerNotification::ItemGuardianApprovalReviewCompleted(notification) => official_event(
            session_id,
            "item/autoApprovalReview/completed",
            notification,
        ),

        ServerNotification::Warning(notification) => {
            progress_event(session_id, "warning", notification.message)
        }

        ServerNotification::GuardianWarning(notification) => {
            json_progress_event(session_id, "guardian_warning", notification)
        }

        ServerNotification::ConfigWarning(notification) => {
            json_progress_event(session_id, "config_warning", notification)
        }

        ServerNotification::ModelRerouted(notification) => {
            json_progress_event(session_id, "model_rerouted", notification)
        }

        ServerNotification::ModelVerification(notification) => {
            official_event(session_id, "model/verification", notification)
        }

        ServerNotification::DeprecationNotice(notification) => {
            official_event(session_id, "deprecationNotice", notification)
        }

        ServerNotification::FuzzyFileSearchSessionUpdated(notification) => {
            official_event(session_id, "fuzzyFileSearch/sessionUpdated", notification)
        }

        ServerNotification::FuzzyFileSearchSessionCompleted(notification) => {
            official_event(session_id, "fuzzyFileSearch/sessionCompleted", notification)
        }

        ServerNotification::ThreadRealtimeStarted(notification) => {
            official_event(session_id, "thread/realtime/started", notification)
        }

        ServerNotification::ThreadRealtimeItemAdded(notification) => {
            official_event(session_id, "thread/realtime/itemAdded", notification)
        }

        ServerNotification::ThreadRealtimeTranscriptDelta(notification) => {
            official_event(session_id, "thread/realtime/transcript/delta", notification)
        }

        ServerNotification::ThreadRealtimeTranscriptDone(notification) => {
            official_event(session_id, "thread/realtime/transcript/done", notification)
        }

        ServerNotification::ThreadRealtimeOutputAudioDelta(notification) => official_event(
            session_id,
            "thread/realtime/outputAudio/delta",
            notification,
        ),

        ServerNotification::ThreadRealtimeSdp(notification) => {
            official_event(session_id, "thread/realtime/sdp", notification)
        }

        ServerNotification::ThreadRealtimeError(notification) => {
            official_event(session_id, "thread/realtime/error", notification)
        }

        ServerNotification::ThreadRealtimeClosed(notification) => {
            official_event(session_id, "thread/realtime/closed", notification)
        }

        ServerNotification::WindowsWorldWritableWarning(notification) => {
            official_event(session_id, "windows/worldWritableWarning", notification)
        }

        ServerNotification::WindowsSandboxSetupCompleted(notification) => {
            official_event(session_id, "windowsSandbox/setupCompleted", notification)
        }

        ServerNotification::AccountLoginCompleted(notification) => {
            official_event(session_id, "account/login/completed", notification)
        }

        ServerNotification::RemoteControlStatusChanged(notification) => {
            official_event(session_id, "remoteControl/status/changed", notification)
        }

        // ── Errors ──
        ServerNotification::Error(notification) => {
            vec![UnifiedAgentEvent::Error {
                session_id: session_id.to_owned(),
                message: notification.error.message,
                recoverable: notification.will_retry,
            }]
        }
    }
}

/// Map a Codex [`ServerRequest`] (permission request) into a unified event.
fn map_server_request(request: ServerRequest, session_id: &str) -> Vec<UnifiedAgentEvent> {
    let (tool_name, input) = match &request {
        ServerRequest::CommandExecutionRequestApproval { params, .. } => (
            "command_execution".to_owned(),
            to_json_value("command_execution_request", params),
        ),
        ServerRequest::FileChangeRequestApproval { params, .. } => (
            "file_change".to_owned(),
            to_json_value("file_change_request", params),
        ),
        ServerRequest::ApplyPatchApproval { params, .. } => (
            "apply_patch".to_owned(),
            to_json_value("apply_patch_request", params),
        ),
        ServerRequest::ExecCommandApproval { params, .. } => (
            "exec_command".to_owned(),
            to_json_value("exec_command_request", params),
        ),
        ServerRequest::PermissionsRequestApproval { params, .. } => (
            "permissions".to_owned(),
            to_json_value("permissions_request", params),
        ),
        ServerRequest::ToolRequestUserInput { params, .. } => (
            "tool_user_input".to_owned(),
            to_json_value("tool_user_input_request", params),
        ),
        ServerRequest::McpServerElicitationRequest { params, .. } => (
            "mcp_elicitation".to_owned(),
            to_json_value("mcp_elicitation_request", params),
        ),
        ServerRequest::DynamicToolCall { params, .. } => (
            "dynamic_tool".to_owned(),
            to_json_value("dynamic_tool_request", params),
        ),
        ServerRequest::ChatgptAuthTokensRefresh { params, .. } => (
            "chatgpt_auth_refresh".to_owned(),
            to_json_value("chatgpt_auth_refresh_request", params),
        ),
    };

    vec![UnifiedAgentEvent::PermissionRequest {
        session_id: session_id.to_owned(),
        request_id: request_id_to_string(request.id()),
        tool_name,
        input,
    }]
}

// Reuse request_id_to_string from types.rs (pub(crate))

#[cfg(test)]
mod tests {
    use super::*;

    use codex_app_server_protocol::{
        AgentMessageDeltaNotification, ServerNotification, ServerRequest,
    };

    #[test]
    fn preserves_raw_codex_server_notification_before_derived_events() {
        let notification = ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: "thread-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            item_id: "item-1".to_owned(),
            delta: "hello".to_owned(),
        });

        let events = map_app_server_event(
            AppServerEvent::ServerNotification(notification),
            "session-1",
        );

        assert_eq!(events.len(), 2);
        match &events[0] {
            UnifiedAgentEvent::CodexAppServerNotification {
                session_id,
                method,
                params,
            } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(method, "item/agentMessage/delta");
                assert_eq!(params["threadId"], "thread-1");
                assert_eq!(params["turnId"], "turn-1");
                assert_eq!(params["itemId"], "item-1");
                assert_eq!(params["delta"], "hello");
            }
            other => panic!("expected raw Codex notification, got {other:?}"),
        }
    }

    #[test]
    fn derives_message_delta_from_agent_message_delta_notification() {
        let notification = ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: "thread-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            item_id: "item-1".to_owned(),
            delta: "hello".to_owned(),
        });

        let events = map_app_server_event(
            AppServerEvent::ServerNotification(notification),
            "session-1",
        );

        match &events[1] {
            UnifiedAgentEvent::MessageDelta { session_id, delta } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(delta, "hello");
            }
            other => panic!("expected derived message delta, got {other:?}"),
        }
    }

    #[test]
    fn maps_chatgpt_auth_refresh_request_to_permission_event() {
        let request = ServerRequest::ChatgptAuthTokensRefresh {
            request_id: codex_app_server_protocol::RequestId::String("req-1".to_owned()),
            params: codex_app_server_protocol::ChatgptAuthTokensRefreshParams {
                reason: codex_app_server_protocol::ChatgptAuthTokensRefreshReason::Unauthorized,
                previous_account_id: None,
            },
        };

        let events = map_app_server_event(AppServerEvent::ServerRequest(request), "session-1");

        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::PermissionRequest {
                session_id,
                request_id,
                tool_name,
                input,
            } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(request_id, "req-1");
                assert_eq!(tool_name, "chatgpt_auth_refresh");
                assert_eq!(input["reason"], "unauthorized");
                assert!(
                    input.get("previousAccountId").is_none()
                        || input["previousAccountId"].is_null()
                );
            }
            other => panic!("expected permission request, got {other:?}"),
        }
    }

    // ── Top-level AppServerEvent branches ──

    #[test]
    fn lagged_event_produces_no_output() {
        let events = map_app_server_event(AppServerEvent::Lagged { skipped: 42 }, "s");
        assert!(events.is_empty());
    }

    #[test]
    fn disconnected_produces_unrecoverable_error() {
        let events = map_app_server_event(
            AppServerEvent::Disconnected {
                message: "server gone".to_owned(),
            },
            "s",
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::Error {
                session_id,
                message,
                recoverable,
            } => {
                assert_eq!(session_id, "s");
                assert!(message.contains("server gone"));
                assert!(!recoverable);
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    // ── ServerNotification: streaming ──

    #[test]
    fn plan_delta_produces_tool_progress() {
        use codex_app_server_protocol::PlanDeltaNotification;
        let notification = ServerNotification::PlanDelta(PlanDeltaNotification {
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
            item_id: "item-1".to_owned(),
            delta: "step 1".to_owned(),
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::ToolCallProgress {
                session_id,
                tool_name,
                progress,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(tool_name, "plan");
                assert_eq!(progress, "step 1");
            }
            other => panic!("expected tool progress, got {other:?}"),
        }
    }

    // ── ServerNotification: item lifecycle ──

    #[test]
    fn item_started_produces_tool_call_started() {
        use codex_app_server_protocol::{ItemStartedNotification, ThreadItem};
        let notification = ServerNotification::ItemStarted(ItemStartedNotification {
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
            item: ThreadItem::AgentMessage {
                id: "item-1".to_owned(),
                text: "thinking...".to_owned(),
                phase: None,
                memory_citation: None,
            },
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::ToolCallStarted {
                session_id,
                tool_name,
                ..
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(tool_name, "agent_message");
            }
            other => panic!("expected tool call started, got {other:?}"),
        }
    }

    #[test]
    fn item_completed_produces_tool_call_completed() {
        use codex_app_server_protocol::{ItemCompletedNotification, ThreadItem};
        let notification = ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
            item: ThreadItem::AgentMessage {
                id: "item-1".to_owned(),
                text: "done".to_owned(),
                phase: None,
                memory_citation: None,
            },
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::ToolCallCompleted {
                session_id,
                tool_name,
                ..
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(tool_name, "agent_message");
            }
            other => panic!("expected tool call completed, got {other:?}"),
        }
    }

    // ── ServerNotification: turn completed with text ──

    #[test]
    fn turn_completed_extracts_agent_message_text() {
        use codex_app_server_protocol::{ThreadItem, Turn, TurnCompletedNotification, TurnStatus};
        let notification = ServerNotification::TurnCompleted(TurnCompletedNotification {
            thread_id: "t".to_owned(),
            turn: Turn {
                id: "turn-1".to_owned(),
                items: vec![
                    ThreadItem::AgentMessage {
                        id: "msg".to_owned(),
                        text: "hello world".to_owned(),
                        phase: None,
                        memory_citation: None,
                    },
                    ThreadItem::AgentMessage {
                        id: "msg-2".to_owned(),
                        text: " more text".to_owned(),
                        phase: None,
                        memory_citation: None,
                    },
                ],
                status: TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::Completed { session_id, result } => {
                assert_eq!(session_id, "s");
                assert_eq!(result.response_text, "hello world more text");
                assert!(result.tool_calls.is_empty());
            }
            other => panic!("expected completed event, got {other:?}"),
        }
    }

    #[test]
    fn turn_completed_with_web_search_tool_call() {
        use codex_app_server_protocol::{ThreadItem, Turn, TurnCompletedNotification, TurnStatus};
        let notification = ServerNotification::TurnCompleted(TurnCompletedNotification {
            thread_id: "t".to_owned(),
            turn: Turn {
                id: "turn-1".to_owned(),
                items: vec![
                    ThreadItem::AgentMessage {
                        id: "msg".to_owned(),
                        text: "results".to_owned(),
                        phase: None,
                        memory_citation: None,
                    },
                    ThreadItem::WebSearch {
                        id: "search-1".to_owned(),
                        query: "rust async".to_owned(),
                        action: None,
                    },
                ],
                status: TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
        });
        let events = map_server_notification(notification, "s");
        match &events[0] {
            UnifiedAgentEvent::Completed { result, .. } => {
                assert_eq!(result.tool_calls.len(), 1);
                assert_eq!(result.tool_calls[0].name, "web_search");
                assert_eq!(result.tool_calls[0].id, "search-1");
            }
            other => panic!("expected completed event, got {other:?}"),
        }
    }

    // ── ServerNotification: error ──

    #[test]
    fn error_notification_recoverable() {
        use codex_app_server_protocol::{ErrorNotification, TurnError};
        let notification = ServerNotification::Error(ErrorNotification {
            error: TurnError {
                message: "rate limited".to_owned(),
                codex_error_info: None,
                additional_details: None,
            },
            will_retry: true,
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::Error {
                session_id,
                message,
                recoverable,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(message, "rate limited");
                assert!(recoverable);
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn error_notification_non_recoverable() {
        use codex_app_server_protocol::{ErrorNotification, TurnError};
        let notification = ServerNotification::Error(ErrorNotification {
            error: TurnError {
                message: "fatal error".to_owned(),
                codex_error_info: None,
                additional_details: None,
            },
            will_retry: false,
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
        });
        let events = map_server_notification(notification, "s");
        match &events[0] {
            UnifiedAgentEvent::Error { recoverable, .. } => assert!(!recoverable),
            other => panic!("expected error event, got {other:?}"),
        }
    }

    // ── ServerNotification: context usage ──

    #[test]
    fn token_usage_produces_context_usage_event() {
        use codex_app_server_protocol::{
            ThreadTokenUsage, ThreadTokenUsageUpdatedNotification, TokenUsageBreakdown,
        };
        let notification =
            ServerNotification::ThreadTokenUsageUpdated(ThreadTokenUsageUpdatedNotification {
                thread_id: "t".to_owned(),
                turn_id: "t".to_owned(),
                token_usage: ThreadTokenUsage {
                    total: TokenUsageBreakdown {
                        total_tokens: 5000,
                        input_tokens: 4000,
                        cached_input_tokens: 1000,
                        output_tokens: 1000,
                        reasoning_output_tokens: 0,
                    },
                    last: TokenUsageBreakdown {
                        total_tokens: 100,
                        input_tokens: 80,
                        cached_input_tokens: 20,
                        output_tokens: 20,
                        reasoning_output_tokens: 0,
                    },
                    model_context_window: Some(128000),
                },
            });
        let events = map_server_notification(notification, "s");
        // Should produce ContextUsage + codex_token_usage progress
        assert!(!events.is_empty());
        match &events[0] {
            UnifiedAgentEvent::ContextUsage {
                session_id,
                used,
                total,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(*used, 5000);
                assert_eq!(*total, 128000);
            }
            other => panic!("expected context usage event, got {other:?}"),
        }
    }

    // ── ServerNotification: context compacted ──

    #[test]
    fn context_compacted_produces_event() {
        use codex_app_server_protocol::ContextCompactedNotification;
        let notification = ServerNotification::ContextCompacted(ContextCompactedNotification {
            thread_id: "t".to_owned(),
            turn_id: "t".to_owned(),
        });
        let events = map_server_notification(notification, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::ContextCompacted { session_id, .. } => {
                assert_eq!(session_id, "s");
            }
            other => panic!("expected context compacted, got {other:?}"),
        }
    }

    // ── ServerRequest: dynamic tool call ──

    #[test]
    fn dynamic_tool_call_maps_to_permission() {
        use codex_app_server_protocol::DynamicToolCallParams;
        let request = ServerRequest::DynamicToolCall {
            request_id: codex_app_server_protocol::RequestId::Integer(99),
            params: DynamicToolCallParams {
                thread_id: "t".to_owned(),
                turn_id: "t".to_owned(),
                call_id: "call-1".to_owned(),
                namespace: None,
                tool: "my_tool".to_owned(),
                arguments: serde_json::Value::Null,
            },
        };
        let events = map_server_request(request, "s");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UnifiedAgentEvent::PermissionRequest {
                session_id,
                request_id,
                tool_name,
                ..
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(request_id, "99");
                assert_eq!(tool_name, "dynamic_tool");
            }
            other => panic!("expected permission request, got {other:?}"),
        }
    }

    // ── thread_item_tool_call: simple variants ──

    #[test]
    fn tool_call_web_search() {
        use codex_app_server_protocol::ThreadItem;
        let item = ThreadItem::WebSearch {
            id: "ws-1".to_owned(),
            query: "rust async".to_owned(),
            action: None,
        };
        let result = thread_item_tool_call(&item);
        assert!(result.is_some());
        let tc = result.unwrap();
        assert_eq!(tc.name, "web_search");
        assert_eq!(tc.id, "ws-1");
    }

    #[test]
    fn tool_call_agent_message_returns_none() {
        use codex_app_server_protocol::ThreadItem;
        let item = ThreadItem::AgentMessage {
            id: "msg".to_owned(),
            text: "hello".to_owned(),
            phase: None,
            memory_citation: None,
        };
        assert!(thread_item_tool_call(&item).is_none());
    }

    #[test]
    fn tool_call_plan_returns_none() {
        use codex_app_server_protocol::ThreadItem;
        let item = ThreadItem::Plan {
            id: "plan-1".to_owned(),
            text: "my plan".to_owned(),
        };
        assert!(thread_item_tool_call(&item).is_none());
    }

    #[test]
    fn tool_call_reasoning_returns_none() {
        use codex_app_server_protocol::ThreadItem;
        let item = ThreadItem::Reasoning {
            id: "r-1".to_owned(),
            summary: vec![],
            content: vec![],
        };
        assert!(thread_item_tool_call(&item).is_none());
    }

    // ── request_id_to_string ──

    #[test]
    fn request_id_string_passthrough() {
        assert_eq!(
            request_id_to_string(&codex_app_server_protocol::RequestId::String(
                "abc".to_owned()
            )),
            "abc",
        );
    }

    #[test]
    fn request_id_integer_to_string() {
        assert_eq!(
            request_id_to_string(&codex_app_server_protocol::RequestId::Integer(42)),
            "42",
        );
    }
}
