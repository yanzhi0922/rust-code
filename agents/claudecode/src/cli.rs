use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use claude_control_plane::SessionState as RemoteSessionState;
use claude_core::{InputFormat, OutputFormat, PermissionMode, ProviderProtocol};
use claude_runner::{ApprovalDecision, RunnerSessionCommandResponse};
use uuid::Uuid;

use crate::hooks::HooksCommand;

#[derive(Parser, Debug)]
#[command(
    name = "remote-code",
    version,
    about = "Remote Code Rust CLI/runtime shell"
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    #[arg(short = 'p', long = "print")]
    pub print_mode: bool,

    #[arg(long, default_value_t = InputFormat::Text)]
    pub input_format: InputFormat,

    #[arg(long, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,

    #[arg(long, env = "REMOTE_CODE_PERMISSION_MODE", default_value_t = PermissionMode::Default)]
    pub permission_mode: PermissionMode,

    #[arg(long)]
    pub cwd: Option<PathBuf>,

    #[arg(long, env = "REMOTE_CODE_PROFILE_DIR")]
    pub profile_dir: Option<PathBuf>,

    #[arg(long)]
    pub session_id: Option<Uuid>,

    #[arg(long = "continue")]
    pub r#continue: bool,

    #[arg(short = 'r', long = "resume", num_args = 0..=1, value_name = "SESSION_ID")]
    pub resume: Option<Option<String>>,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    #[arg(long = "system-prompt-file", hide = true)]
    pub system_prompt_file: Option<PathBuf>,

    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,

    #[arg(long = "append-system-prompt-file", hide = true)]
    pub append_system_prompt_file: Option<PathBuf>,

    #[arg(long = "settings")]
    pub settings_files: Vec<PathBuf>,

    #[arg(long = "setting-sources", value_delimiter = ',')]
    pub setting_sources: Vec<SettingSourceArgValue>,

    #[arg(long = "show-setting-sources")]
    pub show_setting_sources: bool,

    #[arg(
        long,
        alias = "allowedTools",
        value_delimiter = ',',
        num_args = 1..,
        value_name = "TOOLS"
    )]
    pub allowed_tools: Vec<String>,

    #[arg(
        long,
        alias = "disallowedTools",
        value_delimiter = ',',
        num_args = 1..,
        value_name = "TOOLS"
    )]
    pub disallowed_tools: Vec<String>,

    #[arg(long = "json-schema")]
    pub json_schema: Option<String>,

    #[arg(long = "mcp-config", num_args = 1.., value_name = "CONFIG")]
    pub mcp_config: Vec<String>,

    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,

    #[arg(long = "tools", value_delimiter = ',', num_args = 1.., value_name = "TOOLS")]
    pub tools: Vec<String>,

    #[arg(long = "effort")]
    pub effort: Option<String>,

    #[arg(long = "fallback-model", alias = "fallbackModel")]
    pub fallback_model: Option<String>,

    #[arg(long = "output-style", alias = "outputStyle")]
    pub output_style: Option<String>,

    #[arg(long = "language")]
    pub language: Option<String>,

    #[arg(long = "brief", action = clap::ArgAction::SetTrue)]
    pub brief: bool,

    #[arg(long = "no-brief", action = clap::ArgAction::SetTrue)]
    pub no_brief: bool,

    #[arg(long = "proactive", action = clap::ArgAction::SetTrue)]
    pub proactive: bool,

    #[arg(long = "no-proactive", action = clap::ArgAction::SetTrue)]
    pub no_proactive: bool,

    #[arg(
        long = "dangerously-skip-permissions",
        alias = "dangerouslySkipPermissions"
    )]
    pub dangerously_skip_permissions: bool,

    #[arg(
        long = "allow-dangerously-skip-permissions",
        alias = "allowDangerouslySkipPermissions"
    )]
    pub allow_dangerously_skip_permissions: bool,

    #[arg(long = "permission-prompt-tool", alias = "permissionPromptTool")]
    pub permission_prompt_tool: Option<String>,

    #[arg(long = "sdk-url", alias = "sdkUrl", hide = true)]
    pub sdk_url: Option<String>,

    #[arg(long = "enable-auth-status", alias = "enableAuthStatus", hide = true)]
    pub enable_auth_status: bool,

    #[arg(long = "worktree", hide = true)]
    pub worktree: Option<String>,

    #[arg(long = "worktree-base-ref", alias = "worktreeBaseRef", hide = true)]
    pub worktree_base_ref: Option<String>,

    #[arg(long = "include-hook-events", alias = "includeHookEvents")]
    pub include_hook_events: bool,

    #[arg(long = "bare")]
    pub bare: bool,

    #[arg(long)]
    pub provider: Option<String>,

    #[arg(long)]
    pub base_url: Option<String>,

    #[arg(long)]
    pub api_key: Option<String>,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long)]
    pub protocol: Option<ProviderProtocol>,

    #[arg(long, default_value_t = 12)]
    pub max_turns: usize,

    #[arg(short, long)]
    pub verbose: bool,

    #[arg(long)]
    pub replay_user_messages: bool,

    #[arg(long)]
    pub include_partial_messages: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,

    pub prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Doctor(DoctorArgs),
    Status(StatusArgs),
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: Option<SessionsCommand>,
    },
    Review(ReviewArgs),
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    Tasks {
        #[command(subcommand)]
        command: Option<TasksCommand>,
    },
    Resume(ResumeArgs),
    Export(ExportArgs),
    Tui,
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    /// Connect to a remote host via SSH and run remote-code.
    Ssh(SshArgs),
    /// Check for updates or self-update.
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
}

