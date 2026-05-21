//! Internal adapter types for the Codex adapter.
//!
//! Contains request/response structs, pending server request kinds,
//! event pump state, and helper conversions.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use rc_agent_protocol::events::UnifiedAgentEvent;
use rc_agent_protocol::permission::PermissionDecision;

use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::{
    CommandExecParams, DynamicToolCallOutputContentItem, DynamicToolCallResponse,
    FeedbackUploadParams, FileChangeApprovalDecision, GrantedPermissionProfile,
    McpServerElicitationAction, McpServerElicitationRequestResponse, PermissionGrantScope,
    RequestId, RequestPermissionProfile, SortDirection, ThreadListCwdFilter, ThreadListParams,
    ThreadSortKey, ThreadSourceKind, ToolRequestUserInputQuestion,
};
use codex_protocol::protocol::ReviewDecision;

// ---------------------------------------------------------------------------
// Public request types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexExecRequest {
    pub command: Vec<String>,
    pub process_id: Option<String>,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub stream_stdin: bool,
    #[serde(default)]
    pub stream_stdout_stderr: bool,
    pub output_bytes_cap: Option<usize>,
    #[serde(default)]
    pub disable_output_cap: bool,
    #[serde(default)]
    pub disable_timeout: bool,
    pub timeout_ms: Option<i64>,
    pub cwd: Option<PathBuf>,
    pub env: Option<HashMap<String, Option<String>>>,
    pub sandbox_policy: Option<serde_json::Value>,
    pub permission_profile: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadGoalRefRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadGoalSetRequest {
    pub thread_id: String,
    pub text: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub token_budget: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadRollbackRequest {
    pub thread_id: String,
    pub num_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnSteerRequest {
    pub thread_id: String,
    pub expected_turn_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnInterruptRequest {
    pub thread_id: String,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPluginRefRequest {
    pub marketplace_path: Option<PathBuf>,
    pub remote_marketplace_name: Option<String>,
    pub plugin_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexServerRequestResolution {
    #[serde(default)]
    pub allow_all: bool,
    #[serde(default)]
    pub response: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadListRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub sort_key: Option<String>,
    pub sort_direction: Option<String>,
    pub model_providers: Option<Vec<String>>,
    pub source_kinds: Option<Vec<String>>,
    pub archived: Option<bool>,
    pub cwd: Option<serde_json::Value>,
    #[serde(default)]
    pub use_state_db_only: bool,
    pub search_term: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexFeedbackRequest {
    pub classification: String,
    pub reason: Option<String>,
    pub thread_id: Option<String>,
    #[serde(default)]
    pub include_logs: bool,
    pub extra_log_files: Option<Vec<PathBuf>>,
    pub tags: Option<std::collections::BTreeMap<String, String>>,
}

impl From<CodexFeedbackRequest> for FeedbackUploadParams {
    fn from(value: CodexFeedbackRequest) -> Self {
        Self {
            classification: value.classification,
            reason: value.reason,
            thread_id: value.thread_id,
            include_logs: value.include_logs,
            extra_log_files: value.extra_log_files,
            tags: value.tags,
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl TryFrom<CodexExecRequest> for CommandExecParams {
    type Error = anyhow::Error;

    fn try_from(value: CodexExecRequest) -> Result<Self, Self::Error> {
        let sandbox_policy = value
            .sandbox_policy
            .map(serde_json::from_value)
            .transpose()
            .context("invalid Codex command/exec sandbox policy")?;
        let permission_profile = value
            .permission_profile
            .map(serde_json::from_value)
            .transpose()
            .context("invalid Codex command/exec permission profile")?;

        if sandbox_policy.is_some() && permission_profile.is_some() {
            return Err(anyhow::anyhow!(
                "Codex command/exec cannot combine sandboxPolicy and permissionProfile"
            ));
        }

        Ok(Self {
            command: value.command,
            process_id: value.process_id,
            tty: value.tty,
            stream_stdin: value.stream_stdin,
            stream_stdout_stderr: value.stream_stdout_stderr,
            output_bytes_cap: value.output_bytes_cap,
            disable_output_cap: value.disable_output_cap,
            disable_timeout: value.disable_timeout,
            timeout_ms: value.timeout_ms,
            cwd: value.cwd,
            env: value.env,
            size: None,
            sandbox_policy,
            permission_profile,
        })
    }
}

// ---------------------------------------------------------------------------
// Pending server request kinds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum PendingServerRequestKind {
    CommandExecution,
    FileChange,
    ApplyPatch,
    ExecCommand,
    Permissions(RequestPermissionProfile),
    McpElicitation,
    ToolUserInput(Vec<ToolRequestUserInputQuestion>),
    #[allow(dead_code)]
    DynamicTool {
        call_id: String,
        namespace: Option<String>,
        tool: String,
        arguments: serde_json::Value,
    },
    #[allow(dead_code)]
    ChatgptAuthRefresh {
        reason: String,
        previous_account_id: Option<String>,
    },
}

impl PendingServerRequestKind {
    pub(crate) fn from_request(request: &ServerRequest) -> Self {
        match request {
            ServerRequest::CommandExecutionRequestApproval { .. } => Self::CommandExecution,
            ServerRequest::FileChangeRequestApproval { .. } => Self::FileChange,
            ServerRequest::ApplyPatchApproval { .. } => Self::ApplyPatch,
            ServerRequest::ExecCommandApproval { .. } => Self::ExecCommand,
            ServerRequest::PermissionsRequestApproval { params, .. } => {
                Self::Permissions(params.permissions.clone())
            }
            ServerRequest::McpServerElicitationRequest { .. } => Self::McpElicitation,
            ServerRequest::ToolRequestUserInput { params, .. } => {
                Self::ToolUserInput(params.questions.clone())
            }
            ServerRequest::DynamicToolCall { params, .. } => Self::DynamicTool {
                call_id: params.call_id.clone(),
                namespace: params.namespace.clone(),
                tool: params.tool.clone(),
                arguments: params.arguments.clone(),
            },
            ServerRequest::ChatgptAuthTokensRefresh { params, .. } => Self::ChatgptAuthRefresh {
                reason: format!("{:?}", params.reason),
                previous_account_id: params.previous_account_id.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Event pump state
// ---------------------------------------------------------------------------

/// Shared state between the adapter and the background event pump.
///
/// The pump writes events to whichever sender is currently installed.
/// `send_message()` swaps in a new sender for each turn.
pub(crate) struct EventPumpState {
    /// The current event sender, swapped by `send_message()`.
    pub(crate) current_tx: Option<mpsc::Sender<UnifiedAgentEvent>>,
    /// Server request ids and their exact official response shape.
    pub(crate) pending_server_requests: HashMap<String, PendingServerRequestKind>,
    /// Last known token usage from `ThreadTokenUsageUpdated`, used to populate
    /// `Completed` events with real numbers instead of zeros.
    pub(crate) last_usage: Option<rc_agent_protocol::events::UsageInfo>,
}

impl EventPumpState {
    pub(crate) fn new() -> Self {
        Self {
            current_tx: None,
            pending_server_requests: HashMap::new(),
            last_usage: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Permission resolution helpers
// ---------------------------------------------------------------------------

pub(crate) fn typed_server_request_response(
    kind: PendingServerRequestKind,
    decision: PermissionDecision,
    resolution: CodexServerRequestResolution,
) -> anyhow::Result<Option<serde_json::Value>> {
    let allow = matches!(
        decision,
        PermissionDecision::Allow | PermissionDecision::AllowAll
    );
    let allow_all = matches!(decision, PermissionDecision::AllowAll) || resolution.allow_all;

    if let Some(ref response) = resolution.response
        && allow
    {
        return Ok(Some(response.clone()));
    }

    if !allow
        && !matches!(
            &kind,
            PendingServerRequestKind::CommandExecution
                | PendingServerRequestKind::FileChange
                | PendingServerRequestKind::ApplyPatch
                | PendingServerRequestKind::ExecCommand
                | PendingServerRequestKind::McpElicitation
        )
    {
        return Ok(None);
    }

    let value = match kind {
        PendingServerRequestKind::CommandExecution => serde_json::to_value(
            codex_app_server_protocol::CommandExecutionRequestApprovalResponse {
                decision: if allow_all {
                    codex_app_server_protocol::CommandExecutionApprovalDecision::AcceptForSession
                } else if allow {
                    codex_app_server_protocol::CommandExecutionApprovalDecision::Accept
                } else {
                    codex_app_server_protocol::CommandExecutionApprovalDecision::Decline
                },
            },
        )?,
        PendingServerRequestKind::FileChange => serde_json::to_value(
            codex_app_server_protocol::FileChangeRequestApprovalResponse {
                decision: if allow_all {
                    FileChangeApprovalDecision::AcceptForSession
                } else if allow {
                    FileChangeApprovalDecision::Accept
                } else {
                    FileChangeApprovalDecision::Decline
                },
            },
        )?,
        PendingServerRequestKind::ApplyPatch => {
            serde_json::to_value(codex_app_server_protocol::ApplyPatchApprovalResponse {
                decision: if allow {
                    ReviewDecision::Approved
                } else {
                    ReviewDecision::Denied
                },
            })?
        }
        PendingServerRequestKind::ExecCommand => {
            serde_json::to_value(codex_app_server_protocol::ExecCommandApprovalResponse {
                decision: if allow_all {
                    ReviewDecision::ApprovedForSession
                } else if allow {
                    ReviewDecision::Approved
                } else {
                    ReviewDecision::Denied
                },
            })?
        }
        PendingServerRequestKind::Permissions(requested_permissions) => serde_json::to_value(
            codex_app_server_protocol::PermissionsRequestApprovalResponse {
                permissions: requested_permissions_to_granted_profile(requested_permissions),
                scope: if allow_all {
                    PermissionGrantScope::Session
                } else {
                    PermissionGrantScope::Turn
                },
                strict_auto_review: None,
            },
        )?,
        PendingServerRequestKind::McpElicitation => {
            serde_json::to_value(McpServerElicitationRequestResponse {
                action: if allow {
                    McpServerElicitationAction::Accept
                } else {
                    McpServerElicitationAction::Decline
                },
                content: None,
                meta: None,
            })?
        }
        PendingServerRequestKind::ToolUserInput(questions) => {
            serde_json::to_value(codex_app_server_protocol::ToolRequestUserInputResponse {
                answers: default_tool_user_input_answers(&questions),
            })?
        }
        PendingServerRequestKind::DynamicTool { .. } => {
            serde_json::to_value(DynamicToolCallResponse {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: if allow {
                        "No dynamic client tool handler is registered.".to_string()
                    } else {
                        "Dynamic tool call denied by user.".to_string()
                    },
                }],
                success: false,
            })?
        }
        PendingServerRequestKind::ChatgptAuthRefresh { .. } => {
            if let Some(response) = resolution.response {
                let _refresh_response: codex_app_server_protocol::ChatgptAuthTokensRefreshResponse =
                    serde_json::from_value(response.clone())
                        .with_context(|| "Invalid ChatgptAuthTokensRefreshResponse in resolution")?;
                return Ok(Some(response));
            }
            return Ok(None);
        }
    };

    Ok(Some(value))
}

fn requested_permissions_to_granted_profile(
    value: RequestPermissionProfile,
) -> GrantedPermissionProfile {
    GrantedPermissionProfile {
        network: value.network,
        file_system: value.file_system,
    }
}

fn default_tool_user_input_answers(
    questions: &[ToolRequestUserInputQuestion],
) -> HashMap<String, codex_app_server_protocol::ToolRequestUserInputAnswer> {
    questions
        .iter()
        .map(|question| {
            let answers = question
                .options
                .as_deref()
                .and_then(|options| options.first())
                .map(|option| vec![option.label.clone()])
                .unwrap_or_default();
            (
                question.id.clone(),
                codex_app_server_protocol::ToolRequestUserInputAnswer { answers },
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Thread list params conversion helpers
// ---------------------------------------------------------------------------

pub(crate) fn thread_list_params_from_request(request: CodexThreadListRequest) -> ThreadListParams {
    ThreadListParams {
        cursor: request.cursor,
        limit: request.limit,
        sort_key: request.sort_key.as_deref().and_then(parse_thread_sort_key),
        sort_direction: request
            .sort_direction
            .as_deref()
            .and_then(parse_sort_direction),
        model_providers: request.model_providers,
        source_kinds: request.source_kinds.map(|values| {
            values
                .into_iter()
                .filter_map(|value| parse_thread_source_kind(&value))
                .collect()
        }),
        archived: request.archived,
        cwd: request.cwd.and_then(parse_thread_cwd_filter),
        use_state_db_only: request.use_state_db_only,
        search_term: request.search_term,
    }
}

fn parse_thread_sort_key(value: &str) -> Option<ThreadSortKey> {
    match value.trim().to_ascii_lowercase().as_str() {
        "created_at" | "createdat" | "created-at" => Some(ThreadSortKey::CreatedAt),
        "updated_at" | "updatedat" | "updated-at" => Some(ThreadSortKey::UpdatedAt),
        _ => None,
    }
}

fn parse_sort_direction(value: &str) -> Option<SortDirection> {
    match value.trim().to_ascii_lowercase().as_str() {
        "asc" | "ascending" => Some(SortDirection::Asc),
        "desc" | "descending" => Some(SortDirection::Desc),
        _ => None,
    }
}

fn parse_thread_source_kind(value: &str) -> Option<ThreadSourceKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cli" => Some(ThreadSourceKind::Cli),
        "vscode" | "vs_code" | "vs-code" => Some(ThreadSourceKind::VsCode),
        "exec" => Some(ThreadSourceKind::Exec),
        "appserver" | "app-server" | "app_server" => Some(ThreadSourceKind::AppServer),
        "subagent" | "sub-agent" | "sub_agent" => Some(ThreadSourceKind::SubAgent),
        "unknown" => Some(ThreadSourceKind::Unknown),
        _ => None,
    }
}

fn parse_thread_cwd_filter(value: serde_json::Value) -> Option<ThreadListCwdFilter> {
    match value {
        serde_json::Value::String(path) => Some(ThreadListCwdFilter::One(path)),
        serde_json::Value::Array(values) => Some(ThreadListCwdFilter::Many(
            values
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect(),
        )),
        _ => None,
    }
}

pub(crate) fn request_id_to_string(id: &RequestId) -> String {
    match id {
        RequestId::String(s) => s.clone(),
        RequestId::Integer(n) => n.to_string(),
    }
}
