//! Helper functions for event matching, runner selection, and approval relay.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use claude_runner::{
    ApprovalCreateRequest, ApprovalDecision, ApprovalDecisionRequest, ApprovalRequestRecord,
    RunnerSessionCommandRequest, RunnerSessionCommandResponse, RunnerSessionCreateRequest,
    RunnerSessionRecord, RunnerSessionStateUpdateRequest, RunnerSnapshot, RunnerState,
    SessionState as RunnerSessionState,
};
use reqwest::Client;
use uuid::Uuid;

use crate::types::{ApiError, SessionState, TimelineEvent, TimelineEventDetail, TimelineEventKind};

// ---------------------------------------------------------------------------
// Event matching helpers
// ---------------------------------------------------------------------------

pub(crate) fn event_kind(detail: &TimelineEventDetail) -> TimelineEventKind {
    match detail {
        TimelineEventDetail::RunnerRegistered { .. } => TimelineEventKind::RunnerRegistered,
        TimelineEventDetail::RunnerHeartbeat { .. } => TimelineEventKind::RunnerHeartbeat,
        TimelineEventDetail::SessionCreated { .. } => TimelineEventKind::SessionCreated,
        TimelineEventDetail::SessionStateChanged { .. } => TimelineEventKind::SessionStateChanged,
        TimelineEventDetail::ApprovalRequested { .. } => TimelineEventKind::ApprovalRequested,
        TimelineEventDetail::ApprovalResolved { .. } => TimelineEventKind::ApprovalResolved,
        TimelineEventDetail::ArtifactCreated { .. } => TimelineEventKind::ArtifactCreated,
        TimelineEventDetail::MessageDelta { .. } => TimelineEventKind::MessageDelta,
        TimelineEventDetail::MessageCommitted { .. } => TimelineEventKind::MessageCommitted,
        TimelineEventDetail::ToolStarted { .. } => TimelineEventKind::ToolStarted,
        TimelineEventDetail::ToolProgress { .. } => TimelineEventKind::ToolProgress,
        TimelineEventDetail::ToolFinished { .. } => TimelineEventKind::ToolFinished,
        TimelineEventDetail::ArtifactManifest { .. } => TimelineEventKind::ArtifactManifest,
        TimelineEventDetail::RuntimeError { .. } => TimelineEventKind::RuntimeError,
        TimelineEventDetail::DaemonPresenceChanged { .. } => {
            TimelineEventKind::DaemonPresenceChanged
        }
        TimelineEventDetail::SubtaskStarted { .. } => TimelineEventKind::SubtaskStarted,
        TimelineEventDetail::SubtaskProgress { .. } => TimelineEventKind::SubtaskProgress,
        TimelineEventDetail::SubtaskCompleted { .. } => TimelineEventKind::SubtaskCompleted,
        TimelineEventDetail::BatchProgress { .. } => TimelineEventKind::BatchProgress,
        TimelineEventDetail::ContextUsage { .. } => TimelineEventKind::ContextUsage,
        TimelineEventDetail::ContextOverflow { .. } => TimelineEventKind::ContextOverflow,
        TimelineEventDetail::ContextCompacted { .. } => TimelineEventKind::ContextCompacted,
    }
}

pub(crate) fn event_kind_name(kind: TimelineEventKind) -> &'static str {
    match kind {
        TimelineEventKind::RunnerRegistered => "runner_registered",
        TimelineEventKind::RunnerHeartbeat => "runner_heartbeat",
        TimelineEventKind::SessionCreated => "session_created",
        TimelineEventKind::SessionStateChanged => "session_state_changed",
        TimelineEventKind::ApprovalRequested => "approval_requested",
        TimelineEventKind::ApprovalResolved => "approval_resolved",
        TimelineEventKind::ArtifactCreated => "artifact_created",
        TimelineEventKind::MessageDelta => "message_delta",
        TimelineEventKind::MessageCommitted => "message_committed",
        TimelineEventKind::ToolStarted => "tool_started",
        TimelineEventKind::ToolProgress => "tool_progress",
        TimelineEventKind::ToolFinished => "tool_finished",
        TimelineEventKind::ArtifactManifest => "artifact_manifest",
        TimelineEventKind::RuntimeError => "runtime_error",
        TimelineEventKind::DaemonPresenceChanged => "daemon_presence_changed",
        TimelineEventKind::SubtaskStarted => "subtask_started",
        TimelineEventKind::SubtaskProgress => "subtask_progress",
        TimelineEventKind::SubtaskCompleted => "subtask_completed",
        TimelineEventKind::BatchProgress => "batch_progress",
        TimelineEventKind::ContextUsage => "context_usage",
        TimelineEventKind::ContextOverflow => "context_overflow",
        TimelineEventKind::ContextCompacted => "context_compacted",
    }
}

