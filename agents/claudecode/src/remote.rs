use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use claude_control_plane::{
    ArtifactCreateRequest as RemoteArtifactCreateRequest, ArtifactRecord as RemoteArtifactRecord,
    BootstrapClaimRequest as RemoteBootstrapClaimRequest,
    BootstrapClaimResponse as RemoteBootstrapClaimResponse,
    ControlPlaneMeta as RemoteControlPlaneMeta, CreateSessionRequest as RemoteCreateSessionRequest,
    DeviceKind as RemoteDeviceKind, PairingAcceptRequest as RemotePairingAcceptRequest,
    PairingAcceptResponse as RemotePairingAcceptResponse,
    PairingOfferCreateRequest as RemotePairingOfferCreateRequest,
    PairingOfferCreateResponse as RemotePairingOfferCreateResponse,
    SessionRecord as RemoteSessionRecord, SessionState as RemoteSessionState,
    SessionStateUpdateRequest as RemoteSessionStateUpdateRequest,
    TimelineEvent as RemoteTimelineEvent, TimelineEventDetail as RemoteTimelineEventDetail,
    TrustedDeviceRecord as RemoteTrustedDeviceRecord,
};
use claude_runner::{
    ApprovalCreateRequest as SharedApprovalCreateRequest,
    ApprovalDecisionRequest as SharedApprovalDecisionRequest,
    ApprovalRequestRecord as RemoteApprovalRecord, ApprovalState as RemoteApprovalState,
    ListResponse as RemoteListResponse, RunnerSnapshot as RemoteRunnerSnapshot,
    RunnerState as RemoteRunnerState,
};
use futures::StreamExt;
use reqwest::Client;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Error as TungsteniteError,
        client::IntoClientRequest,
        http::{HeaderValue, Request as WsRequest, header::AUTHORIZATION as WS_AUTHORIZATION},
        protocol::Message as TungsteniteMessage,
    },
};
use tracing::warn;
use uuid::Uuid;

use crate::cli::{
    RemoteApprovalCreateArgs, RemoteApprovalRespondArgs, RemoteApprovalShowArgs,
    RemoteApprovalsCommand, RemoteApprovalsListArgs, RemoteArtifactDownloadArgs,
    RemoteArtifactShowArgs, RemoteArtifactUploadArgs, RemoteArtifactsCommand,
    RemoteArtifactsListArgs, RemoteAuthCommand, RemoteBootstrapArgs, RemoteCommand,
    RemoteDeviceKindValue, RemoteDevicesListArgs, RemoteEventKindValue, RemoteEventsArgs,
    RemoteMetaArgs, RemotePairingAcceptArgs, RemotePairingOfferArgs, RemoteRunnerShowArgs,
    RemoteRunnersCommand, RemoteRunnersListArgs, RemoteSessionCommandResponseValue,
    RemoteSessionCreateArgs, RemoteSessionFollowArgs, RemoteSessionInterruptArgs,
    RemoteSessionPromptArgs, RemoteSessionShowArgs, RemoteSessionStateArgs, RemoteSessionsCommand,
    RemoteSessionsListArgs, RemoteTargetArgs,
};

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct RemoteErrorEnvelope {
    error: RemoteErrorDetail,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RemoteErrorDetail {
    message: String,
}

pub(crate) trait StateLabel {
    fn label(&self) -> &'static str;
}

impl StateLabel for RemoteRunnerState {
    fn label(&self) -> &'static str {
        match self {
            RemoteRunnerState::Starting => "starting",
            RemoteRunnerState::Idle => "idle",
            RemoteRunnerState::Busy => "busy",
            RemoteRunnerState::Draining => "draining",
            RemoteRunnerState::Unhealthy => "unhealthy",
            RemoteRunnerState::Offline => "offline",
        }
    }
}

impl StateLabel for RemoteSessionState {
    fn label(&self) -> &'static str {
        match self {
            RemoteSessionState::Pending => "pending",
            RemoteSessionState::Assigned => "assigned",
            RemoteSessionState::Running => "running",
            RemoteSessionState::WaitingApproval => "waiting_approval",
            RemoteSessionState::Completed => "completed",
            RemoteSessionState::Failed => "failed",
            RemoteSessionState::Cancelled => "cancelled",
        }
    }
}

impl StateLabel for RemoteApprovalState {
    fn label(&self) -> &'static str {
        match self {
            RemoteApprovalState::Pending => "pending",
            RemoteApprovalState::Approved => "approved",
            RemoteApprovalState::Denied => "denied",
            RemoteApprovalState::Cancelled => "cancelled",
        }
    }
}

impl StateLabel for RemoteDeviceKind {
    fn label(&self) -> &'static str {
        match self {
            RemoteDeviceKind::Runner => "runner",
            RemoteDeviceKind::Browser => "browser",
            RemoteDeviceKind::Cli => "cli",
        }
    }
}

impl From<RemoteDeviceKindValue> for RemoteDeviceKind {
    fn from(value: RemoteDeviceKindValue) -> Self {
        match value {
            RemoteDeviceKindValue::Runner => Self::Runner,
            RemoteDeviceKindValue::Browser => Self::Browser,
            RemoteDeviceKindValue::Cli => Self::Cli,
        }
    }
}

pub(crate) fn require_control_plane_url(target: &RemoteTargetArgs) -> Result<String> {
    target
        .control_plane_url
        .clone()
        .ok_or_else(|| anyhow!("missing control plane URL; pass --control-plane-url or set REMOTE_CODE_CONTROL_PLANE_URL"))
}

pub(crate) fn parse_repeated_key_value_args(
    flag_name: &str,
    values: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut parsed = BTreeMap::new();
    for value in values {
        let (key, entry_value) = value
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid {flag_name} `{value}`; expected key=value"))?;
        let key = key.trim();
        let entry_value = entry_value.trim();
        if key.is_empty() {
            return Err(anyhow!(
                "invalid {flag_name} `{value}`; key cannot be empty"
            ));
        }
        parsed.insert(key.to_owned(), entry_value.to_owned());
    }
    Ok(parsed)
}

pub(crate) fn normalize_remote_base_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/').to_owned();
    if trimmed.is_empty() {
        return Err(anyhow!("control plane URL is empty"));
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(anyhow!(
            "control plane URL must start with http:// or https://"
        ));
    }
    Ok(trimmed)
}

pub(crate) fn build_remote_http_url(base_url: &str, path: &str) -> Result<String> {
    Ok(format!(
        "{}{}",
        normalize_remote_base_url(base_url)?,
        normalize_remote_request_path(path)
    ))
}

