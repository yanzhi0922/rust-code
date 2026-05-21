//! Runner API server for remote agent execution.
//!
//! Provides an axum-based HTTP API for session management, approval workflows,
//! and health reporting. Runners register with a control plane, accept sessions,
//! and relay approval requests back to the control plane.

use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, State, WebSocketUpgrade},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use claude_config::AppPaths;
use futures::SinkExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;

const DEFAULT_RUNNER_BIND: &str = "127.0.0.1:8788";
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 15;
const DEFAULT_MAX_PARALLEL_SESSIONS: u16 = 4;
const RUNNER_STREAM_BUFFER: usize = 256;
const PHASE: &str = "phase5-remote-stable";

/// Overrides applied to runner configuration from CLI flags or environment variables.
#[derive(Debug, Clone, Default)]
pub struct RunnerConfigOverrides {
    /// Runner identifier override.
    pub runner_id: Option<String>,
    /// Control plane URL override.
    pub control_plane_url: Option<String>,
    /// Bind address override.
    pub bind: Option<SocketAddr>,
    /// Public base URL override.
    pub public_base_url: Option<String>,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: Option<u64>,
    /// Maximum number of parallel sessions.
    pub max_parallel_sessions: Option<u16>,
    /// Workspace list override.
    pub workspaces: Option<Vec<RunnerWorkspace>>,
    /// Label overrides.
    pub labels: Option<BTreeMap<String, String>>,
    /// Bearer token for protecting the runner API.
    pub auth_token: Option<String>,
    /// Bearer token used when this runner calls the control plane.
    pub control_plane_auth_token: Option<String>,
}

/// Full runner configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// Unique runner identifier.
    pub runner_id: String,
    /// Control plane URL for registration and heartbeats.
    pub control_plane_url: Option<String>,
    /// Local bind address for the runner API server.
    pub bind: SocketAddr,
    /// Publicly reachable URL for the runner API.
    pub public_base_url: Option<String>,
    /// Application paths for the runner profile.
    pub profile_dir: AppPaths,
    /// Workspaces this runner can serve.
    pub workspaces: Vec<RunnerWorkspace>,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// Maximum number of concurrent sessions.
    pub max_parallel_sessions: u16,
    /// Arbitrary key-value labels for scheduling.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Bearer token for protecting the runner API.
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,
    /// Bearer token used for outbound control-plane requests.
    #[serde(default, skip_serializing)]
    pub control_plane_auth_token: Option<String>,
    /// Runner capability flags.
    pub capabilities: RunnerCapabilities,
}

/// A workspace that a runner can serve.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerWorkspace {
    /// Unique workspace identifier.
    pub workspace_id: String,
    /// Root directory of the workspace on disk.
    pub root_dir: PathBuf,
    /// Whether the runner has write access to this workspace.
    pub writable: bool,
}

/// Capability flags advertised by a runner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerCapabilities {
    /// Whether the runner supports interactive approval prompts.
    pub interactive_approvals: bool,
    /// Whether the runner can run sessions in the background.
    pub background_sessions: bool,
    /// Whether the runner can upload artifacts.
    pub artifact_uploads: bool,
    /// Maximum number of parallel sessions.
    pub max_parallel_sessions: u16,
}

/// Platform information for a runner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerPlatform {
    /// Operating system name.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// OS family.
    pub family: String,
}

/// Current state of a runner.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RunnerState {
    /// Runner is starting up.
    Starting,
    /// Runner is idle and accepting sessions.
    #[default]
    Idle,
    /// Runner has active sessions.
    Busy,
    /// Runner is draining and not accepting new sessions.
    Draining,
    /// Runner is unhealthy.
    Unhealthy,
    /// Runner is offline.
    Offline,
}

/// Lifecycle state of a runner-managed session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Session is pending assignment.
    #[default]
    Pending,
    /// Session is starting up.
    Starting,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerRegistrationRequest {
    pub runner_id: String,
    pub control_plane_url: Option<String>,
    pub public_base_url: Option<String>,
    pub workspaces: Vec<RunnerWorkspace>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,
    pub capabilities: RunnerCapabilities,
    pub platform: RunnerPlatform,
}

