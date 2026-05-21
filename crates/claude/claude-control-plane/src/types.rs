//! Shared types, constants, and configuration for the control plane.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use claude_runner::{
    ApprovalCreateRequest, ApprovalDecisionRequest, ApprovalState, RunnerSessionCommandRequest,
    RunnerSessionCreateRequest, RunnerSessionStateUpdateRequest, RunnerSnapshot, RunnerState,
};
pub use rc_engine_events::{
    DaemonPresenceState, MessageRole, RuntimeEventCreateRequest, RuntimeEventDetail,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const DEFAULT_BIND: &str = "127.0.0.1:8787";
pub(crate) const DEFAULT_RUNNER_LEASE_TTL_SECS: u64 = 30;
pub(crate) const DEFAULT_EVENT_HISTORY_LIMIT: usize = 256;
pub(crate) const DEFAULT_EVENT_LIST_LIMIT: usize = 50;
pub(crate) const MAX_EVENT_LIST_LIMIT: usize = 200;
pub(crate) const DEFAULT_PAIRING_TTL_SECS: u64 = 600;
pub(crate) const MAX_PAIRING_TTL_SECS: u64 = 3600;
pub(crate) const STREAM_TICKET_TTL_SECS: u64 = 45;
pub(crate) const MAX_ARTIFACT_SIZE_BYTES: usize = 10 * 1024 * 1024; // 10 MiB
pub(crate) const EVENT_STREAM_BUFFER: usize = 256;
pub(crate) const PHASE: &str = "phase5-remote-stable";

// ---------------------------------------------------------------------------
// Public configuration types
// ---------------------------------------------------------------------------

/// CLI / env-var overrides for control plane configuration.
#[derive(Debug, Clone, Default)]
pub struct ControlPlaneConfigOverrides {
    /// Bind address override.
    pub bind: Option<SocketAddr>,
    /// Public base URL override.
    pub public_base_url: Option<String>,
    /// Service name override.
    pub service_name: Option<String>,
    /// Runner lease TTL override in seconds.
    pub runner_lease_ttl_secs: Option<u64>,
    /// Profile directory override.
    pub profile_dir: Option<PathBuf>,
    /// Shared bearer token required for remote access.
    pub auth_token: Option<String>,
    /// Bootstrap secret used to claim the first trusted device.
    pub bootstrap_secret: Option<String>,
    /// Directory containing downloadable app binaries (APK, etc.).
    pub downloads_dir: Option<PathBuf>,
    /// UDP bind address for QUIC transport (optional — requires TLS certs).
    pub quic_bind: Option<SocketAddr>,
    /// Path to PEM-encoded TLS certificate for QUIC.
    pub quic_cert_pem: Option<PathBuf>,
    /// Path to PEM-encoded TLS private key for QUIC.
    pub quic_key_pem: Option<PathBuf>,
}

/// Full control plane configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneConfig {
    /// Address to bind the HTTP server to.
    pub bind: SocketAddr,
    /// Publicly reachable URL (for SSE / WebSocket endpoints).
    pub public_base_url: Option<String>,
    /// Service name for identification.
    pub service_name: String,
    /// Runner lease TTL in seconds.
    pub runner_lease_ttl_secs: u64,
    /// Profile directory for persistent data.
    pub profile_dir: PathBuf,
    /// SQLite database path for state persistence.
    pub state_db_path: PathBuf,
    /// Root directory for artifact storage.
    pub artifact_root_dir: PathBuf,
    /// Shared bearer token required for remote access.
    pub auth_token: Option<String>,
    /// Bootstrap secret used to claim the first trusted device.
    pub bootstrap_secret: Option<String>,
    /// Directory containing downloadable app binaries served at /downloads/.
    pub downloads_dir: Option<PathBuf>,
    /// UDP bind address for QUIC transport.
    pub quic_bind: Option<SocketAddr>,
    /// Path to PEM-encoded TLS certificate for QUIC.
    pub quic_cert_pem: Option<PathBuf>,
    /// Path to PEM-encoded TLS private key for QUIC.
    pub quic_key_pem: Option<PathBuf>,
}