pub(crate) fn build_remote_ws_url(base_url: &str, path: &str) -> Result<String> {
    let base = normalize_remote_base_url(base_url)?;
    let url = if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}{}", normalize_remote_request_path(path))
    } else if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}{}", normalize_remote_request_path(path))
    } else {
        return Err(anyhow!(
            "control plane URL must start with http:// or https://"
        ));
    };
    Ok(url)
}

pub(crate) fn build_remote_ws_request(base_url: &str, path: &str) -> Result<WsRequest<()>> {
    build_remote_ws_request_with_token(base_url, path, remote_control_plane_auth_token().as_deref())
}

pub(crate) fn build_remote_ws_request_with_token(
    base_url: &str,
    path: &str,
    auth_token: Option<&str>,
) -> Result<WsRequest<()>> {
    let ws_url = build_remote_ws_url(base_url, path)?;
    let mut request = ws_url.into_client_request()?;
    if let Some(token) = auth_token.map(str::trim).filter(|token| !token.is_empty()) {
        let header_value = HeaderValue::from_str(&format!("Bearer {token}"))?;
        request.headers_mut().insert(WS_AUTHORIZATION, header_value);
    }
    Ok(request)
}

pub(crate) async fn remote_get_json<T>(base_url: &str, path: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let client = Client::new();
    let response = authorize_remote_request(client.get(build_remote_http_url(base_url, path)?))
        .send()
        .await?;
    decode_remote_json_response(response).await
}

pub(crate) async fn remote_get_bytes(base_url: &str, path: &str) -> Result<Vec<u8>> {
    let client = Client::new();
    let response = authorize_remote_request(client.get(build_remote_http_url(base_url, path)?))
        .send()
        .await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if status.is_success() {
        return Ok(bytes.to_vec());
    }

    let message = serde_json::from_slice::<RemoteErrorEnvelope>(&bytes).map_or_else(
        |_| String::from_utf8_lossy(&bytes).trim().to_owned(),
        |error| error.error.message,
    );
    Err(anyhow!(
        "control plane request failed with HTTP {}: {}",
        status.as_u16(),
        if message.is_empty() {
            "unknown error"
        } else {
            message.as_str()
        }
    ))
}

pub(crate) async fn remote_post_json<I, O>(base_url: &str, path: &str, input: &I) -> Result<O>
where
    I: serde::Serialize,
    O: serde::de::DeserializeOwned,
{
    let client = Client::new();
    let response = authorize_remote_request(client.post(build_remote_http_url(base_url, path)?))
        .json(input)
        .send()
        .await?;
    decode_remote_json_response(response).await
}

async fn decode_remote_json_response<T>(response: reqwest::Response) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    let bytes = response.bytes().await?;
    if status.is_success() {
        return Ok(serde_json::from_slice(&bytes)?);
    }

    let message = serde_json::from_slice::<RemoteErrorEnvelope>(&bytes).map_or_else(
        |_| String::from_utf8_lossy(&bytes).trim().to_owned(),
        |error| error.error.message,
    );
    Err(anyhow!(
        "control plane request failed with HTTP {}: {}",
        status.as_u16(),
        if message.is_empty() {
            "unknown error"
        } else {
            &message
        }
    ))
}

fn normalize_remote_request_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

fn authorize_remote_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(token) = remote_control_plane_auth_token() {
        builder.bearer_auth(token)
    } else {
        builder
    }
}