#[derive(Debug, Serialize)]
struct RunnerRegistrationWire<'a> {
    pub runner_id: &'a str,
    pub control_plane_url: &'a Option<String>,
    pub public_base_url: &'a Option<String>,
    pub workspaces: &'a [RunnerWorkspace],
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: &'a Option<String>,
    pub capabilities: &'a RunnerCapabilities,
    pub platform: &'a RunnerPlatform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerHeartbeat {
    pub runner_id: String,
    pub state: RunnerState,
    pub active_sessions: usize,
    pub queued_sessions: usize,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSnapshot {
    pub registration: RunnerRegistrationRequest,
    pub state: RunnerState,
    pub active_sessions: usize,
    pub queued_sessions: usize,
    pub registered_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerRegistrationLease {
    pub runner_id: String,
    pub registered_at: DateTime<Utc>,
    pub lease_ttl_secs: u64,
    pub snapshot: RunnerSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerStatus {
    pub ok: bool,
    pub runner_id: String,
    pub control_plane_url: Option<String>,
    pub bind: String,
    pub public_base_url: Option<String>,
    pub profile_dir: String,
    pub workspace_count: usize,
    pub workspaces: Vec<RunnerWorkspace>,
    pub heartbeat_interval_secs: u64,
    pub max_parallel_sessions: u16,
    pub auth_required: bool,
    pub issues: Vec<String>,
    pub phase: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerMeta {
    pub service: String,
    pub version: String,
    pub phase: String,
    pub snapshot: RunnerSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerHealth {
    pub ok: bool,
    pub runner_id: String,
    pub state: RunnerState,
    pub active_sessions: usize,
    pub queued_sessions: usize,
    pub workspace_count: usize,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSessionRecord {
    pub session_id: Uuid,
    pub runner_id: String,
    pub workspace_id: String,
    pub state: SessionState,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSessionCreateRequest {
    pub session_id: Option<Uuid>,
    pub workspace_id: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSessionStateUpdateRequest {
    pub state: SessionState,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunnerSessionCommandRequest {
    SendPrompt { content: String },
    Interrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerSessionCommandResponse {
    pub session_id: Uuid,
    pub accepted: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    #[default]
    Pending,
    Approved,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Denied,
    Cancelled,
}

impl From<ApprovalDecision> for ApprovalState {
    fn from(value: ApprovalDecision) -> Self {
        match value {
            ApprovalDecision::Approved => ApprovalState::Approved,
            ApprovalDecision::Denied => ApprovalState::Denied,
            ApprovalDecision::Cancelled => ApprovalState::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestRecord {
    pub approval_id: Uuid,
    pub session_id: Uuid,
    pub runner_id: String,
    pub state: ApprovalState,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub responded_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub responder: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Uuid>,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub decision: ApprovalDecision,
    #[serde(default)]
    pub responder: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_sequence: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum RunnerApiEvent {
    SessionCreated(RunnerSessionRecord),
    ApprovalResolved(ApprovalRequestRecord),
    SessionCommand {
        session_id: Uuid,
        command: RunnerSessionCommandRequest,
    },
}

/// Capacity for runner event channels.
///
/// Bounded to keep memory predictable when consumers stall. The runner emits
/// events from synchronous code paths (HTTP handlers), so on overflow we drop
/// the event and log — better than growing the queue without limit.
pub const RUNNER_EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct RunnerApi {
    meta: RunnerMeta,
    sessions: Arc<RwLock<BTreeMap<Uuid, RunnerSessionRecord>>>,
    approvals: Arc<RwLock<BTreeMap<Uuid, ApprovalRequestRecord>>>,
    event_tx: Option<mpsc::Sender<RunnerApiEvent>>,
    stream_tx: broadcast::Sender<(Uuid, String)>,
}

impl RunnerApi {
    pub fn new(
        config: RunnerConfig,
        service: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let snapshot = config.snapshot();
        let (stream_tx, _) = broadcast::channel(RUNNER_STREAM_BUFFER);
        Self {
            meta: RunnerMeta {
                service: service.into(),
                version: version.into(),
                phase: PHASE.to_owned(),
                snapshot,
            },
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
            approvals: Arc::new(RwLock::new(BTreeMap::new())),
            event_tx: None,
            stream_tx,
        }
    }

    #[must_use]
    pub fn with_event_channel(mut self, event_tx: mpsc::Sender<RunnerApiEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    #[must_use]
    pub fn meta(&self) -> &RunnerMeta {
        &self.meta
    }

    pub async fn list_sessions(&self) -> Vec<RunnerSessionRecord> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    pub async fn list_approvals(&self) -> Vec<ApprovalRequestRecord> {
        let approvals = self.approvals.read().await;
        approvals.values().cloned().collect()
    }

    pub async fn get_session(&self, session_id: Uuid) -> Option<RunnerSessionRecord> {
        let sessions = self.sessions.read().await;
        sessions.get(&session_id).cloned()
    }

    pub async fn create_session_direct(
        &self,
        request: RunnerSessionCreateRequest,
    ) -> Result<RunnerSessionRecord> {
        let workspace = self
            .meta
            .snapshot
            .registration
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == request.workspace_id)
            .ok_or_else(|| {
                anyhow!(
                    "workspace `{}` is not owned by this runner",
                    request.workspace_id
                )
            })?;

        let mut sessions = self.sessions.write().await;
        let session_id = request.session_id.unwrap_or_else(Uuid::new_v4);
        if let Some(existing) = sessions.get(&session_id) {
            return Ok(existing.clone());
        }

        let (active_sessions, queued_sessions) = session_counts(&sessions);
        let max_parallel_sessions = usize::from(
            self.meta
                .snapshot
                .registration
                .capabilities
                .max_parallel_sessions,
        );
        if active_sessions + queued_sessions >= max_parallel_sessions {
            return Err(anyhow!(
                "runner `{}` is at session capacity ({max_parallel_sessions})",
                self.meta.snapshot.registration.runner_id
            ));
        }

        let now = Utc::now();
        let record = RunnerSessionRecord {
            session_id,
            runner_id: self.meta.snapshot.registration.runner_id.clone(),
            workspace_id: workspace.workspace_id.clone(),
            state: SessionState::Pending,
            metadata: request.metadata,
            created_at: now,
            updated_at: now,
        };
        sessions.insert(record.session_id, record.clone());
        self.emit_event(RunnerApiEvent::SessionCreated(record.clone()));
        Ok(record)
    }

    pub async fn apply_session_state_update_direct(
        &self,
        session_id: Uuid,
        request: RunnerSessionStateUpdateRequest,
    ) -> Result<RunnerSessionRecord> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow!("session `{session_id}` was not found"))?;
        session.state = request.state;
        session.metadata.extend(request.metadata);
        session.updated_at = Utc::now();
        Ok(session.clone())
    }

    pub async fn post_session_command_direct(
        &self,
        session_id: Uuid,
        command: RunnerSessionCommandRequest,
    ) -> Result<RunnerSessionCommandResponse> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(&session_id) {
            return Err(anyhow!("session `{session_id}` was not found"));
        }
        drop(sessions);

        let message = match &command {
            RunnerSessionCommandRequest::SendPrompt { .. } => "prompt forwarded",
            RunnerSessionCommandRequest::Interrupt => "interrupt forwarded",
        };
        self.emit_event(RunnerApiEvent::SessionCommand {
            session_id,
            command,
        });
        Ok(RunnerSessionCommandResponse {
            session_id,
            accepted: true,
            message: message.to_owned(),
        })
    }

    pub async fn create_approval_direct(
        &self,
        session_id: Uuid,
        request: ApprovalCreateRequest,
    ) -> Result<ApprovalRequestRecord> {
        if let Some(approval_id) = request.approval_id
            && let Some(existing) = self.approvals.read().await.get(&approval_id).cloned()
        {
            return Ok(existing);
        }

        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow!("session `{session_id}` was not found"))?;
        let now = Utc::now();
        session.state = SessionState::WaitingApproval;
        session.updated_at = now;

        let approval = ApprovalRequestRecord {
            approval_id: request.approval_id.unwrap_or_else(Uuid::new_v4),
            session_id,
            runner_id: self.meta.snapshot.registration.runner_id.clone(),
            state: ApprovalState::Pending,
            title: request.title,
            description: request.description,
            metadata: request.metadata,
            created_at: now,
            updated_at: now,
            responded_at: None,
            responder: None,
            note: None,
        };
        drop(sessions);

        let mut approvals = self.approvals.write().await;
        approvals.insert(approval.approval_id, approval.clone());
        Ok(approval)
    }

    pub async fn apply_approval_decision_direct(
        &self,
        approval_id: Uuid,
        request: ApprovalDecisionRequest,
    ) -> Result<ApprovalRequestRecord> {
        let decision = request.decision;
        let mut approvals = self.approvals.write().await;
        let approval = approvals
            .get_mut(&approval_id)
            .ok_or_else(|| anyhow!("approval `{approval_id}` was not found"))?;
        if !matches!(approval.state, ApprovalState::Pending) {
            return Ok(approval.clone());
        }

        let now = Utc::now();
        approval.state = decision.into();
        approval.updated_at = now;
        approval.responded_at = Some(now);
        approval.responder = request.responder;
        approval.note = request.note;
        let updated = approval.clone();
        let has_pending_approvals = approvals.values().any(|candidate| {
            candidate.session_id == updated.session_id
                && candidate.approval_id != updated.approval_id
                && matches!(candidate.state, ApprovalState::Pending)
        });
        drop(approvals);

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&updated.session_id) {
            session.state = session_state_after_approval(decision, has_pending_approvals);
            session.updated_at = now;
        }

        self.emit_event(RunnerApiEvent::ApprovalResolved(updated.clone()));
        Ok(updated)
    }

    pub async fn heartbeat(&self) -> RunnerHeartbeat {
        let sessions = self.sessions.read().await;
        let (active_sessions, queued_sessions) = session_counts(&sessions);
        RunnerHeartbeat {
            runner_id: self.meta.snapshot.registration.runner_id.clone(),
            state: if active_sessions > 0 {
                RunnerState::Busy
            } else {
                RunnerState::Idle
            },
            active_sessions,
            queued_sessions,
            timestamp: Utc::now(),
        }
    }

    pub fn router(self) -> Router {
        let protected = Router::new()
            .route("/v1/meta", get(get_meta))
            .route("/v1/approvals", get(list_approvals))
            .route("/v1/approvals/{approval_id}", get(get_approval))
            .route(
                "/v1/approvals/{approval_id}/decision",
                axum::routing::post(apply_approval_decision),
            )
            .route("/v1/sessions", get(list_sessions).post(create_session))
            .route("/v1/sessions/{session_id}", get(get_session))
            .route(
                "/v1/sessions/{session_id}/state",
                axum::routing::post(update_session_state),
            )
            .route(
                "/v1/sessions/{session_id}/commands",
                axum::routing::post(post_session_command),
            )
            .route(
                "/v1/sessions/{session_id}/events/stream",
                get(subscribe_session_events),
            )
            .route(
                "/v1/sessions/{session_id}/approvals",
                get(list_session_approvals).post(create_approval),
            )
            .route_layer(middleware::from_fn_with_state(
                self.clone(),
                require_runner_auth,
            ));

        Router::new()
            .route("/healthz", get(get_health))
            .merge(protected)
            .with_state(self)
    }
}

impl RunnerApi {
    fn emit_event(&self, event: RunnerApiEvent) {
        if let Some(event_tx) = &self.event_tx {
            // Synchronous emission path; use try_send and log on saturation
            // rather than blocking inside the HTTP handler.
            match event_tx.try_send(event) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        capacity = RUNNER_EVENT_CHANNEL_CAPACITY,
                        "runner event channel saturated; dropping event"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!("runner event channel closed; dropping event");
                }
            }
        }
    }

    /// Broadcast a raw JSON event line to direct-connect WebSocket subscribers.
    pub fn publish_stream_event(&self, session_id: Uuid, json_line: &str) {
        let _ = self.stream_tx.send((session_id, json_line.to_owned()));
    }

    /// Subscribe to broadcast events for direct-connect streaming.
    pub fn subscribe_stream_events(&self) -> broadcast::Receiver<(Uuid, String)> {
        self.stream_tx.subscribe()
    }

    /// Process a [`UnifiedAgentEvent`] natively — converts to
    /// [`RuntimeEventDetail`] in memory (zero serialization), then broadcasts
    /// the JSON to direct-connect WebSocket subscribers.  Returns `true` if the
    /// event produced a timeline-representable detail.
    pub fn process_agent_event(
        &self,
        session_id: Uuid,
        event: &rc_agent_protocol::UnifiedAgentEvent,
    ) -> bool {
        let Some(detail) = rc_agent_protocol::unified_event_to_runtime_detail(event) else {
            return false;
        };
        // Broadcast to direct-connect WebSocket subscribers as JSON.
        // The caller (e.g. remote-code-runner) is responsible for also
        // persisting the detail to the control plane via post_runtime_event.
        let json_line = serde_json::to_string(&detail).unwrap_or_default();
        self.publish_stream_event(session_id, &json_line);
        true
    }

    /// Convert a [`UnifiedAgentEvent`] into a [`RuntimeEventDetail`] for
    /// control-plane persistence.  Returns `None` for lifecycle events.
    ///
    /// Returns the detail as a serializable JSON value so callers can forward
    /// it to the control plane without depending on the engine-events crate.
    pub fn agent_event_to_runtime_detail(
        &self,
        event: &rc_agent_protocol::UnifiedAgentEvent,
    ) -> Option<serde_json::Value> {
        rc_agent_protocol::unified_event_to_runtime_detail(event)
            .map(|detail| serde_json::to_value(&detail).unwrap_or_default())
    }
}

impl RunnerConfig {
    #[must_use]
    pub fn snapshot(&self) -> RunnerSnapshot {
        let now = Utc::now();
        RunnerSnapshot {
            registration: self.registration_request(),
            state: RunnerState::Idle,
            active_sessions: 0,
            queued_sessions: 0,
            registered_at: now,
            last_seen_at: now,
        }
    }

    #[must_use]
    pub fn registration_request(&self) -> RunnerRegistrationRequest {
        RunnerRegistrationRequest {
            runner_id: self.runner_id.clone(),
            control_plane_url: self.control_plane_url.clone(),
            public_base_url: self.public_base_url.clone(),
            workspaces: self.workspaces.clone(),
            labels: self.labels.clone(),
            auth_token: self.auth_token.clone(),
            capabilities: self.capabilities.clone(),
            platform: RunnerPlatform::detect(),
        }
    }
}

impl RunnerPlatform {
    #[must_use]
    pub fn detect() -> Self {
        Self {
            os: env::consts::OS.to_owned(),
            arch: env::consts::ARCH.to_owned(),
            family: env::consts::FAMILY.to_owned(),
        }
    }
}

pub fn load_runner_config(
    profile_dir_override: Option<PathBuf>,
    overrides: RunnerConfigOverrides,
) -> Result<RunnerConfig> {
    let paths = AppPaths::discover(profile_dir_override)?;
    paths.ensure_exists()?;

    let runner_id = overrides
        .runner_id
        .or_else(|| read_env("REMOTE_CODE_RUNNER_ID"))
        .unwrap_or_else(|| "local-runner".to_owned());
    let control_plane_url = overrides
        .control_plane_url
        .or_else(|| read_env("REMOTE_CODE_CONTROL_PLANE_URL"));
    let bind = match overrides.bind {
        Some(bind) => bind,
        None => parse_socket_addr(
            &read_env("REMOTE_CODE_RUNNER_BIND").unwrap_or_else(|| DEFAULT_RUNNER_BIND.to_owned()),
        )?,
    };
    let public_base_url = overrides
        .public_base_url
        .or_else(|| read_env("REMOTE_CODE_RUNNER_PUBLIC_BASE_URL"));
    let auth_token = overrides
        .auth_token
        .or_else(|| read_env("REMOTE_CODE_RUNNER_AUTH_TOKEN"));
    let control_plane_auth_token = overrides
        .control_plane_auth_token
        .or_else(read_control_plane_auth_token_env);
    let heartbeat_interval_secs = overrides
        .heartbeat_interval_secs
        .or_else(|| parse_env_number("REMOTE_CODE_RUNNER_HEARTBEAT_SECS"))
        .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_SECS)
        .max(1);
    let max_parallel_sessions = overrides
        .max_parallel_sessions
        .or_else(|| parse_env_number("REMOTE_CODE_RUNNER_MAX_PARALLEL_SESSIONS"))
        .unwrap_or(DEFAULT_MAX_PARALLEL_SESSIONS)
        .max(1);
    let labels = overrides
        .labels
        .or_else(|| read_env("REMOTE_CODE_RUNNER_LABELS").map(|raw| parse_key_value_map(&raw)))
        .unwrap_or_default();
    let workspaces = match overrides.workspaces {
        Some(workspaces) => workspaces,
        None => {
            if let Some(raw) = read_env("REMOTE_CODE_RUNNER_WORKSPACES") {
                parse_runner_workspaces(&raw)?
            } else {
                vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: env::current_dir()
                        .context("failed to discover the current working directory")?,
                    writable: true,
                }]
            }
        }
    };

    Ok(RunnerConfig {
        runner_id,
        control_plane_url,
        bind,
        public_base_url,
        profile_dir: paths,
        workspaces,
        heartbeat_interval_secs,
        max_parallel_sessions,
        labels,
        auth_token,
        control_plane_auth_token,
        capabilities: RunnerCapabilities {
            interactive_approvals: true,
            background_sessions: true,
            artifact_uploads: true,
            max_parallel_sessions,
        },
    })
}

pub fn describe_status(config: &RunnerConfig) -> Result<RunnerStatus> {
    let mut issues = validate_runner_config(config);
    if config.control_plane_url.is_none() {
        issues.push("REMOTE_CODE_CONTROL_PLANE_URL is not configured.".to_owned());
    }
    if config.workspaces.is_empty() {
        issues.push("No runner workspaces are configured.".to_owned());
    }

    Ok(RunnerStatus {
        ok: issues.is_empty(),
        runner_id: config.runner_id.clone(),
        control_plane_url: config.control_plane_url.clone(),
        bind: config.bind.to_string(),
        public_base_url: config.public_base_url.clone(),
        profile_dir: config.profile_dir.profile_dir.display().to_string(),
        workspace_count: config.workspaces.len(),
        workspaces: config.workspaces.clone(),
        heartbeat_interval_secs: config.heartbeat_interval_secs,
        max_parallel_sessions: config.max_parallel_sessions,
        auth_required: config.auth_token.is_some(),
        issues,
        phase: PHASE,
    })
}

pub fn validate_runner_config(config: &RunnerConfig) -> Vec<String> {
    let mut issues = Vec::new();
    let auth_configured = config.auth_token.is_some();
    let control_plane_auth_configured = config.control_plane_auth_token.is_some();
    let public_url = config.public_base_url.as_deref();
    let remote_public_url = public_url.filter(|url| !is_local_runner_url(url));
    let remote_control_plane_url = config
        .control_plane_url
        .as_deref()
        .filter(|url| !is_local_runner_url(url));

    if !config.bind.ip().is_loopback() && !auth_configured {
        issues.push("non-loopback runner binds require REMOTE_CODE_RUNNER_AUTH_TOKEN".to_owned());
    }

    if remote_public_url.is_some() && !auth_configured {
        issues.push(
            "remote runner public_base_url requires REMOTE_CODE_RUNNER_AUTH_TOKEN".to_owned(),
        );
    }

    if let Some(url) = remote_public_url
        && !url.starts_with("https://")
    {
        issues.push("remote runner public_base_url must use https".to_owned());
    }

    if remote_control_plane_url.is_some() && !control_plane_auth_configured {
        issues.push(
            "remote control-plane URL requires REMOTE_CODE_CONTROL_PLANE_AUTH_TOKEN or desktop credentials"
                .to_owned(),
        );
    }

    issues
}

fn is_local_runner_url(raw: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(raw) else {
        return false;
    };

    match parsed.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

pub fn parse_runner_workspaces(raw: &str) -> Result<Vec<RunnerWorkspace>> {
    let mut workspaces = Vec::new();
    for entry in raw
        .split([';', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (workspace_id, remainder) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid workspace entry `{entry}`; expected id=path|rw"))?;
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return Err(anyhow!(
                "invalid workspace entry `{entry}`; workspace id is empty"
            ));
        }

        let mut parts = remainder
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty());
        let root_dir = parts
            .next()
            .ok_or_else(|| anyhow!("invalid workspace entry `{entry}`; path is missing"))?;
        let mut writable = true;
        for part in parts {
            if part.eq_ignore_ascii_case("ro") || part.eq_ignore_ascii_case("read-only") {
                writable = false;
            } else if part.eq_ignore_ascii_case("rw") || part.eq_ignore_ascii_case("read-write") {
                writable = true;
            }
        }

        workspaces.push(RunnerWorkspace {
            workspace_id: workspace_id.to_owned(),
            root_dir: PathBuf::from(root_dir),
            writable,
        });
    }

    if workspaces.is_empty() {
        return Err(anyhow!("at least one runner workspace must be configured"));
    }
    Ok(workspaces)
}

pub fn parse_key_value_map(raw: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for entry in raw
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if let Some((key, value)) = entry.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && !value.is_empty() {
                values.insert(key.to_owned(), value.to_owned());
            }
        }
    }
    values
}

pub async fn register_with_control_plane(
    client: &Client,
    control_plane_url: &str,
    registration: &RunnerRegistrationRequest,
    control_plane_auth_token: Option<&str>,
) -> Result<RunnerRegistrationLease> {
    let payload = RunnerRegistrationWire {
        runner_id: &registration.runner_id,
        control_plane_url: &registration.control_plane_url,
        public_base_url: &registration.public_base_url,
        workspaces: &registration.workspaces,
        labels: &registration.labels,
        auth_token: &registration.auth_token,
        capabilities: &registration.capabilities,
        platform: &registration.platform,
    };
    let response = authorize_control_plane_request(
        client.post(control_plane_endpoint(
            control_plane_url,
            "/v1/runners/register",
        )?),
        control_plane_auth_token,
    )
    .json(&payload)
    .send()
    .await
    .context("runner registration request failed")?
    .error_for_status()
    .context("runner registration was rejected by the control plane")?;
    response
        .json::<RunnerRegistrationLease>()
        .await
        .context("failed to decode runner registration response")
}

pub async fn send_heartbeat(
    client: &Client,
    control_plane_url: &str,
    heartbeat: &RunnerHeartbeat,
    auth_token: Option<&str>,
) -> Result<RunnerSnapshot> {
    let path = format!(
        "/v1/runners/{}/heartbeat",
        encode_path_segment(&heartbeat.runner_id)
    );
    let response = authorize_control_plane_request(
        client.post(control_plane_endpoint(control_plane_url, &path)?),
        auth_token,
    )
    .json(heartbeat)
    .send()
    .await
    .context("runner heartbeat request failed")?
    .error_for_status()
    .context("runner heartbeat was rejected by the control plane")?;
    response
        .json::<RunnerSnapshot>()
        .await
        .context("failed to decode runner heartbeat response")
}

fn control_plane_endpoint(base_url: &str, path: &str) -> Result<String> {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err(anyhow!("control plane URL is empty"));
    }
    Ok(format!("{base_url}{path}"))
}