/// Metadata returned by the `/meta` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneMeta {
    /// Service name.
    pub service: String,
    /// Service version.
    pub version: String,
    /// Development phase identifier.
    pub phase: String,
    /// Bind address.
    pub bind: String,
    /// Public base URL.
    pub public_base_url: Option<String>,
    /// Runner lease TTL.
    pub runner_lease_ttl_secs: u64,
    /// Profile directory path.
    pub profile_dir: String,
    /// SQLite database path.
    pub state_db_path: String,
    /// Artifact root directory path.
    pub artifact_root_dir: String,
    /// Whether the `/v1/*` API requires a bearer token.
    pub auth_required: bool,
    /// Whether a bootstrap secret is configured.
    pub bootstrap_secret_configured: bool,
}

/// Status report for the `doctor` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneStatus {
    /// Whether the configuration is valid.
    pub ok: bool,
    /// Blocking issues that must be resolved before the service is considered safe to expose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    /// Bind address.
    pub bind: String,
    /// Public base URL.
    pub public_base_url: Option<String>,
    /// Service name.
    pub service_name: String,
    /// Runner lease TTL.
    pub runner_lease_ttl_secs: u64,
    /// Profile directory path.
    pub profile_dir: String,
    /// SQLite database path.
    pub state_db_path: String,
    /// Artifact root directory path.
    pub artifact_root_dir: String,
    /// Whether the `/v1/*` API requires a bearer token.
    pub auth_required: bool,
    /// Whether a bootstrap secret is configured.
    pub bootstrap_secret_configured: bool,
    /// Development phase.
    pub phase: &'static str,
}

/// Health check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneHealth {
    /// Whether the service is healthy.
    pub ok: bool,
    /// Service name.
    pub service: String,
    /// Development phase.
    pub phase: String,
    /// Total registered runners.
    pub runner_count: usize,
    /// Currently available runners.
    pub available_runner_count: usize,
    /// Total sessions.
    pub session_count: usize,
    /// Total artifacts.
    pub artifact_count: usize,
    /// Number of pending runner pull commands.
    pub queued_runner_command_count: usize,
    /// Whether the `/v1/*` API currently requires authentication.
    pub auth_required: bool,
    /// Whether a bootstrap secret is configured.
    pub bootstrap_secret_configured: bool,
    /// Whether the owner device has already claimed the control plane.
    pub owner_claimed: bool,
    /// Number of trusted devices.
    pub device_count: usize,
}