fn remote_control_plane_auth_token() -> Option<String> {
    env::var("REMOTE_CODE_CONTROL_PLANE_AUTH_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn encode_remote_path_segment(raw: &str) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                #[allow(clippy::format_push_string)]
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

fn is_retryable_remote_follow_connect_error(error: &TungsteniteError) -> bool {
    match error {
        TungsteniteError::Http(response) => {
            !matches!(response.status().as_u16(), 400 | 401 | 403 | 404 | 422)
        }
        TungsteniteError::Url(_) => false,
        _ => true,
    }
}

fn format_remote_follow_connect_error(error: &TungsteniteError) -> String {
    match error {
        TungsteniteError::Http(response) => {
            format!(
                "remote follow websocket handshake failed with HTTP {}",
                response.status().as_u16()
            )
        }
        TungsteniteError::Url(message) => format!("invalid remote websocket URL: {message}"),
        _ => format!("remote follow connect failed: {error}"),
    }
}

fn print_remote_session_summary(session: &RemoteSessionRecord) {
    println!("Remote session {}", session.session_id);
    println!("- workspace: {}", session.workspace_id);
    println!("- state: {}", session.state.label());
    println!(
        "- runner: {}",
        session.owner_runner_id.as_deref().unwrap_or("(unassigned)")
    );
    println!("- created: {}", session.created_at);
    println!("- updated: {}", session.updated_at);
    if !session.metadata.is_empty() {
        println!("- metadata: {}", format_remote_metadata(&session.metadata));
    }
}

fn print_remote_meta(meta: &RemoteControlPlaneMeta) {
    println!("Remote control plane {}", meta.service);
    println!("- version: {}", meta.version);
    println!("- phase: {}", meta.phase);
    println!("- bind: {}", meta.bind);
    println!(
        "- public base URL: {}",
        meta.public_base_url
            .as_deref()
            .unwrap_or("(missing-public-base-url)")
    );
    println!("- runner lease TTL: {}s", meta.runner_lease_ttl_secs);
    println!("- profile dir: {}", meta.profile_dir);
    println!("- artifact root dir: {}", meta.artifact_root_dir);
}

fn print_remote_device_summary(device: &RemoteTrustedDeviceRecord) {
    println!("Trusted device {}", device.device_id);
    println!("- name: {}", device.name);
    println!("- kind: {}", device.kind.label());
    println!("- owner: {}", if device.owner { "yes" } else { "no" });
    println!("- created: {}", device.created_at);
    println!("- last seen: {}", device.last_seen_at);
    if let Some(created_by_device_id) = device.created_by_device_id {
        println!("- created by: {created_by_device_id}");
    }
}

fn print_remote_access_token_help(access_token: &str) {
    println!("Access token");
    println!("{access_token}");
    println!(
        "Set REMOTE_CODE_CONTROL_PLANE_AUTH_TOKEN to this value before using protected remote commands."
    );
}

fn print_remote_pairing_offer(offer: &RemotePairingOfferCreateResponse) {
    println!("Pairing offer {}", offer.offer_id);
    println!("- target name: {}", offer.device_name);
    println!("- target kind: {}", offer.device_kind.label());
    println!("- created: {}", offer.created_at);
    println!("- expires: {}", offer.expires_at);
    println!("- pairing secret: {}", offer.pairing_secret);
    if let Some(pairing_url) = &offer.pairing_url {
        println!("- pairing URL: {pairing_url}");
    }
}

fn print_remote_runner_summary(runner: &RemoteRunnerSnapshot) {
    println!("Remote runner {}", runner.registration.runner_id);
    println!("- state: {}", runner.state.label());
    println!("- active sessions: {}", runner.active_sessions);
    println!("- queued sessions: {}", runner.queued_sessions);
    println!("- registered: {}", runner.registered_at);
    println!("- last seen: {}", runner.last_seen_at);
    if let Some(public_base_url) = &runner.registration.public_base_url {
        println!("- public base URL: {public_base_url}");
    }
    if let Some(control_plane_url) = &runner.registration.control_plane_url {
        println!("- control plane URL: {control_plane_url}");
    }
    if !runner.registration.labels.is_empty() {
        println!(
            "- labels: {}",
            format_remote_metadata(&runner.registration.labels)
        );
    }
    if !runner.registration.workspaces.is_empty() {
        let workspaces = runner
            .registration
            .workspaces
            .iter()
            .map(|workspace| {
                format!(
                    "{}={} ({})",
                    workspace.workspace_id,
                    workspace.root_dir.display(),
                    if workspace.writable {
                        "writable"
                    } else {
                        "read-only"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!("- workspaces: {workspaces}");
    }
}

fn print_remote_approval_summary(approval: &RemoteApprovalRecord) {
    println!("Remote approval {}", approval.approval_id);
    println!("- state: {}", approval.state.label());
    println!("- session: {}", approval.session_id);
    println!(
        "- runner: {}",
        if approval.runner_id.is_empty() {
            "(unassigned-runner)"
        } else {
            approval.runner_id.as_str()
        }
    );
    println!("- title: {}", approval.title);
    println!("- description: {}", approval.description);
    println!("- created: {}", approval.created_at);
    println!("- updated: {}", approval.updated_at);
    if let Some(responder) = &approval.responder {
        println!("- responder: {responder}");
    }
    if let Some(note) = &approval.note {
        println!("- note: {note}");
    }
    if !approval.metadata.is_empty() {
        println!("- metadata: {}", format_remote_metadata(&approval.metadata));
    }
}

fn print_remote_artifact_summary(artifact: &RemoteArtifactRecord) {
    println!("Remote artifact {}", artifact.artifact_id);
    println!("- session: {}", artifact.session_id);
    println!(
        "- runner: {}",
        artifact
            .runner_id
            .as_deref()
            .unwrap_or("(unassigned-runner)")
    );
    println!("- name: {}", artifact.name);
    println!("- file: {}", artifact.file_name);
    println!("- media type: {}", artifact.media_type);
    println!("- size: {}B", artifact.size_bytes);
    println!("- created: {}", artifact.created_at);
    if !artifact.metadata.is_empty() {
        println!("- metadata: {}", format_remote_metadata(&artifact.metadata));
    }
}

fn print_remote_events(events: &[RemoteTimelineEvent]) {
    if events.is_empty() {
        println!("No remote events found.");
        return;
    }
    for event in events {
        println!(
            "{}  {}  {}  session={}  runner={}  {}",
            event.sequence,
            event.recorded_at,
            remote_event_kind(&event.detail),
            event
                .session_id
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            event.runner_id.as_deref().unwrap_or("-"),
            remote_event_summary(&event.detail)
        );
    }
}

fn format_remote_metadata(metadata: &BTreeMap<String, String>) -> String {
    metadata
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn remote_approvals_path(
    session_id: Option<Uuid>,
    runner_id: Option<&str>,
) -> Result<String> {
    match (session_id, runner_id) {
        (Some(_), Some(_)) => Err(anyhow!(
            "choose either --session-id or --runner-id when listing approvals"
        )),
        (Some(session_id), None) => Ok(format!("/v1/sessions/{session_id}/approvals")),
        (None, Some(runner_id)) => Ok(format!(
            "/v1/runners/{}/approvals",
            encode_remote_path_segment(runner_id)
        )),
        (None, None) => Ok("/v1/approvals".to_owned()),
    }
}

pub(crate) fn remote_approval_path(approval_id: Uuid) -> String {
    format!("/v1/approvals/{approval_id}")
}

pub(crate) fn remote_runner_path(runner_id: &str) -> String {
    format!("/v1/runners/{}", encode_remote_path_segment(runner_id))
}

pub(crate) fn remote_sessions_path(
    runner_id: Option<&str>,
    workspace_id: Option<&str>,
    state: Option<RemoteSessionState>,
) -> String {
    let mut path = match runner_id {
        Some(runner_id) => format!(
            "/v1/runners/{}/sessions",
            encode_remote_path_segment(runner_id)
        ),
        None => "/v1/sessions".to_owned(),
    };
    let mut query = Vec::new();
    if let Some(workspace_id) = workspace_id {
        query.push(format!(
            "workspace_id={}",
            encode_remote_path_segment(workspace_id)
        ));
    }
    if let Some(state) = state {
        query.push(format!("state={}", state.label()));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    path
}

pub(crate) fn remote_session_state_path(session_id: Uuid) -> String {
    format!("/v1/sessions/{session_id}/state")
}

pub(crate) fn remote_session_commands_path(session_id: Uuid) -> String {
    format!("/v1/sessions/{session_id}/commands")
}

pub(crate) fn remote_approvals_stream_path(
    session_id: Option<Uuid>,
    runner_id: Option<&str>,
    after: Option<u64>,
) -> Result<String> {
    let mut path = match (session_id, runner_id) {
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "choose either --session-id or --runner-id when following approvals"
            ));
        }
        (Some(session_id), None) => format!("/v1/sessions/{session_id}/approvals/stream"),
        (None, Some(runner_id)) => format!(
            "/v1/runners/{}/approvals/stream",
            encode_remote_path_segment(runner_id)
        ),
        (None, None) => "/v1/approvals/stream".to_owned(),
    };
    if let Some(after) = after {
        #[allow(clippy::format_push_string)]
        path.push_str(&format!("?after={after}"));
    }
    Ok(path)
}

pub(crate) fn remote_artifacts_path(
    session_id: Option<Uuid>,
    runner_id: Option<&str>,
) -> Result<String> {
    match (session_id, runner_id) {
        (Some(_), Some(_)) => Err(anyhow!(
            "choose either --session-id or --runner-id when listing artifacts"
        )),
        (Some(session_id), None) => Ok(format!("/v1/sessions/{session_id}/artifacts")),
        (None, Some(runner_id)) => Ok(format!(
            "/v1/runners/{}/artifacts",
            encode_remote_path_segment(runner_id)
        )),
        (None, None) => Ok("/v1/artifacts".to_owned()),
    }
}

pub(crate) fn remote_artifact_download_path(artifact_id: Uuid) -> String {
    format!("/v1/artifacts/{artifact_id}/download")
}

pub(crate) fn default_artifact_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("artifact")
        .to_owned()
}

pub(crate) fn default_artifact_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("artifact.bin")
        .to_owned()
}

pub(crate) fn remote_events_path(
    session_id: Option<Uuid>,
    runner_id: Option<&str>,
    after: Option<u64>,
    limit: usize,
    kind: Option<RemoteEventKindValue>,
) -> Result<String> {
    let mut path = match (session_id, runner_id) {
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "choose either --session-id or --runner-id when listing events"
            ));
        }
        (Some(session_id), None) => format!("/v1/sessions/{session_id}/events"),
        (None, Some(runner_id)) => format!(
            "/v1/runners/{}/events",
            encode_remote_path_segment(runner_id)
        ),
        (None, None) => "/v1/events".to_owned(),
    };
    let mut query = Vec::new();
    if let Some(after) = after {
        query.push(format!("after={after}"));
    }
    query.push(format!("limit={}", limit.clamp(1, 200)));
    if let Some(kind) = kind {
        query.push(format!("kind={}", kind.as_str()));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    Ok(path)
}

pub(crate) fn remote_events_stream_path(
    session_id: Option<Uuid>,
    runner_id: Option<&str>,
    after: Option<u64>,
    kind: Option<RemoteEventKindValue>,
) -> Result<String> {
    let mut path = match (session_id, runner_id) {
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "choose either --session-id or --runner-id when following events"
            ));
        }
        (Some(session_id), None) => format!("/v1/sessions/{session_id}/events/stream"),
        (None, Some(runner_id)) => format!(
            "/v1/runners/{}/events/stream",
            encode_remote_path_segment(runner_id)
        ),
        (None, None) => "/v1/events/stream".to_owned(),
    };
    let mut query = Vec::new();
    if let Some(after) = after {
        query.push(format!("after={after}"));
    }
    if let Some(kind) = kind {
        query.push(format!("kind={}", kind.as_str()));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    Ok(path)
}