fn authorize_control_plane_request(
    builder: reqwest::RequestBuilder,
    explicit_token: Option<&str>,
) -> reqwest::RequestBuilder {
    let token = explicit_token
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(read_control_plane_auth_token_env);
    if let Some(token) = token {
        builder.bearer_auth(token)
    } else {
        builder
    }
}

fn read_control_plane_auth_token_env() -> Option<String> {
    env::var("REMOTE_CODE_CONTROL_PLANE_AUTH_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn encode_path_segment(raw: &str) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn session_counts(sessions: &BTreeMap<Uuid, RunnerSessionRecord>) -> (usize, usize) {
    let active_sessions = sessions
        .values()
        .filter(|session| {
            matches!(
                session.state,
                SessionState::Starting | SessionState::Running | SessionState::WaitingApproval
            )
        })
        .count();
    let queued_sessions = sessions
        .values()
        .filter(|session| matches!(session.state, SessionState::Pending))
        .count();
    (active_sessions, queued_sessions)
}

fn session_state_after_approval(
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

async fn get_health(State(api): State<RunnerApi>) -> Json<RunnerHealth> {
    let sessions = api.sessions.read().await;
    let (active_sessions, queued_sessions) = session_counts(&sessions);

    Json(RunnerHealth {
        ok: true,
        runner_id: api.meta.snapshot.registration.runner_id.clone(),
        state: if active_sessions > 0 {
            RunnerState::Busy
        } else {
            RunnerState::Idle
        },
        active_sessions,
        queued_sessions,
        workspace_count: api.meta.snapshot.registration.workspaces.len(),
        auth_required: api.meta.snapshot.registration.auth_token.is_some(),
    })
}

async fn require_runner_auth(
    State(api): State<RunnerApi>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = api.meta.snapshot.registration.auth_token.as_deref() else {
        return next.run(request).await;
    };

    // Check Authorization header first.
    let header_ok = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .is_some_and(|provided| constant_time_token_eq(provided, expected));

    if header_ok {
        return next.run(request).await;
    }

    // Query-token WebSocket auth is a temporary legacy path. Native clients use
    // Authorization headers; browser clients should use control-plane tickets.
    let is_stream_path = request.uri().path().ends_with("/stream");
    let is_ws_upgrade = request.headers().get("upgrade").is_some_and(|v| {
        v.to_str()
            .is_ok_and(|v| v.eq_ignore_ascii_case("websocket"))
    });
    let is_get = request.method() == axum::http::Method::GET;

    if is_stream_path
        && (is_ws_upgrade || is_get)
        && runner_query_access_tokens_enabled()
        && let Some(token) = extract_query_auth_token(request.uri().query())
        && constant_time_token_eq(&token, expected)
    {
        strip_auth_from_request_uri(&mut request);
        return next.run(request).await;
    }

    ApiError::unauthorized("missing or invalid runner bearer token".to_owned()).into_response()
}

fn extract_query_auth_token(query: Option<&str>) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?.trim();
        let value = parts.next().unwrap_or_default().trim();
        if matches!(key, "token" | "access_token") && !value.is_empty() {
            return Some(percent_decode_query_value(value));
        }
    }
    None
}