/// Subcommands for the update command.
#[derive(Subcommand, Debug)]
pub enum UpdateCommand {
    /// Check if a newer version is available.
    Check,
    /// Download and install the latest version.
    Run,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingSourceArgValue {
    User,
    Project,
    Local,
}

#[derive(Subcommand, Debug)]
pub enum SessionsCommand {
    List,
    Show(ShowArgs),
    Stats(SessionsStatsArgs),
}

#[derive(Subcommand, Debug)]
pub enum TasksCommand {
    List(TasksListArgs),
    Show(TaskShowArgs),
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCommand {
    List(WorktreeListArgs),
    Add(WorktreeAddArgs),
    Remove(WorktreeRemoveArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    Meta(RemoteMetaArgs),
    Auth {
        #[command(subcommand)]
        command: RemoteAuthCommand,
    },
    Runners {
        #[command(subcommand)]
        command: RemoteRunnersCommand,
    },
    Artifacts {
        #[command(subcommand)]
        command: RemoteArtifactsCommand,
    },
    Approvals {
        #[command(subcommand)]
        command: RemoteApprovalsCommand,
    },
    Events(RemoteEventsArgs),
    Sessions {
        #[command(subcommand)]
        command: RemoteSessionsCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum RemoteRunnersCommand {
    List(RemoteRunnersListArgs),
    Show(RemoteRunnerShowArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteAuthCommand {
    Devices(RemoteDevicesListArgs),
    Bootstrap(RemoteBootstrapArgs),
    PairOffer(RemotePairingOfferArgs),
    PairAccept(RemotePairingAcceptArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteArtifactsCommand {
    List(RemoteArtifactsListArgs),
    Show(RemoteArtifactShowArgs),
    Download(RemoteArtifactDownloadArgs),
    Upload(RemoteArtifactUploadArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteApprovalsCommand {
    List(RemoteApprovalsListArgs),
    Create(RemoteApprovalCreateArgs),
    Show(RemoteApprovalShowArgs),
    Respond(RemoteApprovalRespondArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteSessionsCommand {
    List(RemoteSessionsListArgs),
    Show(RemoteSessionShowArgs),
    Create(RemoteSessionCreateArgs),
    Follow(RemoteSessionFollowArgs),
    State(RemoteSessionStateArgs),
    Prompt(RemoteSessionPromptArgs),
    Interrupt(RemoteSessionInterruptArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct DoctorArgs {
    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub probe_network: bool,

    #[arg(long)]
    pub probe_provider: bool,

    #[arg(long)]
    pub probe_mcp: bool,

    #[arg(long)]
    pub include_env_providers: bool,
}

#[derive(Args, Debug)]
pub struct ResumeArgs {
    pub session_id: Uuid,
    pub prompt: Vec<String>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct StatusArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    pub session_id: Uuid,
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
    pub format: ExportFormat,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    pub session_id: Uuid,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SessionsStatsArgs {
    pub session_id: Option<Uuid>,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct TasksListArgs {
    #[arg(long)]
    pub session_id: Option<Uuid>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct TaskShowArgs {
    pub task_id: String,
    #[arg(long)]
    pub session_id: Option<Uuid>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub output: bool,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorktreeListArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorktreeAddArgs {
    pub name: Option<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorktreeRemoveArgs {
    pub action: String,

    #[arg(long)]
    pub discard_changes: bool,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone)]
pub struct RemoteTargetArgs {
    #[arg(long, env = "REMOTE_CODE_CONTROL_PLANE_URL")]
    pub control_plane_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemoteMetaArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteRunnersListArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteRunnerShowArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub runner_id: String,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteDevicesListArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteBootstrapArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub bootstrap_secret: String,

    #[arg(long)]
    pub device_name: String,

    #[arg(long, value_enum, default_value_t = RemoteDeviceKindValue::Cli)]
    pub device_kind: RemoteDeviceKindValue,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemotePairingOfferArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub device_name: String,

    #[arg(long, value_enum, default_value_t = RemoteDeviceKindValue::Browser)]
    pub device_kind: RemoteDeviceKindValue,

    #[arg(long)]
    pub expires_in_secs: Option<u64>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemotePairingAcceptArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub offer_id: Uuid,

    #[arg(long)]
    pub pairing_secret: String,

    #[arg(long)]
    pub device_name: Option<String>,

    #[arg(long, value_enum)]
    pub device_kind: Option<RemoteDeviceKindValue>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionsListArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub runner_id: Option<String>,

    #[arg(long)]
    pub workspace_id: Option<String>,

    #[arg(long, value_enum)]
    pub state: Option<RemoteSessionStateValue>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionShowArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub session_id: Uuid,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionCreateArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub workspace_id: String,

    #[arg(long)]
    pub preferred_runner_id: Option<String>,

    #[arg(long = "meta")]
    pub metadata: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum RemoteSessionStateValue {
    Pending,
    Assigned,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

impl From<RemoteSessionStateValue> for RemoteSessionState {
    fn from(value: RemoteSessionStateValue) -> Self {
        match value {
            RemoteSessionStateValue::Pending => RemoteSessionState::Pending,
            RemoteSessionStateValue::Assigned => RemoteSessionState::Assigned,
            RemoteSessionStateValue::Running => RemoteSessionState::Running,
            RemoteSessionStateValue::WaitingApproval => RemoteSessionState::WaitingApproval,
            RemoteSessionStateValue::Completed => RemoteSessionState::Completed,
            RemoteSessionStateValue::Failed => RemoteSessionState::Failed,
            RemoteSessionStateValue::Cancelled => RemoteSessionState::Cancelled,
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum RemoteDeviceKindValue {
    Runner,
    Browser,
    Cli,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum RemoteEventKindValue {
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
}

impl RemoteEventKindValue {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunnerRegistered => "runner_registered",
            Self::RunnerHeartbeat => "runner_heartbeat",
            Self::SessionCreated => "session_created",
            Self::SessionStateChanged => "session_state_changed",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalResolved => "approval_resolved",
            Self::ArtifactCreated => "artifact_created",
            Self::MessageDelta => "message_delta",
            Self::MessageCommitted => "message_committed",
            Self::ToolStarted => "tool_started",
            Self::ToolProgress => "tool_progress",
            Self::ToolFinished => "tool_finished",
            Self::ArtifactManifest => "artifact_manifest",
            Self::RuntimeError => "runtime_error",
            Self::DaemonPresenceChanged => "daemon_presence_changed",
        }
    }
}

#[derive(Args, Debug)]
pub struct RemoteSessionStateArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub session_id: Uuid,

    #[arg(long, value_enum)]
    pub state: RemoteSessionStateValue,

    #[arg(long = "meta")]
    pub metadata: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionPromptArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub session_id: Uuid,

    pub prompt: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionInterruptArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub session_id: Uuid,

    #[arg(long)]
    pub json: bool,
}

pub type RemoteSessionCommandResponseValue = RunnerSessionCommandResponse;

#[derive(Args, Debug)]
pub struct RemoteArtifactsListArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub session_id: Option<Uuid>,

    #[arg(long)]
    pub runner_id: Option<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteArtifactShowArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub artifact_id: Uuid,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteArtifactDownloadArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub artifact_id: Uuid,

    #[arg(long)]
    pub output: Option<PathBuf>,

    #[arg(long)]
    pub overwrite: bool,

    #[arg(long)]
    pub stdout: bool,
}

#[derive(Args, Debug)]
pub struct RemoteArtifactUploadArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub session_id: Uuid,

    #[arg(long)]
    pub file: PathBuf,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long)]
    pub file_name: Option<String>,

    #[arg(long)]
    pub media_type: Option<String>,

    #[arg(long = "meta")]
    pub metadata: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteApprovalsListArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub session_id: Option<Uuid>,

    #[arg(long)]
    pub runner_id: Option<String>,

    #[arg(long)]
    pub after: Option<u64>,

    #[arg(long)]
    pub follow: bool,

    #[arg(long, default_value_t = 2)]
    pub reconnect_delay_secs: u64,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteApprovalShowArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub approval_id: Uuid,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteApprovalCreateArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub session_id: Uuid,

    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub description: String,

    #[arg(long = "meta")]
    pub metadata: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteApprovalDecision {
    Approved,
    Denied,
    Cancelled,
}

impl From<RemoteApprovalDecision> for ApprovalDecision {
    fn from(value: RemoteApprovalDecision) -> Self {
        match value {
            RemoteApprovalDecision::Approved => ApprovalDecision::Approved,
            RemoteApprovalDecision::Denied => ApprovalDecision::Denied,
            RemoteApprovalDecision::Cancelled => ApprovalDecision::Cancelled,
        }
    }
}

#[derive(Args, Debug)]
pub struct RemoteApprovalRespondArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub approval_id: Uuid,

    #[arg(long, value_enum)]
    pub decision: RemoteApprovalDecision,

    #[arg(long)]
    pub responder: Option<String>,

    #[arg(long)]
    pub note: Option<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteEventsArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    #[arg(long)]
    pub session_id: Option<Uuid>,

    #[arg(long)]
    pub runner_id: Option<String>,

    #[arg(long, value_enum)]
    pub kind: Option<RemoteEventKindValue>,

    #[arg(long)]
    pub after: Option<u64>,

    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    #[arg(long)]
    pub follow: bool,

    #[arg(long, default_value_t = 2)]
    pub reconnect_delay_secs: u64,

    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum MigrateCommand {
    Import {
        #[arg(long)]
        source: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentsCommand {
    Plan(AgentsPlanArgs),
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    List(McpListArgs),
    Get(McpGetArgs),
    Add(McpAddArgs),
    Remove(McpRemoveArgs),
    Enable(McpToggleArgs),
    Disable(McpToggleArgs),
    Reset(McpResetArgs),
    Serve(McpServeArgs),
    Call(McpCallArgs),
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    List(PluginsListArgs),
    Inspect(PluginsInspectArgs),
    Invoke(PluginsInvokeArgs),
    Validate(PluginsValidateArgs),
    Install(PluginsInstallArgs),
    Remove(PluginsRemoveArgs),
    Enable(PluginsToggleArgs),
    Disable(PluginsToggleArgs),
    Update(PluginsUpdateArgs),
}

#[derive(Subcommand, Debug)]
pub enum SkillsCommand {
    List(SkillsListArgs),
    Show(SkillsShowArgs),
    Lock(SkillsLockArgs),
    Index(SkillsIndexArgs),
}

#[derive(Args, Debug)]
pub struct AgentsPlanArgs {
    #[arg(long, default_value = "codex-lead")]
    pub lead: String,

    #[arg(long)]
    pub objective: String,

    #[arg(long = "agent")]
    pub agents: Vec<String>,

    #[arg(long = "task")]
    pub tasks: Vec<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct McpListArgs {
    #[arg(long)]
    pub connect: bool,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "server")]
    pub servers: Vec<String>,

    #[arg(long)]
    pub include_disabled: bool,

    #[arg(long = "config")]
    pub config_paths: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct McpGetArgs {
    #[arg(long)]
    pub server: String,

    #[arg(long)]
    pub connect: bool,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "include-disabled")]
    pub include_disabled: bool,

    #[arg(long = "config")]
    pub config_paths: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct McpAddArgs {
    pub name: String,

    #[arg(long)]
    pub command: Option<String>,

    #[arg(long)]
    pub url: Option<String>,

    #[arg(long = "arg")]
    pub args: Vec<String>,

    #[arg(long)]
    pub cwd: Option<PathBuf>,

    #[arg(long = "env")]
    pub env: Vec<String>,

    #[arg(long)]
    pub disabled: bool,

    #[arg(long)]
    pub startup_timeout_secs: Option<u64>,

    #[arg(long)]
    pub request_timeout_secs: Option<u64>,

    #[arg(long = "meta")]
    pub metadata: Vec<String>,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "config")]
    pub config_path: Option<PathBuf>,

    #[arg(long)]
    pub project: bool,
}

#[derive(Args, Debug)]
pub struct McpRemoveArgs {
    pub name: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "config")]
    pub config_path: Option<PathBuf>,

    #[arg(long)]
    pub project: bool,

    #[arg(long)]
    pub if_exists: bool,
}

#[derive(Args, Debug)]
pub struct McpToggleArgs {
    pub name: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "config")]
    pub config_path: Option<PathBuf>,

    #[arg(long)]
    pub project: bool,

    #[arg(long)]
    pub if_exists: bool,
}

#[derive(Args, Debug)]
pub struct McpResetArgs {
    #[arg(long)]
    pub json: bool,

    #[arg(long = "config")]
    pub config_path: Option<PathBuf>,

    #[arg(long)]
    pub project: bool,

    #[arg(long)]
    pub if_exists: bool,
}

#[derive(Args, Debug)]
pub struct McpServeArgs {
    #[arg(long)]
    pub server: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "include-disabled")]
    pub include_disabled: bool,

    #[arg(long = "config")]
    pub config_paths: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct McpCallArgs {
    #[arg(long)]
    pub server: String,

    #[arg(long)]
    pub tool: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "include-disabled")]
    pub include_disabled: bool,

    #[arg(long = "arg")]
    pub args: Vec<String>,

    #[arg(long = "args-json")]
    pub args_json: Option<String>,

    #[arg(long = "config")]
    pub config_paths: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PluginsListArgs {
    #[arg(long)]
    pub connect: bool,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "plugin")]
    pub plugins: Vec<String>,

    #[arg(long = "plugins-dir")]
    pub plugin_roots: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PluginsInspectArgs {
    #[arg(long)]
    pub plugin: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "plugins-dir")]
    pub plugin_roots: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PluginsInvokeArgs {
    #[arg(long)]
    pub plugin: String,

    #[arg(long)]
    pub action: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "arg")]
    pub args: Vec<String>,

    #[arg(long = "input-json")]
    pub input_json: Option<String>,

    #[arg(long = "plugins-dir")]
    pub plugin_roots: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PluginsValidateArgs {
    #[arg(long)]
    pub plugin: Option<String>,

    #[arg(long)]
    pub path: Option<PathBuf>,

    #[arg(long)]
    pub json: bool,

    #[arg(long = "plugins-dir")]
    pub plugin_roots: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PluginsInstallArgs {
    pub path: PathBuf,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct PluginsRemoveArgs {
    pub plugin: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub if_exists: bool,
}

#[derive(Args, Debug)]
pub struct PluginsToggleArgs {
    pub plugin: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub if_exists: bool,
}

#[derive(Args, Debug)]
pub struct PluginsUpdateArgs {
    pub path: PathBuf,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteSessionFollowArgs {
    #[command(flatten)]
    pub target: RemoteTargetArgs,

    pub session_id: Uuid,

    #[arg(long)]
    pub after: Option<u64>,

    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    #[arg(long, default_value_t = 2)]
    pub reconnect_delay_secs: u64,

    #[arg(long)]
    pub stop_on_terminal: bool,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SkillsListArgs {
    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub no_plugins: bool,
}

#[derive(Args, Debug)]
pub struct SkillsShowArgs {
    pub skill: String,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub include_instructions: bool,

    #[arg(long)]
    pub no_plugins: bool,
}

#[derive(Args, Debug)]
pub struct SkillsLockArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SkillsIndexArgs {
    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub no_plugins: bool,

    #[arg(long)]
    pub write_cache: bool,

    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SshArgs {
    /// Remote host to connect to (e.g. user@host or just host).
    #[arg(long)]
    pub host: String,

    /// SSH user name (overrides user in host if both given).
    #[arg(long)]
    pub user: Option<String>,

    /// SSH port (default 22).
    #[arg(long, default_value_t = 22)]
    pub port: u16,

    /// Remote command to execute on the host.
    #[arg(long)]
    pub command: Option<String>,

    /// Identity file (SSH private key path).
    #[arg(short = 'i', long)]
    pub identity: Option<PathBuf>,

    /// SSH config file path.
    #[arg(short = 'F', long)]
    pub config: Option<PathBuf>,

    /// Enable verbose SSH output (-v).
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// Enable SSH agent forwarding (-A).
    #[arg(short = 'A', long)]
    pub forward_agent: bool,

    /// Local port forwarding (e.g. "8080:localhost:80").
    #[arg(short = 'L', long)]
    pub local_forward: Vec<String>,

    /// Remote port forwarding (e.g. "9090:localhost:8080").
    #[arg(short = 'R', long)]
    pub remote_forward: Vec<String>,

    /// Connection timeout in seconds.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,

    /// Extra arguments to pass to remote-code on the remote host.
    #[arg(long)]
    pub remote_args: Vec<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ExportFormat {
    Ndjson,
    Json,
}

#[cfg(test)]
mod tests {
    use super::{Cli, SettingSourceArgValue};
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parses_setting_source_filters_and_show_flag_independently() {
        let cli = Cli::try_parse_from([
            "remote-code",
            "--setting-sources",
            "user,local",
            "--show-setting-sources",
            "status",
        ])
        .expect("cli should parse");

        assert_eq!(
            cli.setting_sources,
            vec![SettingSourceArgValue::User, SettingSourceArgValue::Local]
        );
        assert!(cli.show_setting_sources);
    }

    #[test]
    fn parses_system_prompt_file_flags() {
        let cli = Cli::try_parse_from([
            "remote-code",
            "--system-prompt-file",
            "system.txt",
            "--append-system-prompt-file",
            "append.txt",
            "status",
        ])
        .expect("cli should parse");

        assert_eq!(cli.system_prompt_file, Some(PathBuf::from("system.txt")));
        assert_eq!(
            cli.append_system_prompt_file,
            Some(PathBuf::from("append.txt"))
        );
    }

    #[test]
    fn parses_reference_style_tool_filter_aliases_and_mcp_flags() {
        let cli = Cli::try_parse_from([
            "remote-code",
            "--allowedTools",
            "Bash(git:*)",
            "Edit,Read",
            "--disallowedTools",
            "WebFetch",
            "--json-schema",
            r#"{"type":"object"}"#,
            "--mcp-config",
            "mcp.json",
            r#"{"mcpServers":{"demo":{"command":"node"}}}"#,
            "--strict-mcp-config",
            "-p",
            "--output-format",
            "json",
            "hello",
        ])
        .expect("cli should parse");

        assert_eq!(cli.allowed_tools, vec!["Bash(git:*)", "Edit", "Read"]);
        assert_eq!(cli.disallowed_tools, vec!["WebFetch"]);
        assert_eq!(cli.json_schema.as_deref(), Some(r#"{"type":"object"}"#));
        assert_eq!(cli.mcp_config.len(), 2);
        assert!(cli.strict_mcp_config);
    }

    #[test]
    fn parses_reference_runtime_knobs() {
        let cli = Cli::try_parse_from([
            "remote-code",
            "-p",
            "--tools",
            "Read,Edit",
            "--tools",
            "Bash(git:*)",
            "--effort",
            "high",
            "--fallback-model",
            "minimax-m2.7",
            "--output-style",
            "concise",
            "--language",
            "zh-CN",
            "--brief",
            "--no-proactive",
            "--dangerously-skip-permissions",
            "--permission-prompt-tool",
            "mcp__auth__ask",
            "--include-hook-events",
            "--bare",
            "hello",
        ])
        .expect("cli should parse");

        assert_eq!(cli.tools, vec!["Read", "Edit", "Bash(git:*)"]);
        assert_eq!(cli.effort.as_deref(), Some("high"));
        assert_eq!(cli.fallback_model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(cli.output_style.as_deref(), Some("concise"));
        assert_eq!(cli.language.as_deref(), Some("zh-CN"));
        assert!(cli.brief);
        assert!(cli.no_proactive);
        assert!(cli.dangerously_skip_permissions);
        assert_eq!(
            cli.permission_prompt_tool.as_deref(),
            Some("mcp__auth__ask")
        );
        assert!(cli.include_hook_events);
        assert!(cli.bare);
    }
}