fn remote_event_kind(detail: &RemoteTimelineEventDetail) -> &'static str {
    match detail {
        RemoteTimelineEventDetail::RunnerRegistered { .. } => "runner_registered",
        RemoteTimelineEventDetail::RunnerHeartbeat { .. } => "runner_heartbeat",
        RemoteTimelineEventDetail::SessionCreated { .. } => "session_created",
        RemoteTimelineEventDetail::SessionStateChanged { .. } => "session_state_changed",
        RemoteTimelineEventDetail::ApprovalRequested { .. } => "approval_requested",
        RemoteTimelineEventDetail::ApprovalResolved { .. } => "approval_resolved",
        RemoteTimelineEventDetail::ArtifactCreated { .. } => "artifact_created",
        RemoteTimelineEventDetail::MessageDelta { .. } => "message_delta",
        RemoteTimelineEventDetail::MessageCommitted { .. } => "message_committed",
        RemoteTimelineEventDetail::ToolStarted { .. } => "tool_started",
        RemoteTimelineEventDetail::ToolProgress { .. } => "tool_progress",
        RemoteTimelineEventDetail::ToolFinished { .. } => "tool_finished",
        RemoteTimelineEventDetail::ArtifactManifest { .. } => "artifact_manifest",
        RemoteTimelineEventDetail::RuntimeError { .. } => "runtime_error",
        RemoteTimelineEventDetail::DaemonPresenceChanged { .. } => "daemon_presence_changed",
        RemoteTimelineEventDetail::SubtaskStarted { .. } => "subtask_started",
        RemoteTimelineEventDetail::SubtaskProgress { .. } => "subtask_progress",
        RemoteTimelineEventDetail::SubtaskCompleted { .. } => "subtask_completed",
        RemoteTimelineEventDetail::BatchProgress { .. } => "batch_progress",
        RemoteTimelineEventDetail::ContextUsage { .. } => "context_usage",
        RemoteTimelineEventDetail::ContextOverflow { .. } => "context_overflow",
        RemoteTimelineEventDetail::ContextCompacted { .. } => "context_compacted",
    }
}