fn percent_decode_query_value(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = bytes[index + 1] as char;
                let low = bytes[index + 2] as char;
                if let (Some(high), Some(low)) = (high.to_digit(16), low.to_digit(16)) {
                    decoded.push(((high << 4) | low) as u8);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn strip_auth_from_request_uri(request: &mut axum::extract::Request) {
    let uri = request.uri().clone();
    let Some(query) = uri.query() else {
        return;
    };
    let cleaned: String = query
        .split('&')
        .filter(|pair| {
            let key = pair.split('=').next().unwrap_or("").trim();
            !matches!(key, "token" | "access_token")
        })
        .collect::<Vec<_>>()
        .join("&");
    let new_uri = if cleaned.is_empty() {
        uri.path().to_owned()
    } else {
        format!("{}?{cleaned}", uri.path())
    };
    if let Ok(parsed) = new_uri.parse::<axum::http::Uri>() {
        *request.uri_mut() = parsed;
    }
}

fn runner_query_access_tokens_enabled() -> bool {
    std::env::var("REMOTE_CODE_RUNNER_ALLOW_QUERY_ACCESS_TOKEN")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn constant_time_token_eq(provided: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};

    let provided_digest: [u8; 32] = Sha256::digest(provided.as_bytes()).into();
    let expected_digest: [u8; 32] = Sha256::digest(expected.as_bytes()).into();
    constant_time_eq::constant_time_eq_32(&provided_digest, &expected_digest)
}

async fn get_meta(State(api): State<RunnerApi>) -> Json<RunnerMeta> {
    Json(api.meta.clone())
}

async fn list_sessions(State(api): State<RunnerApi>) -> Json<ListResponse<RunnerSessionRecord>> {
    let sessions = api.sessions.read().await;
    Json(ListResponse {
        items: sessions.values().cloned().collect(),
        latest_sequence: None,
    })
}

async fn get_session(
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Result<Json<RunnerSessionRecord>, ApiError> {
    let sessions = api.sessions.read().await;
    let session = sessions
        .get(&session_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
    Ok(Json(session))
}

async fn list_approvals(State(api): State<RunnerApi>) -> Json<ListResponse<ApprovalRequestRecord>> {
    let approvals = api.approvals.read().await;
    Json(ListResponse {
        items: approvals.values().cloned().collect(),
        latest_sequence: None,
    })
}

async fn get_approval(
    State(api): State<RunnerApi>,
    AxumPath(approval_id): AxumPath<Uuid>,
) -> Result<Json<ApprovalRequestRecord>, ApiError> {
    let approvals = api.approvals.read().await;
    let approval = approvals
        .get(&approval_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("approval `{approval_id}` was not found")))?;
    Ok(Json(approval))
}

async fn list_session_approvals(
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Result<Json<ListResponse<ApprovalRequestRecord>>, ApiError> {
    let sessions = api.sessions.read().await;
    if !sessions.contains_key(&session_id) {
        return Err(ApiError::not_found(format!(
            "session `{session_id}` was not found"
        )));
    }
    drop(sessions);

    let approvals = api.approvals.read().await;
    Ok(Json(ListResponse {
        items: approvals
            .values()
            .filter(|approval| approval.session_id == session_id)
            .cloned()
            .collect(),
        latest_sequence: None,
    }))
}

async fn create_session(
    State(api): State<RunnerApi>,
    Json(request): Json<RunnerSessionCreateRequest>,
) -> Result<(StatusCode, Json<RunnerSessionRecord>), ApiError> {
    let workspace = api
        .meta
        .snapshot
        .registration
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == request.workspace_id)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "workspace `{}` is not owned by this runner",
                request.workspace_id
            ))
        })?;

    let mut sessions = api.sessions.write().await;
    let (active_sessions, queued_sessions) = session_counts(&sessions);
    let max_parallel_sessions = usize::from(
        api.meta
            .snapshot
            .registration
            .capabilities
            .max_parallel_sessions,
    );
    if active_sessions + queued_sessions >= max_parallel_sessions {
        return Err(ApiError::conflict(format!(
            "runner `{}` is at session capacity ({max_parallel_sessions})",
            api.meta.snapshot.registration.runner_id
        )));
    }

    let session_id = request.session_id.unwrap_or_else(Uuid::new_v4);
    let now = Utc::now();
    let record = RunnerSessionRecord {
        session_id,
        runner_id: api.meta.snapshot.registration.runner_id.clone(),
        workspace_id: workspace.workspace_id.clone(),
        state: SessionState::Pending,
        metadata: request.metadata,
        created_at: now,
        updated_at: now,
    };

    sessions.insert(record.session_id, record.clone());
    api.emit_event(RunnerApiEvent::SessionCreated(record.clone()));
    Ok((StatusCode::CREATED, Json(record)))
}