pub(crate) fn event_kind_name_for_detail(detail: &TimelineEventDetail) -> &'static str {
    event_kind_name(event_kind(detail))
}

pub(crate) fn is_approval_kind(kind: TimelineEventKind) -> bool {
    matches!(
        kind,
        TimelineEventKind::ApprovalRequested | TimelineEventKind::ApprovalResolved
    )
}

pub(crate) fn event_matches_kind(event: &TimelineEvent, kind: Option<TimelineEventKind>) -> bool {
    kind.is_none_or(|kind| event_kind(&event.detail) == kind)
}

pub(crate) fn approval_event_matches(
    event: &TimelineEvent,
    kind: Option<TimelineEventKind>,
) -> bool {
    is_approval_event(event) && event_matches_kind(event, kind)
}

pub(crate) fn is_approval_event(event: &TimelineEvent) -> bool {
    matches!(
        event.detail,
        TimelineEventDetail::ApprovalRequested { .. }
            | TimelineEventDetail::ApprovalResolved { .. }
    )
}

// ---------------------------------------------------------------------------
// Artifact helpers
// ---------------------------------------------------------------------------

pub(crate) fn artifact_file_path(root: &Path, artifact: &crate::types::ArtifactRecord) -> PathBuf {
    root.join(artifact.session_id.to_string())
        .join(format!("{}-{}", artifact.artifact_id, artifact.file_name))
}

pub(crate) fn sanitize_artifact_component(raw: &str, fallback: &str) -> String {
    let candidate = Path::new(raw)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(raw)
        .trim();
    let mut sanitized = candidate
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if character.is_control() => '_',
            _ => character,
        })
        .collect::<String>();
    sanitized = sanitized.trim_end_matches(['.', ' ']).to_owned();
    if sanitized.is_empty() {
        return fallback.to_owned();
    }
    if is_windows_reserved_file_name(&sanitized) {
        sanitized = append_reserved_suffix(&sanitized);
    }
    sanitized
}

pub(crate) fn build_content_disposition(file_name: &str) -> String {
    let ascii_fallback = file_name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '_',
        })
        .collect::<String>();
    let ascii_fallback = if ascii_fallback.trim_matches('_').is_empty() {
        "download".to_owned()
    } else {
        ascii_fallback
    };
    let encoded = encode_header_value(file_name);
    format!("attachment; filename=\"{ascii_fallback}\"; filename*=UTF-8''{encoded}")
}

fn encode_header_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push_str("%20"),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn is_windows_reserved_file_name(file_name: &str) -> bool {
    let stem = file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .trim()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn append_reserved_suffix(file_name: &str) -> String {
    match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            format!("{stem}_.{extension}")
        }
        _ => format!("{file_name}_"),
    }
}

// ---------------------------------------------------------------------------
// Runner helpers
// ---------------------------------------------------------------------------

pub(crate) fn runner_can_host(
    snapshot: &RunnerSnapshot,
    workspace_id: &str,
    lease_ttl_secs: u64,
) -> bool {
    runner_is_available(snapshot, lease_ttl_secs)
        && runner_has_capacity(snapshot)
        && snapshot
            .registration
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == workspace_id)
}

pub(crate) fn runner_has_capacity(snapshot: &RunnerSnapshot) -> bool {
    let max_parallel_sessions =
        usize::from(snapshot.registration.capabilities.max_parallel_sessions);
    snapshot.active_sessions + snapshot.queued_sessions < max_parallel_sessions
}

pub(crate) fn runner_is_available(snapshot: &RunnerSnapshot, lease_ttl_secs: u64) -> bool {
    !matches!(
        snapshot.state,
        RunnerState::Draining | RunnerState::Offline | RunnerState::Unhealthy
    ) && snapshot.last_seen_at >= Utc::now() - Duration::seconds(lease_ttl_secs as i64)
}

pub(crate) fn runner_rank(state: RunnerState) -> u8 {
    match state {
        RunnerState::Idle => 0,
        RunnerState::Busy => 1,
        RunnerState::Starting => 2,
        RunnerState::Draining => 3,
        RunnerState::Unhealthy => 4,
        RunnerState::Offline => 5,
    }
}