fn remote_event_summary(detail: &RemoteTimelineEventDetail) -> String {
    match detail {
        RemoteTimelineEventDetail::RunnerRegistered {
            workspace_ids,
            state,
            ..
        } => format!(
            "workspaces={} state={}",
            workspace_ids.join(","),
            state.label()
        ),
        RemoteTimelineEventDetail::RunnerHeartbeat {
            state,
            active_sessions,
            queued_sessions,
            ..
        } => format!(
            "state={} active={} queued={}",
            state.label(),
            active_sessions,
            queued_sessions
        ),
        RemoteTimelineEventDetail::SessionCreated {
            workspace_id,
            owner_runner_id,
            state,
        } => format!(
            "workspace={} runner={} state={}",
            workspace_id,
            owner_runner_id.as_deref().unwrap_or("(unassigned)"),
            state.label()
        ),
        RemoteTimelineEventDetail::SessionStateChanged {
            previous_state,
            state,
        } => format!(
            "previous_state={} state={}",
            previous_state.label(),
            state.label()
        ),
        RemoteTimelineEventDetail::ApprovalRequested { title, state, .. } => {
            format!("title={title} state={}", state.label())
        }
        RemoteTimelineEventDetail::ApprovalResolved {
            state, responder, ..
        } => format!(
            "state={} responder={}",
            state.label(),
            responder.as_deref().unwrap_or("(none)")
        ),
        RemoteTimelineEventDetail::ArtifactCreated {
            file_name,
            media_type,
            size_bytes,
            ..
        } => format!("file={file_name} media_type={media_type} size={size_bytes}B"),
        RemoteTimelineEventDetail::MessageDelta {
            role,
            delta,
            message_id,
        } => format!(
            "role={role:?} message_id={} delta={}",
            message_id.as_deref().unwrap_or("(none)"),
            truncate_remote_preview(delta, 80)
        ),
        RemoteTimelineEventDetail::MessageCommitted {
            role,
            text,
            message_id,
        } => format!(
            "role={role:?} message_id={} text={}",
            message_id.as_deref().unwrap_or("(none)"),
            truncate_remote_preview(text, 80)
        ),
        RemoteTimelineEventDetail::ToolStarted {
            tool_call_id,
            tool_name,
        } => format!("tool_call_id={tool_call_id} tool_name={tool_name}"),
        RemoteTimelineEventDetail::ToolProgress {
            tool_call_id,
            tool_name,
            delta,
            elapsed_time_seconds,
        } => format!(
            "tool_call_id={} tool_name={} delta={} elapsed={}s",
            tool_call_id.as_deref().unwrap_or("(none)"),
            tool_name.as_deref().unwrap_or("(none)"),
            delta
                .as_deref()
                .map(|value| truncate_remote_preview(value, 80))
                .unwrap_or_else(|| "(none)".to_owned()),
            elapsed_time_seconds.map_or_else(|| "(none)".to_owned(), |value| value.to_string())
        ),
        RemoteTimelineEventDetail::ToolFinished {
            tool_call_id,
            tool_name,
            is_error,
            summary,
        } => format!(
            "tool_call_id={tool_call_id} tool_name={tool_name} is_error={is_error} summary={}",
            summary
                .as_deref()
                .map(|value| truncate_remote_preview(value, 80))
                .unwrap_or_else(|| "(none)".to_owned())
        ),
        RemoteTimelineEventDetail::ArtifactManifest { artifact_ids } => {
            format!("artifact_ids={}", artifact_ids.len())
        }
        RemoteTimelineEventDetail::RuntimeError { message } => {
            format!("message={}", truncate_remote_preview(message, 80))
        }
        RemoteTimelineEventDetail::DaemonPresenceChanged { state } => {
            format!("state={state:?}")
        }
        RemoteTimelineEventDetail::SubtaskStarted {
            task_id,
            parent_task_id,
            description,
            depth,
        } => {
            format!(
                "task_id={task_id} parent={:?} depth={depth} desc={}",
                parent_task_id.as_deref().unwrap_or("(none)"),
                truncate_remote_preview(description, 60)
            )
        }
        RemoteTimelineEventDetail::SubtaskProgress {
            task_id,
            status,
            summary,
        } => {
            format!(
                "task_id={task_id} status={status} summary={}",
                truncate_remote_preview(summary, 60)
            )
        }
        RemoteTimelineEventDetail::SubtaskCompleted {
            task_id,
            status,
            summary,
            turns_used,
        } => {
            format!(
                "task_id={task_id} status={status} turns={:?} summary={}",
                turns_used,
                truncate_remote_preview(summary, 60)
            )
        }
        RemoteTimelineEventDetail::BatchProgress {
            total,
            completed,
            running,
        } => {
            format!("total={total} completed={completed} running={running}")
        }
        RemoteTimelineEventDetail::ContextUsage {
            estimated_tokens,
            max_input_tokens,
            threshold_tokens,
            ratio,
        } => {
            format!(
                "tokens={estimated_tokens}/{max_input_tokens} threshold={threshold_tokens} ratio={ratio:.2}"
            )
        }
        RemoteTimelineEventDetail::ContextOverflow {
            estimated_tokens,
            max_input_tokens,
            threshold_tokens,
            ratio,
        } => {
            format!(
                "OVERFLOW tokens={estimated_tokens}/{max_input_tokens} threshold={threshold_tokens} ratio={ratio:.2}"
            )
        }
        RemoteTimelineEventDetail::ContextCompacted {
            entries_removed,
            usage_ratio,
        } => {
            format!("removed={entries_removed} ratio={usage_ratio:.2}")
        }
    }
}

fn truncate_remote_preview(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = collapsed.chars().take(max_chars).collect::<String>();
    if collapsed.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

pub async fn run_remote(command: RemoteCommand) -> Result<()> {
    match command {
        RemoteCommand::Meta(args) => run_remote_meta(args).await,
        RemoteCommand::Auth { command } => run_remote_auth(command).await,
        RemoteCommand::Runners { command } => run_remote_runners(command).await,
        RemoteCommand::Artifacts { command } => run_remote_artifacts(command).await,
        RemoteCommand::Approvals { command } => run_remote_approvals(command).await,
        RemoteCommand::Events(args) => run_remote_events(args).await,
        RemoteCommand::Sessions { command } => run_remote_sessions(command).await,
    }
}

async fn run_remote_auth(command: RemoteAuthCommand) -> Result<()> {
    match command {
        RemoteAuthCommand::Devices(args) => run_remote_devices_list(args).await,
        RemoteAuthCommand::Bootstrap(args) => run_remote_auth_bootstrap(args).await,
        RemoteAuthCommand::PairOffer(args) => run_remote_auth_pair_offer(args).await,
        RemoteAuthCommand::PairAccept(args) => run_remote_auth_pair_accept(args).await,
    }
}

async fn run_remote_meta(args: RemoteMetaArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let meta: RemoteControlPlaneMeta = remote_get_json(&control_plane_url, "/v1/meta").await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&meta)?);
        return Ok(());
    }
    print_remote_meta(&meta);
    Ok(())
}

async fn run_remote_devices_list(args: RemoteDevicesListArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let devices: RemoteListResponse<RemoteTrustedDeviceRecord> =
        remote_get_json(&control_plane_url, "/v1/devices").await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
    } else if devices.items.is_empty() {
        println!("No trusted devices found.");
    } else {
        for device in &devices.items {
            print_remote_device_summary(device);
        }
    }
    Ok(())
}

async fn run_remote_auth_bootstrap(args: RemoteBootstrapArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemoteBootstrapClaimResponse = remote_post_json(
        &control_plane_url,
        "/v1/bootstrap/claim",
        &RemoteBootstrapClaimRequest {
            bootstrap_secret: args.bootstrap_secret,
            device_name: args.device_name,
            device_kind: args.device_kind.into(),
        },
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        print_remote_device_summary(&response.device);
        print_remote_access_token_help(&response.access_token);
    }
    Ok(())
}

async fn run_remote_auth_pair_offer(args: RemotePairingOfferArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemotePairingOfferCreateResponse = remote_post_json(
        &control_plane_url,
        "/v1/pairing/offers",
        &RemotePairingOfferCreateRequest {
            device_name: args.device_name,
            device_kind: args.device_kind.into(),
            expires_in_secs: args.expires_in_secs,
        },
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        print_remote_pairing_offer(&response);
    }
    Ok(())
}

async fn run_remote_auth_pair_accept(args: RemotePairingAcceptArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemotePairingAcceptResponse = remote_post_json(
        &control_plane_url,
        "/v1/pairing/accept",
        &RemotePairingAcceptRequest {
            offer_id: args.offer_id,
            pairing_secret: args.pairing_secret,
            device_name: args.device_name,
            device_kind: args.device_kind.map(Into::into),
        },
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        print_remote_device_summary(&response.device);
        print_remote_access_token_help(&response.access_token);
    }
    Ok(())
}