async fn update_session_state(
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<RunnerSessionStateUpdateRequest>,
) -> Result<Json<RunnerSessionRecord>, ApiError> {
    let mut sessions = api.sessions.write().await;
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
    session.state = request.state;
    session.metadata.extend(request.metadata);
    session.updated_at = Utc::now();
    Ok(Json(session.clone()))
}

async fn post_session_command(
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(command): Json<RunnerSessionCommandRequest>,
) -> Result<Json<RunnerSessionCommandResponse>, ApiError> {
    let sessions = api.sessions.read().await;
    if !sessions.contains_key(&session_id) {
        return Err(ApiError::not_found(format!(
            "session `{session_id}` was not found"
        )));
    }
    drop(sessions);

    let message = match &command {
        RunnerSessionCommandRequest::SendPrompt { .. } => "prompt forwarded",
        RunnerSessionCommandRequest::Interrupt => "interrupt forwarded",
    };
    api.emit_event(RunnerApiEvent::SessionCommand {
        session_id,
        command,
    });
    Ok(Json(RunnerSessionCommandResponse {
        session_id,
        accepted: true,
        message: message.to_owned(),
    }))
}

async fn create_approval(
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<ApprovalCreateRequest>,
) -> Result<(StatusCode, Json<ApprovalRequestRecord>), ApiError> {
    let mut sessions = api.sessions.write().await;
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
    let now = Utc::now();
    session.state = SessionState::WaitingApproval;
    session.updated_at = now;

    let approval = ApprovalRequestRecord {
        approval_id: request.approval_id.unwrap_or_else(Uuid::new_v4),
        session_id,
        runner_id: api.meta.snapshot.registration.runner_id.clone(),
        state: ApprovalState::Pending,
        title: request.title,
        description: request.description,
        metadata: request.metadata,
        created_at: now,
        updated_at: now,
        responded_at: None,
        responder: None,
        note: None,
    };
    drop(sessions);

    let mut approvals = api.approvals.write().await;
    approvals.insert(approval.approval_id, approval.clone());
    Ok((StatusCode::CREATED, Json(approval)))
}