// ---------------------------------------------------------------------------
// Trusted-device / pairing types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Runner,
    Browser,
    #[default]
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDeviceRecord {
    pub device_id: Uuid,
    pub name: String,
    pub kind: DeviceKind,
    pub owner: bool,
    pub created_by_device_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapClaimRequest {
    pub bootstrap_secret: String,
    pub device_name: String,
    #[serde(default)]
    pub device_kind: DeviceKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapClaimResponse {
    pub device: TrustedDeviceRecord,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingOfferCreateRequest {
    pub device_name: String,
    #[serde(default)]
    pub device_kind: DeviceKind,
    pub expires_in_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingOfferCreateResponse {
    pub offer_id: Uuid,
    pub device_name: String,
    pub device_kind: DeviceKind,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub pairing_secret: String,
    pub pairing_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingAcceptRequest {
    pub offer_id: Uuid,
    pub pairing_secret: String,
    pub device_name: Option<String>,
    pub device_kind: Option<DeviceKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingAcceptResponse {
    pub device: TrustedDeviceRecord,
    pub access_token: String,
    pub refresh_token: String,
}

// ---------------------------------------------------------------------------
// Token refresh
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRefreshResponse {
    pub access_token: String,
}

// ---------------------------------------------------------------------------
// WebSocket stream tickets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTicketRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTicketResponse {
    pub stream_ticket: String,
    pub expires_in_secs: u64,
}

// ---------------------------------------------------------------------------
// Push-token registration (mobile devices)
// ---------------------------------------------------------------------------

/// Push notification platform.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PushPlatform {
    #[default]
    Apns,
    Fcm,
}

/// Request body for `POST /v1/devices/push-token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushTokenRegistrationRequest {
    pub push_token: String,
    #[serde(default)]
    pub platform: PushPlatform,
}

/// Response body for `POST /v1/devices/push-token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushTokenRegistrationResponse {
    pub registered: bool,
}

// ---------------------------------------------------------------------------
// Runner pull-command types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunnerQueuedCommandBody {
    CreateSession {
        request: RunnerSessionCreateRequest,
    },
    UpdateSessionState {
        session_id: Uuid,
        request: RunnerSessionStateUpdateRequest,
    },
    SessionCommand {
        session_id: Uuid,
        request: RunnerSessionCommandRequest,
    },
    CreateApproval {
        session_id: Uuid,
        request: ApprovalCreateRequest,
    },
    ApplyApprovalDecision {
        approval_id: Uuid,
        request: ApprovalDecisionRequest,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerQueuedCommand {
    pub command_id: Uuid,
    pub runner_id: String,
    pub created_at: DateTime<Utc>,
    pub body: RunnerQueuedCommandBody,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerCommandPullResponse {
    pub commands: Vec<RunnerQueuedCommand>,
}

// ---------------------------------------------------------------------------
// Public session types
// ---------------------------------------------------------------------------

/// Lifecycle state of a control-plane session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Waiting for runner assignment.
    #[default]
    Pending,
    /// Assigned to a runner, not yet started.
    Assigned,
    /// Currently running.
    Running,
    /// Waiting for user approval.
    WaitingApproval,
    /// Completed successfully.
    Completed,
    /// Failed.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// Persistent record of a control-plane session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Session identifier.
    pub session_id: Uuid,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Runner currently owning this session.
    pub owner_runner_id: Option<String>,
    /// Current session state.
    pub state: SessionState,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Tenant-scoping user identity.  Set to an explicitly provisioned
    /// user-key token when the session is created by an `AuthPrincipal::User`.
    /// `None` for legacy sessions or admin-created sessions (visible to all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}

/// Session payload exposed by the HTTP API with dynamic runner availability metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    #[serde(flatten)]
    pub session: SessionRecord,
    #[serde(default)]
    pub owner_runner_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_runner_state: Option<RunnerState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_runner_last_seen_at: Option<DateTime<Utc>>,
    /// Direct-connect URL for the runner hosting this session.
    /// When present, clients can stream events and send commands
    /// directly to the runner instead of relaying through the control plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_runner_public_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub session_id: Option<Uuid>,
    pub workspace_id: String,
    pub preferred_runner_id: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateUpdateRequest {
    pub state: SessionState,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Public runner types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerRegistrationResponse {
    pub runner_id: String,
    pub registered_at: DateTime<Utc>,
    pub lease_ttl_secs: u64,
    pub snapshot: RunnerSnapshot,
}

// ---------------------------------------------------------------------------
// Public artifact types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: Uuid,
    pub session_id: Uuid,
    pub runner_id: Option<String>,
    pub name: String,
    pub file_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactCreateRequest {
    pub name: String,
    pub file_name: Option<String>,
    pub media_type: Option<String>,
    pub content_base64: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Public timeline types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub sequence: u64,
    pub recorded_at: DateTime<Utc>,
    pub runner_id: Option<String>,
    pub session_id: Option<Uuid>,
    pub detail: TimelineEventDetail,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SharedRuntimeEventContract {
    MessageDelta {
        role: MessageRole,
        delta: String,
        #[serde(default)]
        message_id: Option<String>,
    },
    MessageCommitted {
        role: MessageRole,
        text: String,
        #[serde(default)]
        message_id: Option<String>,
    },
    ToolStarted {
        #[serde(alias = "tool_call_id")]
        tool_use_id: String,
        tool_name: String,
    },
    ToolProgress {
        #[serde(flatten)]
        progress: SharedToolProgressPayload,
    },
    ToolFinished {
        #[serde(alias = "tool_call_id")]
        tool_use_id: String,
        tool_name: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        summary: Option<String>,
    },
    ArtifactManifest {
        artifact_ids: Vec<Uuid>,
    },
    RuntimeError {
        message: String,
    },
    SubtaskStarted {
        task_id: String,
        #[serde(default)]
        parent_task_id: Option<String>,
        description: String,
        #[serde(default)]
        depth: u32,
    },
    SubtaskProgress {
        task_id: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        summary: String,
    },
    SubtaskCompleted {
        task_id: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        turns_used: Option<u32>,
    },
    BatchProgress {
        #[serde(default)]
        total: u32,
        #[serde(default)]
        completed: u32,
        #[serde(default)]
        running: u32,
    },
    ContextUsage {
        #[serde(default)]
        estimated_tokens: u64,
        #[serde(default)]
        max_input_tokens: u64,
        #[serde(default)]
        threshold_tokens: u64,
        #[serde(default)]
        ratio: f64,
    },
    ContextOverflow {
        #[serde(default)]
        estimated_tokens: u64,
        #[serde(default)]
        max_input_tokens: u64,
        #[serde(default)]
        threshold_tokens: u64,
        #[serde(default)]
        ratio: f64,
    },
    ContextCompacted {
        #[serde(default)]
        entries_removed: u32,
        #[serde(default)]
        usage_ratio: f64,
    },
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct SharedToolProgressPayload {
    #[serde(default, alias = "tool_call_id", alias = "tool_use_id")]
    tool_use_id: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default, alias = "delta", alias = "input_delta")]
    input_delta: Option<String>,
    #[serde(default)]
    elapsed_time_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineEventDetail {
    RunnerRegistered {
        lease_ttl_secs: u64,
        workspace_ids: Vec<String>,
        state: RunnerState,
    },
    RunnerHeartbeat {
        state: RunnerState,
        active_sessions: usize,
        queued_sessions: usize,
        reported_at: DateTime<Utc>,
    },
    SessionCreated {
        workspace_id: String,
        owner_runner_id: Option<String>,
        state: SessionState,
    },
    SessionStateChanged {
        previous_state: SessionState,
        state: SessionState,
    },
    ApprovalRequested {
        approval_id: Uuid,
        title: String,
        state: ApprovalState,
    },
    ApprovalResolved {
        approval_id: Uuid,
        state: ApprovalState,
        responder: Option<String>,
    },
    ArtifactCreated {
        artifact_id: Uuid,
        name: String,
        file_name: String,
        media_type: String,
        size_bytes: u64,
    },
    MessageDelta {
        role: MessageRole,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    MessageCommitted {
        role: MessageRole,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
    ToolStarted {
        tool_call_id: Arc<str>,
        tool_name: Arc<str>,
    },
    ToolProgress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<Arc<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<Arc<str>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_time_seconds: Option<u64>,
    },
    ToolFinished {
        tool_call_id: Arc<str>,
        tool_name: Arc<str>,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    ArtifactManifest {
        artifact_ids: Vec<Uuid>,
    },
    RuntimeError {
        message: String,
    },
    DaemonPresenceChanged {
        state: DaemonPresenceState,
    },
    SubtaskStarted {
        task_id: Arc<str>,
        parent_task_id: Option<Arc<str>>,
        description: String,
        depth: u32,
    },
    SubtaskProgress {
        task_id: Arc<str>,
        status: String,
        summary: String,
    },
    SubtaskCompleted {
        task_id: Arc<str>,
        status: String,
        summary: String,
        turns_used: Option<u32>,
    },
    BatchProgress {
        total: u32,
        completed: u32,
        running: u32,
    },
    ContextUsage {
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    },
    ContextOverflow {
        estimated_tokens: u64,
        max_input_tokens: u64,
        threshold_tokens: u64,
        ratio: f64,
    },
    ContextCompacted {
        entries_removed: u32,
        usage_ratio: f64,
    },
}

// ---------------------------------------------------------------------------
// Internal timeline types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TimelineEventKind {
    RunnerRegistered,
    RunnerHeartbeat,
    SessionCreated,
    SessionStateChanged,
    ApprovalRequested,
    ApprovalResolved,
    ArtifactCreated,
    MessageDelta,
    MessageCommitted,
    ToolStarted,
    ToolProgress,
    ToolFinished,
    ArtifactManifest,
    RuntimeError,
    DaemonPresenceChanged,
    SubtaskStarted,
    SubtaskProgress,
    SubtaskCompleted,
    BatchProgress,
    ContextUsage,
    ContextOverflow,
    ContextCompacted,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineEventDraft {
    pub(crate) runner_id: Option<String>,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) detail: TimelineEventDetail,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionStateTransition {
    pub(crate) runner_id: Option<String>,
    pub(crate) session_id: Uuid,
    pub(crate) previous_state: SessionState,
    pub(crate) state: SessionState,
}

impl From<RuntimeEventDetail> for TimelineEventDetail {
    fn from(value: RuntimeEventDetail) -> Self {
        match value {
            RuntimeEventDetail::MessageDelta {
                role,
                delta,
                message_id,
            } => Self::MessageDelta {
                role,
                delta,
                message_id,
            },
            RuntimeEventDetail::MessageCommitted {
                role,
                text,
                message_id,
            } => Self::MessageCommitted {
                role,
                text,
                message_id,
            },
            RuntimeEventDetail::ToolStarted {
                tool_call_id,
                tool_name,
            } => Self::ToolStarted {
                tool_call_id,
                tool_name,
            },
            RuntimeEventDetail::ToolProgress {
                tool_call_id,
                tool_name,
                delta,
                elapsed_time_seconds,
            } => Self::ToolProgress {
                tool_call_id,
                tool_name,
                delta,
                elapsed_time_seconds,
            },
            RuntimeEventDetail::ToolFinished {
                tool_call_id,
                tool_name,
                is_error,
                summary,
            } => Self::ToolFinished {
                tool_call_id,
                tool_name,
                is_error,
                summary,
            },
            RuntimeEventDetail::ArtifactManifest { artifact_ids } => {
                Self::ArtifactManifest { artifact_ids }
            }
            RuntimeEventDetail::RuntimeError { message } => Self::RuntimeError { message },
            RuntimeEventDetail::DaemonPresenceChanged { state } => {
                Self::DaemonPresenceChanged { state }
            }
            RuntimeEventDetail::SubtaskStarted {
                task_id,
                parent_task_id,
                description,
                depth,
            } => Self::SubtaskStarted {
                task_id,
                parent_task_id,
                description,
                depth,
            },
            RuntimeEventDetail::SubtaskProgress {
                task_id,
                status,
                summary,
            } => Self::SubtaskProgress {
                task_id,
                status,
                summary,
            },
            RuntimeEventDetail::SubtaskCompleted {
                task_id,
                status,
                summary,
                turns_used,
            } => Self::SubtaskCompleted {
                task_id,
                status,
                summary,
                turns_used,
            },
            RuntimeEventDetail::BatchProgress {
                total,
                completed,
                running,
            } => Self::BatchProgress {
                total,
                completed,
                running,
            },
            RuntimeEventDetail::ContextUsage {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextUsage {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            },
            RuntimeEventDetail::ContextOverflow {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextOverflow {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            },
            RuntimeEventDetail::ContextCompacted {
                entries_removed,
                usage_ratio,
            } => Self::ContextCompacted {
                entries_removed,
                usage_ratio,
            },
        }
    }
}

impl TryFrom<SharedRuntimeEventContract> for RuntimeEventDetail {
    type Error = ();

    fn try_from(value: SharedRuntimeEventContract) -> Result<Self, Self::Error> {
        let detail = match value {
            SharedRuntimeEventContract::MessageDelta {
                role,
                delta,
                message_id,
            } => Self::MessageDelta {
                role,
                delta,
                message_id,
            },
            SharedRuntimeEventContract::MessageCommitted {
                role,
                text,
                message_id,
            } => Self::MessageCommitted {
                role,
                text,
                message_id,
            },
            SharedRuntimeEventContract::ToolStarted {
                tool_use_id,
                tool_name,
            } => Self::ToolStarted {
                tool_call_id: tool_use_id.into(),
                tool_name: tool_name.into(),
            },
            SharedRuntimeEventContract::ToolProgress { progress } => Self::ToolProgress {
                tool_call_id: progress.tool_use_id.map(Into::into),
                tool_name: progress.tool_name.map(Into::into),
                delta: progress.input_delta,
                elapsed_time_seconds: progress.elapsed_time_seconds,
            },
            SharedRuntimeEventContract::ToolFinished {
                tool_use_id,
                tool_name,
                is_error,
                summary,
            } => Self::ToolFinished {
                tool_call_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                is_error,
                summary,
            },
            SharedRuntimeEventContract::ArtifactManifest { artifact_ids } => {
                Self::ArtifactManifest { artifact_ids }
            }
            SharedRuntimeEventContract::RuntimeError { message } => Self::RuntimeError { message },
            SharedRuntimeEventContract::SubtaskStarted {
                task_id,
                parent_task_id,
                description,
                depth,
            } => Self::SubtaskStarted {
                task_id: task_id.into(),
                parent_task_id: parent_task_id.map(Into::into),
                description,
                depth,
            },
            SharedRuntimeEventContract::SubtaskProgress {
                task_id,
                status,
                summary,
            } => Self::SubtaskProgress {
                task_id: task_id.into(),
                status,
                summary,
            },
            SharedRuntimeEventContract::SubtaskCompleted {
                task_id,
                status,
                summary,
                turns_used,
            } => Self::SubtaskCompleted {
                task_id: task_id.into(),
                status,
                summary,
                turns_used,
            },
            SharedRuntimeEventContract::BatchProgress {
                total,
                completed,
                running,
            } => Self::BatchProgress {
                total,
                completed,
                running,
            },
            SharedRuntimeEventContract::ContextUsage {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextUsage {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            },
            SharedRuntimeEventContract::ContextOverflow {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            } => Self::ContextOverflow {
                estimated_tokens,
                max_input_tokens,
                threshold_tokens,
                ratio,
            },
            SharedRuntimeEventContract::ContextCompacted {
                entries_removed,
                usage_ratio,
            } => Self::ContextCompacted {
                entries_removed,
                usage_ratio,
            },
        };
        Ok(detail)
    }
}

#[must_use]
pub fn runtime_event_detail_from_stream_json_value(
    value: &JsonValue,
) -> Option<RuntimeEventDetail> {
    serde_json::from_value::<SharedRuntimeEventContract>(value.clone())
        .ok()
        .and_then(|event| RuntimeEventDetail::try_from(event).ok())
}

#[cfg(test)]
mod tests {
    use super::{RuntimeEventDetail, runtime_event_detail_from_stream_json_value};
    use serde_json::json;

    #[test]
    fn stream_json_parser_accepts_shared_tool_progress_fields() {
        let detail = runtime_event_detail_from_stream_json_value(&json!({
            "type": "tool_progress",
            "tool_use_id": "tool-1",
            "tool_name": "shell",
            "input_delta": "dir",
            "elapsed_time_seconds": 2
        }))
        .expect("shared stream-json event should parse");

        assert_eq!(
            detail,
            RuntimeEventDetail::ToolProgress {
                tool_call_id: Some("tool-1".into()),
                tool_name: Some("shell".into()),
                delta: Some("dir".to_owned()),
                elapsed_time_seconds: Some(2),
            }
        );
    }

    #[test]
    fn stream_json_parser_accepts_legacy_tool_progress_aliases() {
        let detail = runtime_event_detail_from_stream_json_value(&json!({
            "type": "tool_progress",
            "tool_call_id": "tool-legacy",
            "tool_name": "shell",
            "delta": "ls"
        }))
        .expect("legacy stream-json event should parse");

        assert_eq!(
            detail,
            RuntimeEventDetail::ToolProgress {
                tool_call_id: Some("tool-legacy".into()),
                tool_name: Some("shell".into()),
                delta: Some("ls".to_owned()),
                elapsed_time_seconds: None,
            }
        );
    }
}

// ---------------------------------------------------------------------------
// Internal query types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RecentEventsQuery {
    pub(crate) after: Option<u64>,
    pub(crate) limit: Option<usize>,
    pub(crate) kind: Option<TimelineEventKind>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ListSessionsQuery {
    pub(crate) runner_id: Option<String>,
    pub(crate) workspace_id: Option<String>,
    pub(crate) state: Option<SessionState>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct EventStreamQuery {
    pub(crate) after: Option<u64>,
    pub(crate) kind: Option<TimelineEventKind>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RunnerCommandPullQuery {
    pub(crate) limit: Option<usize>,
    pub(crate) timeout: Option<u64>,
}

// ---------------------------------------------------------------------------
// Internal error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub(crate) error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ErrorDetail {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message,
        }
    }

    pub(crate) fn conflict(message: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message,
        }
    }

    pub(crate) fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message,
        }
    }

    pub(crate) fn service_unavailable(message: String) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message,
        }
    }

    pub(crate) fn bad_gateway(message: String) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "bad_gateway",
            message,
        }
    }

    pub(crate) fn unauthorized(message: String) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message,
        }
    }

    pub(crate) fn forbidden(message: String) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message,
        }
    }

    pub(crate) fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
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