async fn run_remote_runners(command: RemoteRunnersCommand) -> Result<()> {
    match command {
        RemoteRunnersCommand::List(args) => run_remote_runners_list(args).await,
        RemoteRunnersCommand::Show(args) => run_remote_runners_show(args).await,
    }
}

async fn run_remote_artifacts(command: RemoteArtifactsCommand) -> Result<()> {
    match command {
        RemoteArtifactsCommand::List(args) => run_remote_artifacts_list(args).await,
        RemoteArtifactsCommand::Show(args) => run_remote_artifacts_show(args).await,
        RemoteArtifactsCommand::Download(args) => run_remote_artifacts_download(args).await,
        RemoteArtifactsCommand::Upload(args) => run_remote_artifacts_upload(args).await,
    }
}

async fn run_remote_sessions(command: RemoteSessionsCommand) -> Result<()> {
    match command {
        RemoteSessionsCommand::List(args) => run_remote_sessions_list(args).await,
        RemoteSessionsCommand::Show(args) => run_remote_sessions_show(args).await,
        RemoteSessionsCommand::Create(args) => run_remote_sessions_create(args).await,
        RemoteSessionsCommand::Follow(args) => run_remote_sessions_follow(args).await,
        RemoteSessionsCommand::State(args) => run_remote_sessions_state(args).await,
        RemoteSessionsCommand::Prompt(args) => run_remote_sessions_prompt(args).await,
        RemoteSessionsCommand::Interrupt(args) => run_remote_sessions_interrupt(args).await,
    }
}

async fn run_remote_runners_list(args: RemoteRunnersListArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemoteListResponse<RemoteRunnerSnapshot> =
        remote_get_json(&control_plane_url, "/v1/runners").await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if response.items.is_empty() {
        println!("No remote runners found.");
        return Ok(());
    }
    for runner in response.items {
        println!(
            "{}  {}  active={}  queued={}  {}",
            runner.registration.runner_id,
            runner.state.label(),
            runner.active_sessions,
            runner.queued_sessions,
            runner
                .registration
                .public_base_url
                .as_deref()
                .unwrap_or("(missing-public-base-url)")
        );
    }
    Ok(())
}

async fn run_remote_runners_show(args: RemoteRunnerShowArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let runner: RemoteRunnerSnapshot =
        remote_get_json(&control_plane_url, &remote_runner_path(&args.runner_id)).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&runner)?);
        return Ok(());
    }
    print_remote_runner_summary(&runner);
    Ok(())
}

async fn run_remote_artifacts_list(args: RemoteArtifactsListArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let path = remote_artifacts_path(args.session_id, args.runner_id.as_deref())?;
    let response: RemoteListResponse<RemoteArtifactRecord> =
        remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if response.items.is_empty() {
        println!("No remote artifacts found.");
        return Ok(());
    }
    for artifact in response.items {
        println!(
            "{}  {}  {}  {}  {}B  session={}  runner={}",
            artifact.artifact_id,
            artifact.created_at,
            artifact.file_name,
            artifact.media_type,
            artifact.size_bytes,
            artifact.session_id,
            artifact
                .runner_id
                .as_deref()
                .unwrap_or("(unassigned-runner)")
        );
    }
    Ok(())
}

async fn run_remote_artifacts_show(args: RemoteArtifactShowArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let path = format!("/v1/artifacts/{}", args.artifact_id);
    let artifact: RemoteArtifactRecord = remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&artifact)?);
        return Ok(());
    }
    print_remote_artifact_summary(&artifact);
    Ok(())
}

async fn run_remote_artifacts_download(args: RemoteArtifactDownloadArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let artifact_path = format!("/v1/artifacts/{}", args.artifact_id);
    let artifact: RemoteArtifactRecord =
        remote_get_json(&control_plane_url, &artifact_path).await?;
    let bytes = remote_get_bytes(
        &control_plane_url,
        &remote_artifact_download_path(args.artifact_id),
    )
    .await?;

    if args.stdout {
        let mut stdout = tokio::io::stdout();
        stdout.write_all(&bytes).await?;
        stdout.flush().await?;
        return Ok(());
    }

    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(&artifact.file_name));
    if output_path.exists() && !args.overwrite {
        return Err(anyhow!(
            "refusing to overwrite {}; pass --overwrite to replace it",
            output_path.display()
        ));
    }

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&output_path, &bytes).await?;
    println!(
        "Downloaded artifact {} to {} ({} bytes).",
        artifact.artifact_id,
        output_path.display(),
        bytes.len()
    );
    Ok(())
}

async fn run_remote_artifacts_upload(args: RemoteArtifactUploadArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let bytes = tokio::fs::read(&args.file)
        .await
        .map_err(|error| anyhow!("failed to read {}: {error}", args.file.display()))?;
    let request = RemoteArtifactCreateRequest {
        name: args
            .name
            .clone()
            .unwrap_or_else(|| default_artifact_name(&args.file)),
        file_name: Some(
            args.file_name
                .clone()
                .unwrap_or_else(|| default_artifact_file_name(&args.file)),
        ),
        media_type: args.media_type.clone(),
        content_base64: BASE64_STANDARD.encode(&bytes),
        metadata: parse_repeated_key_value_args("--meta", &args.metadata)?,
    };
    let path = remote_artifacts_path(Some(args.session_id), None)?;
    let artifact: RemoteArtifactRecord =
        remote_post_json(&control_plane_url, &path, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&artifact)?);
        return Ok(());
    }
    print_remote_artifact_summary(&artifact);
    Ok(())
}

async fn run_remote_sessions_list(args: RemoteSessionsListArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemoteListResponse<RemoteSessionRecord> = remote_get_json(
        &control_plane_url,
        &remote_sessions_path(
            args.runner_id.as_deref(),
            args.workspace_id.as_deref(),
            args.state.map(Into::into),
        ),
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if response.items.is_empty() {
        println!("No remote sessions found.");
        return Ok(());
    }
    for session in response.items {
        println!(
            "{}  {}  {}  {}  {}",
            session.session_id,
            session.updated_at,
            session.state.label(),
            session.workspace_id,
            session.owner_runner_id.as_deref().unwrap_or("(unassigned)")
        );
    }
    Ok(())
}

async fn run_remote_sessions_show(args: RemoteSessionShowArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let path = format!("/v1/sessions/{}", args.session_id);
    let session: RemoteSessionRecord = remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }
    print_remote_session_summary(&session);
    Ok(())
}