// ---------------------------------------------------------------------------
// Session state conversion helpers
// ---------------------------------------------------------------------------

pub(crate) fn session_state_after_approval(
    decision: ApprovalDecision,
    has_pending_approvals: bool,
) -> SessionState {
    if has_pending_approvals {
        SessionState::WaitingApproval
    } else {
        match decision {
            ApprovalDecision::Approved => SessionState::Running,
            ApprovalDecision::Denied => SessionState::Failed,
            ApprovalDecision::Cancelled => SessionState::Cancelled,
        }
    }
}

pub(crate) fn session_state_from_runner(state: RunnerSessionState) -> SessionState {
    match state {
        RunnerSessionState::Pending | RunnerSessionState::Starting => SessionState::Assigned,
        RunnerSessionState::Running => SessionState::Running,
        RunnerSessionState::WaitingApproval => SessionState::WaitingApproval,
        RunnerSessionState::Completed => SessionState::Completed,
        RunnerSessionState::Failed => SessionState::Failed,
        RunnerSessionState::Cancelled => SessionState::Cancelled,
    }
}

pub(crate) fn session_state_to_runner(state: SessionState) -> RunnerSessionState {
    match state {
        SessionState::Pending => RunnerSessionState::Pending,
        SessionState::Assigned => RunnerSessionState::Starting,
        SessionState::Running => RunnerSessionState::Running,
        SessionState::WaitingApproval => RunnerSessionState::WaitingApproval,
        SessionState::Completed => RunnerSessionState::Completed,
        SessionState::Failed => RunnerSessionState::Failed,
        SessionState::Cancelled => RunnerSessionState::Cancelled,
    }
}

// ---------------------------------------------------------------------------
// Runner relay helpers
// ---------------------------------------------------------------------------

pub(crate) fn runner_public_base_url(runner: &RunnerSnapshot) -> Result<&str, ApiError> {
    runner
        .registration
        .public_base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ApiError::service_unavailable(format!(
                "runner `{}` does not expose a public base URL",
                runner.registration.runner_id
            ))
        })
}

pub(crate) fn runner_uses_pull_commands(runner: &RunnerSnapshot) -> bool {
    runner
        .registration
        .public_base_url
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
}

fn authorize_runner_request(
    builder: reqwest::RequestBuilder,
    runner: &RunnerSnapshot,
) -> reqwest::RequestBuilder {
    if let Some(token) = runner.registration.auth_token.as_deref() {
        builder.bearer_auth(token)
    } else {
        builder
    }
}

pub(crate) async fn dispatch_session_to_runner(
    client: &Client,
    runner: &RunnerSnapshot,
    request: &RunnerSessionCreateRequest,
) -> Result<RunnerSessionRecord, ApiError> {
    let base_url = runner_public_base_url(runner)?;
    let response = authorize_runner_request(
        client.post(format!("{}/v1/sessions", base_url.trim_end_matches('/'))),
        runner,
    )
    .json(request)
    .send()
    .await
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to dispatch session to runner `{}`: {error}",
            runner.registration.runner_id
        ))
    })?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "runner `{}` rejected session dispatch: {error}",
            runner.registration.runner_id
        ))
    })?;
    response
        .json::<RunnerSessionRecord>()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "failed to decode session dispatch response from runner `{}`: {error}",
                runner.registration.runner_id
            ))
        })
}

pub(crate) async fn update_runner_session_state(
    client: &Client,
    runner: &RunnerSnapshot,
    session_id: Uuid,
    request: &RunnerSessionStateUpdateRequest,
) -> Result<RunnerSessionRecord, ApiError> {
    let base_url = runner_public_base_url(runner)?;
    let response = authorize_runner_request(
        client.post(format!(
            "{}/v1/sessions/{session_id}/state",
            base_url.trim_end_matches('/')
        )),
        runner,
    )
    .json(request)
    .send()
    .await
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to update session `{session_id}` on runner `{}`: {error}",
            runner.registration.runner_id
        ))
    })?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "runner `{}` rejected state update for session `{session_id}`: {error}",
            runner.registration.runner_id
        ))
    })?;
    response
        .json::<RunnerSessionRecord>()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "failed to decode state update response from runner `{}`: {error}",
                runner.registration.runner_id
            ))
        })
}