async fn apply_approval_decision(
    State(api): State<RunnerApi>,
    AxumPath(approval_id): AxumPath<Uuid>,
    Json(request): Json<ApprovalDecisionRequest>,
) -> Result<Json<ApprovalRequestRecord>, ApiError> {
    let decision = request.decision;
    let mut approvals = api.approvals.write().await;
    let approval = approvals
        .get_mut(&approval_id)
        .ok_or_else(|| ApiError::not_found(format!("approval `{approval_id}` was not found")))?;
    if !matches!(approval.state, ApprovalState::Pending) {
        return Err(ApiError::conflict(format!(
            "approval `{approval_id}` is already resolved"
        )));
    }

    let now = Utc::now();
    approval.state = decision.into();
    approval.updated_at = now;
    approval.responded_at = Some(now);
    approval.responder = request.responder;
    approval.note = request.note;
    let updated = approval.clone();
    let has_pending_approvals = approvals.values().any(|candidate| {
        candidate.session_id == updated.session_id
            && candidate.approval_id != updated.approval_id
            && matches!(candidate.state, ApprovalState::Pending)
    });
    drop(approvals);

    let mut sessions = api.sessions.write().await;
    if let Some(session) = sessions.get_mut(&updated.session_id) {
        session.state = session_state_after_approval(decision, has_pending_approvals);
        session.updated_at = now;
    }

    api.emit_event(RunnerApiEvent::ApprovalResolved(updated.clone()));

    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// WebSocket event streaming (direct-connect)
// ---------------------------------------------------------------------------

async fn subscribe_session_events(
    ws: WebSocketUpgrade,
    State(api): State<RunnerApi>,
    AxumPath(session_id): AxumPath<Uuid>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let after: u64 = params
        .get("after")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    ws.on_upgrade(move |socket| serve_runner_session_stream(socket, api, session_id, after))
}

async fn serve_runner_session_stream(
    mut socket: axum::extract::ws::WebSocket,
    api: RunnerApi,
    session_id: Uuid,
    after: u64,
) {
    // Replay backlog: drain recent events from the broadcast buffer,
    // filter by session and sequence, and send those the client hasn't seen.
    if after > 0 {
        let mut rx = api.subscribe_stream_events();
        // The broadcast receiver starts at the tail of the channel buffer.
        // Try to receive and filter — events with sequence <= after are skipped.
        let replay_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(500);
        loop {
            match tokio::time::timeout(tokio::time::Duration::from_millis(50), rx.recv()).await {
                Ok(Ok((sid, line))) if sid == session_id => {
                    if let Some(seq) = extract_sequence_from_json(&line)
                        && seq <= after
                    {
                        continue;
                    }
                    if socket
                        .send(axum::extract::ws::Message::Text(line.into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(Ok(_)) => continue,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => break,
                Ok(Err(_)) => break,
                Err(_) => break, // timeout — done replaying
            }
            if tokio::time::Instant::now() >= replay_deadline {
                break;
            }
        }
    }

    // Stream live events.
    let mut rx = api.subscribe_stream_events();
    loop {
        match rx.recv().await {
            Ok((sid, line)) if sid == session_id => {
                if socket
                    .send(axum::extract::ws::Message::Text(line.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(_) => continue,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let _ = socket.close().await;
                break;
            }
            Err(_) => break,
        }
    }
}

fn extract_sequence_from_json(line: &str) -> Option<u64> {
    // Fast path: look for "sequence":<number> without full parse.
    let needle = "\"sequence\":";
    let start = line.find(needle)?;
    let num_start = start + needle.len();
    let num_str = line[num_start..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?;
    num_str.parse().ok()
}

#[derive(Debug, Clone, Serialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

#[derive(Debug, Clone)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message,
        }
    }

    fn conflict(message: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message,
        }
    }

    fn unauthorized(message: String) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorDetail {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

fn parse_socket_addr(raw: &str) -> Result<SocketAddr> {
    SocketAddr::from_str(raw).with_context(|| format!("invalid socket address `{raw}`"))
}

fn parse_env_number<T>(key: &str) -> Option<T>
where
    T: FromStr,
{
    read_env(key).and_then(|value| value.parse::<T>().ok())
}

fn read_env(key: &str) -> Option<String> {
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
    use super::*;
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::{Path as AxumPath, State},
        http::Request,
        routing::post,
    };
    use serde::de::DeserializeOwned;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::{net::TcpListener, sync::Mutex};
    use tower::ServiceExt;

    #[test]
    fn workspace_parser_supports_multiple_entries() {
        let workspaces = parse_runner_workspaces("default=C:\\repo|rw;docs=C:\\docs|ro")
            .expect("workspaces should parse");
        assert_eq!(workspaces.len(), 2);
        assert!(workspaces[0].writable);
        assert!(!workspaces[1].writable);
    }

    #[test]
    fn strip_auth_from_runner_uri_removes_secret_query_without_trailing_marker() {
        let mut request = Request::get("/v1/events/stream?access_token=secret")
            .body(Body::empty())
            .expect("request should build");
        strip_auth_from_request_uri(&mut request);
        assert_eq!(request.uri().to_string(), "/v1/events/stream");

        let mut request = Request::get("/v1/events/stream?after=7&token=secret")
            .body(Body::empty())
            .expect("request should build");
        strip_auth_from_request_uri(&mut request);
        assert_eq!(request.uri().to_string(), "/v1/events/stream?after=7");
    }

    #[test]
    fn load_runner_config_uses_overrides() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-a".to_owned()),
                control_plane_url: Some("http://127.0.0.1:8787".to_owned()),
                bind: Some(SocketAddr::from_str("127.0.0.1:9999").expect("bind should parse")),
                public_base_url: Some("http://127.0.0.1:9999".to_owned()),
                heartbeat_interval_secs: Some(30),
                max_parallel_sessions: Some(8),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: PathBuf::from("C:/workspace"),
                    writable: true,
                }]),
                labels: Some(BTreeMap::from([(
                    String::from("region"),
                    String::from("lab"),
                )])),
                auth_token: Some("runner-secret".to_owned()),
                control_plane_auth_token: Some("control-plane-secret".to_owned()),
            },
        )
        .expect("config should load");

        assert_eq!(config.runner_id, "runner-a");
        assert_eq!(config.bind.to_string(), "127.0.0.1:9999");
        assert_eq!(config.max_parallel_sessions, 8);
        assert_eq!(config.labels.get("region").map(String::as_str), Some("lab"));
        assert_eq!(config.auth_token.as_deref(), Some("runner-secret"));
        assert_eq!(
            config.control_plane_auth_token.as_deref(),
            Some("control-plane-secret")
        );
    }

    #[test]
    fn remote_runner_config_requires_auth_and_https() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                bind: Some(SocketAddr::from_str("0.0.0.0:9999").expect("bind should parse")),
                public_base_url: Some("http://remote.example.com".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: PathBuf::from("C:/workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");

        let issues = validate_runner_config(&config);
        assert!(issues.iter().any(|issue| issue.contains("non-loopback")));
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("public_base_url requires"))
        );
        assert!(issues.iter().any(|issue| issue.contains("must use https")));
    }

    #[test]
    fn encode_path_segment_escapes_reserved_bytes() {
        assert_eq!(encode_path_segment("runner-a"), "runner-a");
        assert_eq!(encode_path_segment("runner/a b?c"), "runner%2Fa%20b%3Fc");
    }

    #[tokio::test]
    async fn runner_router_creates_and_reads_sessions() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-a".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let create_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "workspace_id": "default",
                            "metadata": {"kind": "smoke"}
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let session: RunnerSessionRecord = read_json(create_response).await;

        let get_response = app
            .oneshot(
                Request::get(format!("/v1/sessions/{}", session.session_id))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        assert_eq!(get_response.status(), StatusCode::OK);
        let loaded: RunnerSessionRecord = read_json(get_response).await;
        assert_eq!(loaded.workspace_id, "default");
        assert_eq!(
            loaded.metadata.get("kind").map(String::as_str),
            Some("smoke")
        );
    }

    #[tokio::test]
    async fn runner_router_requires_bearer_token_when_configured() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-auth".to_owned()),
                auth_token: Some("runner-secret".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let health_response = app
            .clone()
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("health request should complete");
        assert_eq!(health_response.status(), StatusCode::OK);

        let unauthorized = app
            .clone()
            .oneshot(
                Request::get("/v1/meta")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::get("/v1/meta")
                    .header("authorization", "Bearer runner-secret")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should complete");
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn runner_router_emits_session_command_events() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-command".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(RUNNER_EVENT_CHANNEL_CAPACITY);
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0")
            .with_event_channel(event_tx)
            .router();

        let create_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": Uuid::nil(),
                            "workspace_id": "default",
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("session create should succeed");
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let _ = event_rx
            .recv()
            .await
            .expect("session create event should arrive");

        let command_response = app
            .oneshot(
                Request::post(format!("/v1/sessions/{}/commands", Uuid::nil()))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&RunnerSessionCommandRequest::SendPrompt {
                            content: "hello remote".to_owned(),
                        })
                        .expect("request should serialize"),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("command request should succeed");
        assert_eq!(command_response.status(), StatusCode::OK);
        let response: RunnerSessionCommandResponse = read_json(command_response).await;
        assert!(response.accepted);
        assert_eq!(response.session_id, Uuid::nil());

        match event_rx.recv().await.expect("command event should arrive") {
            RunnerApiEvent::SessionCommand {
                session_id,
                command,
            } => {
                assert_eq!(session_id, Uuid::nil());
                assert_eq!(
                    command,
                    RunnerSessionCommandRequest::SendPrompt {
                        content: "hello remote".to_owned()
                    }
                );
            }
            other => panic!("unexpected runner event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn health_endpoint_reports_busy_state_when_sessions_exist() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-b".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let _ = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let response = app
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let health: RunnerHealth = read_json(response).await;
        assert_eq!(health.state, RunnerState::Idle);
        assert_eq!(health.queued_sessions, 1);
    }

    #[tokio::test]
    async fn runner_api_heartbeat_reports_session_counts() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-c".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let api = RunnerApi::new(config, "remote-code-runner", "0.1.0");
        let app = api.clone().router();

        let _ = app
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed");

        let heartbeat = api.heartbeat().await;
        assert_eq!(heartbeat.runner_id, "runner-c");
        assert_eq!(heartbeat.state, RunnerState::Idle);
        assert_eq!(heartbeat.active_sessions, 0);
        assert_eq!(heartbeat.queued_sessions, 1);
    }

    #[tokio::test]
    async fn session_state_updates_change_health_and_heartbeat_counts() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-state".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let api = RunnerApi::new(config, "remote-code-runner", "0.1.0");
        let app = api.clone().router();

        let create_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "workspace_id": "default",
                            "metadata": {"phase": "queued"}
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("session create should succeed");
        let created: RunnerSessionRecord = read_json(create_response).await;

        let queued_health_response = app
            .clone()
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("queued health request should succeed");
        let queued_health: RunnerHealth = read_json(queued_health_response).await;
        assert_eq!(queued_health.state, RunnerState::Idle);
        assert_eq!(queued_health.active_sessions, 0);
        assert_eq!(queued_health.queued_sessions, 1);

        let running_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/sessions/{}/state", created.session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!(RunnerSessionStateUpdateRequest {
                            state: SessionState::Running,
                            metadata: BTreeMap::from([("phase".to_owned(), "running".to_owned(),)]),
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("state update should succeed");
        assert_eq!(running_response.status(), StatusCode::OK);
        let running_session: RunnerSessionRecord = read_json(running_response).await;
        assert_eq!(running_session.state, SessionState::Running);
        assert_eq!(
            running_session.metadata.get("phase").map(String::as_str),
            Some("running")
        );

        let running_health_response = app
            .clone()
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("running health request should succeed");
        let running_health: RunnerHealth = read_json(running_health_response).await;
        assert_eq!(running_health.state, RunnerState::Busy);
        assert_eq!(running_health.active_sessions, 1);
        assert_eq!(running_health.queued_sessions, 0);

        let running_heartbeat = api.heartbeat().await;
        assert_eq!(running_heartbeat.state, RunnerState::Busy);
        assert_eq!(running_heartbeat.active_sessions, 1);
        assert_eq!(running_heartbeat.queued_sessions, 0);

        let completed_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/sessions/{}/state", created.session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!(RunnerSessionStateUpdateRequest {
                            state: SessionState::Completed,
                            metadata: BTreeMap::from([("result".to_owned(), "ok".to_owned())]),
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("completion update should succeed");
        assert_eq!(completed_response.status(), StatusCode::OK);
        let completed_session: RunnerSessionRecord = read_json(completed_response).await;
        assert_eq!(completed_session.state, SessionState::Completed);
        assert_eq!(
            completed_session.metadata.get("phase").map(String::as_str),
            Some("running")
        );
        assert_eq!(
            completed_session.metadata.get("result").map(String::as_str),
            Some("ok")
        );

        let completed_health_response = app
            .clone()
            .oneshot(
                Request::get("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("completed health request should succeed");
        let completed_health: RunnerHealth = read_json(completed_health_response).await;
        assert_eq!(completed_health.state, RunnerState::Idle);
        assert_eq!(completed_health.active_sessions, 0);
        assert_eq!(completed_health.queued_sessions, 0);

        let completed_heartbeat = api.heartbeat().await;
        assert_eq!(completed_heartbeat.state, RunnerState::Idle);
        assert_eq!(completed_heartbeat.active_sessions, 0);
        assert_eq!(completed_heartbeat.queued_sessions, 0);
    }

    #[tokio::test]
    async fn send_heartbeat_url_encodes_runner_id_segments() {
        #[derive(Clone)]
        struct HeartbeatCapture {
            runner_id: Arc<Mutex<Option<String>>>,
        }

        async fn capture_heartbeat(
            State(state): State<HeartbeatCapture>,
            AxumPath(runner_id): AxumPath<String>,
            Json(heartbeat): Json<RunnerHeartbeat>,
        ) -> Json<RunnerSnapshot> {
            *state.runner_id.lock().await = Some(runner_id.clone());
            Json(RunnerSnapshot {
                registration: RunnerRegistrationRequest {
                    runner_id,
                    control_plane_url: None,
                    public_base_url: Some("http://127.0.0.1:9".to_owned()),
                    workspaces: Vec::new(),
                    labels: BTreeMap::new(),
                    auth_token: None,
                    capabilities: RunnerCapabilities {
                        interactive_approvals: true,
                        background_sessions: true,
                        artifact_uploads: true,
                        max_parallel_sessions: 4,
                    },
                    platform: RunnerPlatform {
                        os: "windows".to_owned(),
                        arch: "x86_64".to_owned(),
                        family: "windows".to_owned(),
                    },
                },
                state: heartbeat.state,
                active_sessions: heartbeat.active_sessions,
                queued_sessions: heartbeat.queued_sessions,
                registered_at: heartbeat.timestamp,
                last_seen_at: heartbeat.timestamp,
            })
        }

        let state = HeartbeatCapture {
            runner_id: Arc::new(Mutex::new(None)),
        };
        let app = Router::new()
            .route("/v1/runners/{runner_id}/heartbeat", post(capture_heartbeat))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("address should be readable");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("server should keep serving");
        });

        let heartbeat = RunnerHeartbeat {
            runner_id: "runner/a b?c".to_owned(),
            state: RunnerState::Busy,
            active_sessions: 2,
            queued_sessions: 1,
            timestamp: Utc::now(),
        };
        let snapshot = send_heartbeat(
            &Client::new(),
            &format!("http://{address}"),
            &heartbeat,
            None,
        )
        .await
        .expect("heartbeat request should succeed");
        assert_eq!(snapshot.registration.runner_id, "runner/a b?c");
        assert_eq!(
            state.runner_id.lock().await.as_deref(),
            Some("runner/a b?c")
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn runner_router_creates_and_resolves_approvals() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-approval".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let create_session_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("session create should succeed");
        let session: RunnerSessionRecord = read_json(create_session_response).await;

        let approval_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/sessions/{}/approvals", session.session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "approval_id": Uuid::nil(),
                            "title": "Execute shell command",
                            "description": "Needs user confirmation"
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("approval create should succeed");
        assert_eq!(approval_response.status(), StatusCode::CREATED);
        let approval: ApprovalRequestRecord = read_json(approval_response).await;
        assert_eq!(approval.approval_id, Uuid::nil());
        assert_eq!(approval.state, ApprovalState::Pending);

        let list_response = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/sessions/{}/approvals", session.session_id))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("approval list should succeed");
        let approvals: ListResponse<ApprovalRequestRecord> = read_json(list_response).await;
        assert_eq!(approvals.items.len(), 1);

        let decide_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/approvals/{}/decision", approval.approval_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "approved",
                            "responder": "tester",
                            "note": "Ship it"
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("approval decision should succeed");
        assert_eq!(decide_response.status(), StatusCode::OK);
        let resolved: ApprovalRequestRecord = read_json(decide_response).await;
        assert_eq!(resolved.state, ApprovalState::Approved);
        assert_eq!(resolved.responder.as_deref(), Some("tester"));

        let session_response = app
            .oneshot(
                Request::get(format!("/v1/sessions/{}", session.session_id))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("session fetch should succeed");
        let updated_session: RunnerSessionRecord = read_json(session_response).await;
        assert_eq!(updated_session.state, SessionState::Running);
    }

    #[tokio::test]
    async fn runner_router_keeps_waiting_for_remaining_approvals_and_handles_denial() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-approval-multi".to_owned()),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let create_session_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("session create should succeed");
        let session: RunnerSessionRecord = read_json(create_session_response).await;

        let create_approval = |title: &str| {
            Request::post(format!("/v1/sessions/{}/approvals", session.session_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "title": title,
                        "description": "Needs user confirmation"
                    })
                    .to_string(),
                ))
                .expect("request should build")
        };

        let first_response = app
            .clone()
            .oneshot(create_approval("First approval"))
            .await
            .expect("first approval create should succeed");
        let first: ApprovalRequestRecord = read_json(first_response).await;

        let second_response = app
            .clone()
            .oneshot(create_approval("Second approval"))
            .await
            .expect("second approval create should succeed");
        let second: ApprovalRequestRecord = read_json(second_response).await;

        let approve_first_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/approvals/{}/decision", first.approval_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "approved",
                            "responder": "tester"
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("first approval decision should succeed");
        assert_eq!(approve_first_response.status(), StatusCode::OK);

        let waiting_session_response = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/sessions/{}", session.session_id))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("session fetch should succeed");
        let waiting_session: RunnerSessionRecord = read_json(waiting_session_response).await;
        assert_eq!(waiting_session.state, SessionState::WaitingApproval);

        let deny_second_response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/approvals/{}/decision", second.approval_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "denied",
                            "responder": "tester",
                            "note": "Denied for safety"
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("second approval decision should succeed");
        assert_eq!(deny_second_response.status(), StatusCode::OK);
        let denied: ApprovalRequestRecord = read_json(deny_second_response).await;
        assert_eq!(denied.state, ApprovalState::Denied);

        let failed_session_response = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/sessions/{}", session.session_id))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("session fetch should succeed");
        let failed_session: RunnerSessionRecord = read_json(failed_session_response).await;
        assert_eq!(failed_session.state, SessionState::Failed);

        let duplicate_decision_response = app
            .oneshot(
                Request::post(format!("/v1/approvals/{}/decision", second.approval_id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "approved",
                            "responder": "tester"
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("duplicate decision request should complete");
        assert_eq!(duplicate_decision_response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn runner_router_rejects_sessions_above_capacity() {
        let profile_dir = tempdir().expect("tempdir should exist");
        let config = load_runner_config(
            Some(profile_dir.path().join("profile")),
            RunnerConfigOverrides {
                runner_id: Some("runner-capacity".to_owned()),
                max_parallel_sessions: Some(1),
                workspaces: Some(vec![RunnerWorkspace {
                    workspace_id: "default".to_owned(),
                    root_dir: profile_dir.path().join("workspace"),
                    writable: true,
                }]),
                ..RunnerConfigOverrides::default()
            },
        )
        .expect("config should load");
        let app = RunnerApi::new(config, "remote-code-runner", "0.1.0").router();

        let first_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("first request should succeed");
        assert_eq!(first_response.status(), StatusCode::CREATED);

        let second_response = app
            .clone()
            .oneshot(
                Request::post("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"workspace_id": "default"}).to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("second request should complete");
        assert_eq!(second_response.status(), StatusCode::CONFLICT);
    }

    async fn read_json<T>(response: Response<Body>) -> T
    where
        T: DeserializeOwned,
    {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        serde_json::from_slice(&body).expect("json should parse")
    }
}