async fn run_remote_sessions_follow(args: RemoteSessionFollowArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let path = format!("/v1/sessions/{}", args.session_id);
    let session: RemoteSessionRecord = remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
    } else {
        print_remote_session_summary(&session);
    }

    let history_path =
        remote_events_path(Some(args.session_id), None, args.after, args.limit, None)?;
    let response: RemoteListResponse<RemoteTimelineEvent> =
        remote_get_json(&control_plane_url, &history_path).await?;
    if args.json {
        for event in &response.items {
            println!("{}", serde_json::to_string(event)?);
        }
    } else {
        print_remote_events(&response.items);
    }

    if args.stop_on_terminal && is_terminal_remote_session_state(session.state) {
        return Ok(());
    }

    let follow_after = merge_follow_sequence(
        merge_follow_sequence(
            response.items.last().map(|event| event.sequence),
            response.latest_sequence,
        ),
        args.after,
    )
    .or(Some(0));
    let json = args.json;
    let stop_on_terminal = args.stop_on_terminal;
    follow_remote_timeline_stream(
        &control_plane_url,
        follow_after,
        Duration::from_secs(args.reconnect_delay_secs.max(1)),
        move |after| remote_events_stream_path(Some(args.session_id), None, after, None),
        move |event| {
            let should_stop =
                stop_on_terminal && remote_event_reaches_terminal_session_state(&event);
            if json {
                println!("{}", serde_json::to_string(&event)?);
            } else {
                print_remote_events(std::slice::from_ref(&event));
            }
            if should_stop {
                Ok(RemoteFollowControl::Stop)
            } else {
                Ok(RemoteFollowControl::Continue)
            }
        },
    )
    .await
}

async fn run_remote_sessions_state(args: RemoteSessionStateArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let request = RemoteSessionStateUpdateRequest {
        state: args.state.into(),
        metadata: parse_repeated_key_value_args("--meta", &args.metadata)?,
    };
    let session: RemoteSessionRecord = remote_post_json(
        &control_plane_url,
        &remote_session_state_path(args.session_id),
        &request,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }
    print_remote_session_summary(&session);
    Ok(())
}

async fn run_remote_sessions_prompt(args: RemoteSessionPromptArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let prompt = args.prompt.join(" ").trim().to_owned();
    if prompt.is_empty() {
        return Err(anyhow!("prompt cannot be empty"));
    }
    let response: RemoteSessionCommandResponseValue = remote_post_json(
        &control_plane_url,
        &remote_session_commands_path(args.session_id),
        &claude_control_plane::RunnerSessionCommandRequest::SendPrompt { content: prompt },
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    println!(
        "Remote session {} accepted prompt: {}",
        response.session_id, response.message
    );
    Ok(())
}

async fn run_remote_sessions_interrupt(args: RemoteSessionInterruptArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let response: RemoteSessionCommandResponseValue = remote_post_json(
        &control_plane_url,
        &remote_session_commands_path(args.session_id),
        &claude_control_plane::RunnerSessionCommandRequest::Interrupt,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    println!(
        "Remote session {} accepted interrupt: {}",
        response.session_id, response.message
    );
    Ok(())
}

async fn run_remote_approvals(command: RemoteApprovalsCommand) -> Result<()> {
    match command {
        RemoteApprovalsCommand::List(args) => run_remote_approvals_list(args).await,
        RemoteApprovalsCommand::Create(args) => run_remote_approvals_create(args).await,
        RemoteApprovalsCommand::Show(args) => run_remote_approvals_show(args).await,
        RemoteApprovalsCommand::Respond(args) => run_remote_approvals_respond(args).await,
    }
}

async fn run_remote_approvals_list(args: RemoteApprovalsListArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    if args.follow {
        return run_remote_approvals_follow(control_plane_url, args).await;
    }
    let path = remote_approvals_path(args.session_id, args.runner_id.as_deref())?;
    let response: RemoteListResponse<RemoteApprovalRecord> =
        remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if response.items.is_empty() {
        println!("No remote approvals found.");
        return Ok(());
    }
    for approval in response.items {
        println!(
            "{}  {}  {}  {}  {}",
            approval.approval_id,
            approval.state.label(),
            approval.session_id,
            if approval.runner_id.is_empty() {
                "(unassigned-runner)"
            } else {
                approval.runner_id.as_str()
            },
            approval.title
        );
    }
    Ok(())
}

async fn run_remote_approvals_create(args: RemoteApprovalCreateArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let request = SharedApprovalCreateRequest {
        approval_id: None,
        title: args.title,
        description: args.description,
        metadata: parse_repeated_key_value_args("--meta", &args.metadata)?,
    };
    let approval: RemoteApprovalRecord = remote_post_json(
        &control_plane_url,
        &remote_approvals_path(Some(args.session_id), None)?,
        &request,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&approval)?);
        return Ok(());
    }
    print_remote_approval_summary(&approval);
    Ok(())
}

async fn run_remote_approvals_follow(
    control_plane_url: String,
    args: RemoteApprovalsListArgs,
) -> Result<()> {
    let mut follow_after = args.after;
    if args.after.is_none() {
        let path = remote_approvals_path(args.session_id, args.runner_id.as_deref())?;
        let response: RemoteListResponse<RemoteApprovalRecord> =
            remote_get_json(&control_plane_url, &path).await?;
        if args.json {
            for approval in &response.items {
                println!("{}", serde_json::to_string(approval)?);
            }
        } else if response.items.is_empty() {
            println!("No remote approvals found.");
        } else {
            for approval in &response.items {
                println!(
                    "{}  {}  {}  {}  {}",
                    approval.approval_id,
                    approval.state.label(),
                    approval.session_id,
                    if approval.runner_id.is_empty() {
                        "(unassigned-runner)"
                    } else {
                        approval.runner_id.as_str()
                    },
                    approval.title
                );
            }
        }
        follow_after = merge_follow_sequence(follow_after, response.latest_sequence);
    }

    let session_id = args.session_id;
    let runner_id = args.runner_id.clone();
    let json = args.json;
    follow_remote_timeline_stream(
        &control_plane_url,
        follow_after,
        Duration::from_secs(args.reconnect_delay_secs.max(1)),
        move |after| remote_approvals_stream_path(session_id, runner_id.as_deref(), after),
        move |event| {
            if json {
                println!("{}", serde_json::to_string(&event)?);
            } else {
                print_remote_events(&[event]);
            }
            Ok(RemoteFollowControl::Continue)
        },
    )
    .await
}

async fn run_remote_approvals_respond(args: RemoteApprovalRespondArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let path = format!("{}/decision", remote_approval_path(args.approval_id));
    let request = SharedApprovalDecisionRequest {
        decision: args.decision.into(),
        responder: args.responder,
        note: args.note,
    };
    let approval: RemoteApprovalRecord =
        remote_post_json(&control_plane_url, &path, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&approval)?);
        return Ok(());
    }
    print_remote_approval_summary(&approval);
    Ok(())
}