pub(crate) async fn relay_approval_to_runner(
    client: &Client,
    runner: &RunnerSnapshot,
    session_id: Uuid,
    request: &ApprovalCreateRequest,
) -> Result<ApprovalRequestRecord, ApiError> {
    let base_url = runner_public_base_url(runner)?;
    let response = authorize_runner_request(
        client.post(format!(
            "{}/v1/sessions/{session_id}/approvals",
            base_url.trim_end_matches('/')
        )),
        runner,
    )
    .json(request)
    .send()
    .await
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to relay approval for session `{session_id}` to runner `{}`: {error}",
            runner.registration.runner_id
        ))
    })?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "runner `{}` rejected approval relay for session `{session_id}`: {error}",
            runner.registration.runner_id
        ))
    })?;
    response
        .json::<ApprovalRequestRecord>()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "failed to decode approval relay response from runner `{}`: {error}",
                runner.registration.runner_id
            ))
        })
}

pub(crate) async fn relay_approval_decision_to_runner(
    client: &Client,
    runner: &RunnerSnapshot,
    approval_id: Uuid,
    request: &ApprovalDecisionRequest,
) -> Result<ApprovalRequestRecord, ApiError> {
    let base_url = runner_public_base_url(runner)?;
    let response = authorize_runner_request(
        client.post(format!(
            "{}/v1/approvals/{approval_id}/decision",
            base_url.trim_end_matches('/')
        )),
        runner,
    )
    .json(request)
    .send()
    .await
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to relay approval decision `{approval_id}` to runner `{}`: {error}",
            runner.registration.runner_id
        ))
    })?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "runner `{}` rejected approval decision `{approval_id}`: {error}",
            runner.registration.runner_id
        ))
    })?;
    response
        .json::<ApprovalRequestRecord>()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "failed to decode approval decision response from runner `{}`: {error}",
                runner.registration.runner_id
            ))
        })
}

pub(crate) async fn dispatch_session_command_to_runner(
    client: &Client,
    runner: &RunnerSnapshot,
    session_id: Uuid,
    request: &RunnerSessionCommandRequest,
) -> Result<RunnerSessionCommandResponse, ApiError> {
    let base_url = runner_public_base_url(runner)?;
    let response = authorize_runner_request(
        client.post(format!(
            "{}/v1/sessions/{session_id}/commands",
            base_url.trim_end_matches('/')
        )),
        runner,
    )
    .json(request)
    .send()
    .await
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to relay session command for `{session_id}` to runner `{}`: {error}",
            runner.registration.runner_id
        ))
    })?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad_gateway(format!(
            "runner `{}` rejected session command for `{session_id}`: {error}",
            runner.registration.runner_id
        ))
    })?;
    response
        .json::<RunnerSessionCommandResponse>()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "failed to decode session command response from runner `{}`: {error}",
                runner.registration.runner_id
            ))
        })
}

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

pub(crate) fn parse_socket_addr(raw: &str) -> Result<SocketAddr> {
    SocketAddr::from_str(raw).with_context(|| format!("invalid socket address `{raw}`"))
}

pub(crate) fn parse_env_number<T>(key: &str) -> Option<T>
where
    T: FromStr,
{
    read_env(key).and_then(|value| value.parse::<T>().ok())
}

pub(crate) fn read_env(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{build_content_disposition, sanitize_artifact_component};

    #[test]
    fn sanitize_artifact_component_preserves_cjk_and_replaces_reserved_chars() {
        assert_eq!(
            sanitize_artifact_component("检查报告.txt", "artifact.bin"),
            "检查报告.txt"
        );
        assert_eq!(
            sanitize_artifact_component("a/b:c?.txt", "artifact.bin"),
            "b_c_.txt"
        );
    }

    #[test]
    fn sanitize_artifact_component_guards_windows_reserved_names() {
        assert_eq!(sanitize_artifact_component("CON", "artifact.bin"), "CON_");
        assert_eq!(
            sanitize_artifact_component("NUL.txt", "artifact.bin"),
            "NUL_.txt"
        );
    }

    #[test]
    fn build_content_disposition_emits_ascii_and_utf8_names() {
        let header = build_content_disposition("检查报告.txt");
        assert!(header.contains("filename=\"____.txt\""));
        assert!(header.contains("filename*=UTF-8''%E6%A3%80%E6%9F%A5%E6%8A%A5%E5%91%8A.txt"));
    }
}