async fn run_remote_approvals_show(args: RemoteApprovalShowArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let approval: RemoteApprovalRecord =
        remote_get_json(&control_plane_url, &remote_approval_path(args.approval_id)).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&approval)?);
        return Ok(());
    }
    print_remote_approval_summary(&approval);
    Ok(())
}

async fn run_remote_events(args: RemoteEventsArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    if args.follow {
        return run_remote_events_follow(control_plane_url, args).await;
    }

    let path = remote_events_path(
        args.session_id,
        args.runner_id.as_deref(),
        args.after,
        args.limit,
        args.kind,
    )?;
    let response: RemoteListResponse<RemoteTimelineEvent> =
        remote_get_json(&control_plane_url, &path).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else {
        print_remote_events(&response.items);
    }
    Ok(())
}

async fn run_remote_events_follow(control_plane_url: String, args: RemoteEventsArgs) -> Result<()> {
    let history_path = remote_events_path(
        args.session_id,
        args.runner_id.as_deref(),
        args.after,
        args.limit,
        args.kind,
    )?;
    let response: RemoteListResponse<RemoteTimelineEvent> =
        remote_get_json(&control_plane_url, &history_path).await?;
    if args.json {
        for event in &response.items {
            println!("{}", serde_json::to_string(event)?);
        }
    } else {
        print_remote_events(&response.items);
    }

    let follow_after = merge_follow_sequence(
        merge_follow_sequence(
            response.items.last().map(|event| event.sequence),
            response.latest_sequence,
        ),
        args.after,
    )
    .or(Some(0));
    let session_id = args.session_id;
    let runner_id = args.runner_id.clone();
    let kind = args.kind;
    let json = args.json;
    follow_remote_timeline_stream(
        &control_plane_url,
        follow_after,
        Duration::from_secs(args.reconnect_delay_secs.max(1)),
        move |after| remote_events_stream_path(session_id, runner_id.as_deref(), after, kind),
        move |event| {
            if json {
                println!("{}", serde_json::to_string(&event)?);
            } else {
                print_remote_events(&[event]);
            }
            Ok(RemoteFollowControl::Continue)
        },
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteFollowControl {
    Continue,
    Stop,
}

pub(crate) fn merge_follow_sequence(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

pub(crate) fn is_terminal_remote_session_state(state: RemoteSessionState) -> bool {
    matches!(
        state,
        RemoteSessionState::Completed | RemoteSessionState::Failed | RemoteSessionState::Cancelled
    )
}

pub(crate) fn remote_event_reaches_terminal_session_state(event: &RemoteTimelineEvent) -> bool {
    match &event.detail {
        RemoteTimelineEventDetail::SessionCreated { state, .. }
        | RemoteTimelineEventDetail::SessionStateChanged { state, .. } => {
            is_terminal_remote_session_state(*state)
        }
        _ => false,
    }
}

pub(crate) async fn follow_remote_timeline_stream<FPath, FEvent>(
    control_plane_url: &str,
    mut last_sequence: Option<u64>,
    reconnect_delay: Duration,
    mut path_builder: FPath,
    mut event_handler: FEvent,
) -> Result<()>
where
    FPath: FnMut(Option<u64>) -> Result<String>,
    FEvent: FnMut(RemoteTimelineEvent) -> Result<RemoteFollowControl>,
{
    loop {
        let ws_path = path_builder(last_sequence)?;
        let ws_request = build_remote_ws_request(control_plane_url, &ws_path)?;
        let (mut socket, _) = match connect_async(ws_request).await {
            Ok(connection) => connection,
            Err(error) => {
                if !is_retryable_remote_follow_connect_error(&error) {
                    return Err(anyhow!(format_remote_follow_connect_error(&error)));
                }
                warn!("{}", format_remote_follow_connect_error(&error));
                if wait_for_remote_follow_retry(reconnect_delay).await? {
                    return Ok(());
                }
                continue;
            }
        };

        let mut should_stop = false;
        loop {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result?;
                    return Ok(());
                }
                message = socket.next() => {
                    let Some(message) = message else {
                        warn!("remote follow stream ended; reconnecting");
                        break;
                    };
                    match message {
                        Ok(TungsteniteMessage::Text(text)) => {
                            let event: RemoteTimelineEvent = serde_json::from_str(&text)?;
                            if last_sequence.is_none_or(|sequence| event.sequence > sequence) {
                                last_sequence = Some(event.sequence);
                                if matches!(event_handler(event)?, RemoteFollowControl::Stop) {
                                    should_stop = true;
                                    break;
                                }
                            }
                        }
                        Ok(TungsteniteMessage::Binary(bytes)) => {
                            let event: RemoteTimelineEvent = serde_json::from_slice(&bytes)?;
                            if last_sequence.is_none_or(|sequence| event.sequence > sequence) {
                                last_sequence = Some(event.sequence);
                                if matches!(event_handler(event)?, RemoteFollowControl::Stop) {
                                    should_stop = true;
                                    break;
                                }
                            }
                        }
                        Ok(TungsteniteMessage::Close(frame)) => {
                            if let Some(frame) = frame {
                                warn!(
                                    "remote follow stream closed by server: code={} reason={}",
                                    frame.code,
                                    frame.reason
                                );
                            } else {
                                warn!("remote follow stream closed by server");
                            }
                            break;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            warn!("remote follow stream error: {error}");
                            break;
                        }
                    }
                }
            }
        }

        if should_stop {
            return Ok(());
        }

        if wait_for_remote_follow_retry(reconnect_delay).await? {
            return Ok(());
        }
    }
}

async fn wait_for_remote_follow_retry(duration: Duration) -> Result<bool> {
    tokio::select! {
        () = tokio::time::sleep(duration) => Ok(false),
        result = tokio::signal::ctrl_c() => {
            result?;
            Ok(true)
        }
    }
}

async fn run_remote_sessions_create(args: RemoteSessionCreateArgs) -> Result<()> {
    let control_plane_url = require_control_plane_url(&args.target)?;
    let request = RemoteCreateSessionRequest {
        session_id: None,
        workspace_id: args.workspace_id,
        preferred_runner_id: args.preferred_runner_id,
        metadata: parse_repeated_key_value_args("--meta", &args.metadata)?,
    };
    let session: RemoteSessionRecord =
        remote_post_json(&control_plane_url, "/v1/sessions", &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }
    print_remote_session_summary(&session);
    Ok(())
}
