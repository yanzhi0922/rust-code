//! Core type definitions for the remote-code-rust workspace.
//!
//! This crate defines the shared domain types used across all other crates:
//! permission modes, provider protocols, conversation entries, tool calls,
//! usage summaries, hook definitions, and session events.

pub mod app_state;
pub mod exit_reasons;
pub mod hook_executor;
pub mod hook_matcher;
pub mod hook_registry;
pub mod hook_types;
pub mod hooks;
pub mod ids;
pub mod message;
pub mod message_types;
pub mod model_cost;
pub mod permission_types;
pub mod state;
pub mod subprocess_env;
pub mod task_stack;
pub mod usage;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

pub use app_state::{
    AppStateManager, SharedStateManager, StateManagerExt, StateSnapshot, StateUpdate,
};
pub use exit_reasons::{ExitReason, ExitReasonTracker, ExitRecord};
pub use hook_executor::{
    HookBatchResult, HookExecutor, HookOutcome, format_blocking_message, is_url_safe_for_hook,
};
pub use hook_matcher::{
    MatchedHooks, deduplicate_hooks, filter_hooks_by_managed, filter_hooks_by_trust,
    is_hook_event as is_hook_event_match, match_hooks, match_tool_name, parse_hook_event,
};
pub use hook_registry::{HookRegistry, HooksConfigSnapshot};
pub use hook_types::{
    AggregatedHookResult, HookCallback, HookCommand as HookCommandDef, HookDefinition,
    HookFunction, HookHttp, HookInput, HookMatcherEntry, HookOutput, HookPrompt, HookResponse,
    HookResponseDecision, HookSpecificOutputV2, HookType,
    PermissionBehavior as HookPermissionBehavior, PermissionRequestDecision, PermissionUpdate,
};
pub use hooks::{
    HOOK_EVENTS, HookDecision, HookEventEnvelope, HookEventKind, HookResponse as HookResponseV1,
    HookSpecificOutput, is_hook_event,
};
pub use ids::{AgentId, SessionId};
pub use message::{
    AssistantContentBlock, AssistantMessage, AttachmentMessage, CollapsedReadSearchMessage,
    GroupedToolUseMessage, HookResultMessage, Message, MessageBase, MessageOrigin, ProgressMessage,
    SystemMessage, SystemMessageSubtype, TombstoneMessage, ToolUseSummaryMessage, UserMessage,
};
pub use message_types::{
    AttachmentMessageType, HookResultMessageType, NormalizedMessage, NormalizedOrigin,
    ProgressMessageType, SystemAPIErrorMessage, SystemAgentsKilledMessage, SystemApiMetricsMessage,
    SystemAwaySummaryMessage, SystemCompactBoundaryMessage, SystemFileSnapshotMessage,
    SystemMemorySavedMessage, SystemMicrocompactBoundaryMessage, SystemPermissionRetryMessage,
    SystemStopHookSummaryMessage, SystemThinkingMessage, TombstoneMessageType,
    ToolUseSummaryMessageType,
};
pub use permission_types::{
    PermissionBehavior, PermissionDecisionMeta, PermissionDecisionReason, PermissionResult,
    PermissionRule, PermissionRuleSource,
};
pub use state::{AppState, FileHistoryState, ToolPermissionContext};
pub use usage::UsageAccumulator;

/// Application binary name.
pub const APP_NAME: &str = "remote-code";
/// Human-readable product name.
pub const PRODUCT_NAME: &str = "Remote Code Rust";
/// Default directory name for the application profile.
pub const DEFAULT_PROFILE_DIR_NAME: &str = ".remote-code-rust";
/// Legacy directory name used by the upstream Node.js runtime.
pub const LEGACY_PROFILE_DIR_NAME: &str = ".remote-code";

/// Permission mode controlling how tool executions are authorised.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Ask for every non-read operation.
    #[default]
    Default,
    /// Auto-accept file edits, ask for everything else.
    AcceptEdits,
    /// Model decides which operations to auto-approve based on confidence.
    Auto,
    /// Skip all permission prompts (dangerous).
    BypassPermissions,
    /// Never prompt; only pre-approved or read-only operations proceed.
    DontAsk,
    /// Plan-only mode — no tool execution at all.
    Plan,
}

impl PermissionMode {
    /// Return the legacy string representation used by the upstream runtime.
    #[must_use]
    pub fn as_legacy_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Plan => "plan",
        }
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" | "Default" => Ok(Self::Default),
            "acceptEdits" | "accept-edits" | "accept_edits" => Ok(Self::AcceptEdits),
            "auto" | "Auto" => Ok(Self::Auto),
            "bypassPermissions" | "bypass-permissions" | "bypass_permissions" => {
                Ok(Self::BypassPermissions)
            }
            "dontAsk" | "dont-ask" | "dont_ask" => Ok(Self::DontAsk),
            "plan" | "Plan" => Ok(Self::Plan),
            _ => Err(format!("unknown permission mode: {s}")),
        }
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_legacy_str())
    }
}

/// LLM provider wire protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderProtocol {
    /// OpenAI-compatible chat completions API.
    #[default]
    OpenAi,
    /// Anthropic Messages API.
    Anthropic,
    /// AWS Bedrock (uses SigV4 auth — not yet implemented at the provider layer).
    Bedrock,
    /// Google Vertex AI (uses GCP auth — not yet implemented at the provider layer).
    Vertex,
}

impl ProviderProtocol {
    /// Return the kebab-case protocol identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Bedrock => "bedrock",
            Self::Vertex => "vertex",
        }
    }
}

impl FromStr for ProviderProtocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            "bedrock" => Ok(Self::Bedrock),
            "vertex" => Ok(Self::Vertex),
            _ => Err(format!("unknown provider protocol: {s}")),
        }
    }
}

impl std::fmt::Display for ProviderProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Input format for the CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputFormat {
    /// Human-readable text input.
    #[default]
    Text,
    /// Line-delimited JSON streaming input.
    StreamJson,
}

impl FromStr for InputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "stream-json" | "stream_json" | "streamJson" => Ok(Self::StreamJson),
            _ => Err(format!("unknown input format: {s}")),
        }
    }
}

impl std::fmt::Display for InputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => f.write_str("text"),
            Self::StreamJson => f.write_str("stream-json"),
        }
    }
}

/// Output format for the CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Human-readable text output.
    #[default]
    Text,
    /// Single JSON result object.
    Json,
    /// Line-delimited JSON streaming output.
    StreamJson,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "stream-json" | "stream_json" | "streamJson" => Ok(Self::StreamJson),
            _ => Err(format!("unknown output format: {s}")),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => f.write_str("text"),
            Self::Json => f.write_str("json"),
            Self::StreamJson => f.write_str("stream-json"),
        }
    }
}

/// Hook lifecycle events that can trigger command hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HookEvent {
    /// Fired when a new session starts.
    SessionStart,
    /// Fired before a tool is executed.
    PreToolUse,
    /// Fired after a tool succeeds.
    PostToolUse,
    /// Fired after a tool fails.
    PostToolUseFailure,
}

impl HookEvent {
    /// Return the PascalCase event name used by the upstream runtime.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
        }
    }
}

impl FromStr for HookEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "session-start" | "SessionStart" | "session_start" => Ok(Self::SessionStart),
            "pre-tool-use" | "PreToolUse" | "pre_tool_use" => Ok(Self::PreToolUse),
            "post-tool-use" | "PostToolUse" | "post_tool_use" => Ok(Self::PostToolUse),
            "post-tool-use-failure" | "PostToolUseFailure" | "post_tool_use_failure" => {
                Ok(Self::PostToolUseFailure)
            }
            _ => Err(format!("unknown hook event: {s}")),
        }
    }
}

/// Shell interpreter used to execute hook commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookShell {
    /// POSIX bash shell.
    Bash,
    /// Windows PowerShell.
    PowerShell,
}

impl HookShell {
    /// Return the lowercase shell name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::PowerShell => "powershell",
        }
    }
}

impl FromStr for HookShell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bash" => Ok(Self::Bash),
            "powershell" | "pwsh" => Ok(Self::PowerShell),
            _ => Err(format!("unknown hook shell: {s}")),
        }
    }
}

/// A single command hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandHook {
    /// The shell command to execute.
    pub command: String,
    /// Optional condition expression (e.g. `Bash(git status *)`).
    #[serde(default, rename = "if")]
    pub condition: Option<String>,
    /// Shell interpreter override.
    #[serde(default)]
    pub shell: Option<HookShell>,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Status message shown while the hook runs.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Whether the hook should only fire once per session.
    #[serde(default)]
    pub once: bool,
}

/// Tagged union of hook command types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookCommand {
    /// A shell command hook.
    Command(CommandHook),
}

impl HookCommand {
    /// Borrow the inner [`CommandHook`].
    #[must_use]
    pub fn as_command(&self) -> &CommandHook {
        match self {
            Self::Command(command) => command,
        }
    }
}

/// A hook matcher that groups a pattern with its associated hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMatcher {
    /// Optional tool-name pattern (e.g. `Bash` or `Bash(git *)`).
    #[serde(default)]
    pub matcher: Option<String>,
    /// Hooks to run when the matcher fires.
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

/// Current state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// No active prompt is being processed.
    Idle,
    /// A prompt is currently being processed.
    Running,
    /// Waiting for user approval of a tool call.
    RequiresAction,
}

impl SessionState {
    /// Return the snake_case state name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::RequiresAction => "requires_action",
        }
    }
}

/// Role of a conversation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationRole {
    /// System prompt.
    System,
    /// User input.
    User,
    /// Assistant response.
    Assistant,
    /// Tool result.
    Tool,
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Tool name (e.g. `"read_file"`).
    pub name: String,
    /// JSON object of tool arguments.
    #[serde(default)]
    pub input: Value,
}

/// Supported media types for multimodal attachments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttachmentMediaType {
    /// PNG image.
    ImagePng,
    /// JPEG image.
    ImageJpeg,
    /// GIF image.
    ImageGif,
    /// WebP image.
    ImageWebp,
    /// PDF document.
    ApplicationPdf,
}

impl AttachmentMediaType {
    /// Return the MIME type string (e.g. `"image/png"`).
    #[must_use]
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::ImagePng => "image/png",
            Self::ImageJpeg => "image/jpeg",
            Self::ImageGif => "image/gif",
            Self::ImageWebp => "image/webp",
            Self::ApplicationPdf => "application/pdf",
        }
    }

    /// Infer media type from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(Self::ImagePng),
            "jpg" | "jpeg" => Some(Self::ImageJpeg),
            "gif" => Some(Self::ImageGif),
            "webp" => Some(Self::ImageWebp),
            "pdf" => Some(Self::ApplicationPdf),
            _ => None,
        }
    }
}

/// A multimodal attachment (image or PDF) embedded in a conversation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// MIME type of the attachment.
    pub media_type: AttachmentMediaType,
    /// Base64-encoded content.
    pub data: String,
    /// Optional original filename.
    #[serde(default)]
    pub filename: Option<String>,
}

impl Attachment {
    /// Create an attachment from raw bytes.
    pub fn from_bytes(
        media_type: AttachmentMediaType,
        data: &[u8],
        filename: Option<String>,
    ) -> Self {
        Self {
            media_type,
            data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
            filename,
        }
    }

    /// Read a file and create an attachment, inferring the media type from extension.
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let media_type = AttachmentMediaType::from_extension(ext)
            .ok_or_else(|| format!("unsupported file type: .{ext}"))?;
        let data =
            std::fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
        Ok(Self::from_bytes(media_type, &data, filename))
    }
}

/// Token usage statistics returned by the provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Number of input (prompt) tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Number of output (completion) tokens.
    #[serde(default)]
    pub output_tokens: u64,
    /// Anthropic cache read tokens (tokens served from cache).
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Anthropic cache creation tokens (tokens written to cache).
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Server-side web search requests (Anthropic server tool use).
    #[serde(default)]
    pub server_tool_use_web_search_requests: u64,
    /// Server-side web fetch requests (Anthropic server tool use).
    #[serde(default)]
    pub server_tool_use_web_fetch_requests: u64,
    /// Cache creation ephemeral 5-minute TTL input tokens.
    #[serde(default)]
    pub cache_creation_ephemeral_5m_input_tokens: u64,
    /// Cache creation ephemeral 1-hour TTL input tokens.
    #[serde(default)]
    pub cache_creation_ephemeral_1h_input_tokens: u64,
}

/// A single entry in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// Stable message identifier used for transcript-aware features.
    #[serde(default = "Uuid::new_v4")]
    pub uuid: Uuid,
    /// Who produced this entry.
    pub role: ConversationRole,
    /// Primary text content.
    #[serde(default)]
    pub text: String,
    /// Optional abbreviated text for context-window compaction.
    #[serde(default)]
    pub history_text: Option<String>,
    /// Anthropic-style content blocks.
    #[serde(default)]
    pub content_blocks: Vec<Value>,
    /// Tool calls embedded in an assistant message.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Multimodal attachments (images, PDFs) for user-role entries.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Tool-call ID this entry responds to (for tool-role entries).
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// Tool name for tool-role entries.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether this entry represents an error.
    #[serde(default)]
    pub is_error: bool,
}

impl ConversationEntry {
    /// Create a system-role entry.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::System,
            text: text.into(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    /// Create a user-role entry.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::User,
            text: text.into(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    /// Create a user-role entry backed entirely by provider content blocks.
    pub fn user_with_content_blocks(content_blocks: Vec<Value>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::User,
            text: String::new(),
            history_text: None,
            content_blocks,
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    /// Create a user-role entry with multimodal attachments.
    pub fn user_with_attachments(text: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::User,
            text: text.into(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            attachments,
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    /// Create an assistant-role entry.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::Assistant,
            text: text.into(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        }
    }

    /// Create a tool-role entry responding to a tool call.
    pub fn tool(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        text: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            role: ConversationRole::Tool,
            text: text.into(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            is_error,
        }
    }

    /// Return the history text, falling back to the full text if none is set.
    #[must_use]
    pub fn history_text(&self) -> String {
        self.history_text
            .clone()
            .unwrap_or_else(|| self.text.clone())
    }
}

/// Parsed response from the LLM provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// Primary text content of the response.
    pub text: String,
    /// Optional abbreviated text for context compaction.
    #[serde(default)]
    pub history_text: Option<String>,
    /// Extended thinking/reasoning content (if enabled and returned by the model).
    #[serde(default)]
    pub thinking: Option<String>,
    /// Anthropic-style content blocks.
    #[serde(default)]
    pub content_blocks: Vec<Value>,
    /// Tool calls requested by the assistant.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Provider request / response identifier when surfaced by the backend.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Token usage statistics.
    #[serde(default)]
    pub usage: UsageSummary,
    /// Provider stop reason (e.g. `"end_turn"`, `"tool_use"`).
    #[serde(default = "default_stop_reason")]
    pub stop_reason: String,
    /// Research metadata from the model (Anthropic research mode).
    #[serde(default)]
    pub research: Option<Value>,
}

fn default_stop_reason() -> String {
    "end_turn".to_owned()
}

/// Result of executing a tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool output content.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
    /// Provider-facing structured content blocks (for example `tool_reference`).
    #[serde(default)]
    pub content_blocks: Vec<Value>,
    /// Additional user-role provider blocks that should be appended after the tool result.
    #[serde(default)]
    pub follow_up_user_blocks: Vec<Value>,
}

/// Fully-resolved execution request for a concrete sub-agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentForkSnapshot {
    /// Parent context messages for cache-safe fork execution.
    #[serde(default)]
    pub fork_context_messages: Vec<Message>,
    /// Parent system prompt, if explicitly captured.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Parent user context.
    #[serde(default)]
    pub user_context: std::collections::BTreeMap<String, String>,
    /// Parent system context.
    #[serde(default)]
    pub system_context: std::collections::BTreeMap<String, String>,
}

/// Fully-resolved execution request for a concrete sub-agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentExecutionRequest {
    /// Agent type identifier.
    pub agent_type: String,
    /// Optional teammate name when this agent is part of a named team.
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Optional team name for teammate-scoped execution.
    #[serde(default)]
    pub team_name: Option<String>,
    /// Primary task prompt.
    pub task: String,
    /// Optional short human description of the task.
    #[serde(default)]
    pub description: Option<String>,
    /// Conversation context inherited from the caller.
    #[serde(default)]
    pub context: Vec<ConversationEntry>,
    /// Optional system prompt override for the child agent.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Short critical reminder reinjected as a system-reminder user message.
    #[serde(default)]
    pub critical_system_reminder: Option<String>,
    /// Omit CLAUDE.md-derived user context for this child run.
    #[serde(default)]
    pub omit_claude_md: bool,
    /// Omit gitStatus from the child system context.
    #[serde(default)]
    pub omit_git_status: bool,
    /// Optional model override for the child agent.
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum number of turns for the child agent.
    pub max_turns: u32,
    /// Resolved internal tool names available to the child agent.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Permission mode requested for the child runtime.
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// Working directory for the child agent.
    pub working_dir: PathBuf,
    /// Additional working directories available to the child agent.
    #[serde(default)]
    pub additional_working_directories: Vec<PathBuf>,
    /// Run the child without writing transcript/session artifacts to the parent profile.
    #[serde(default)]
    pub skip_transcript: bool,
    /// Cache-safe fork snapshot for implicit fork runs.
    #[serde(default)]
    pub fork_snapshot: Option<SubAgentForkSnapshot>,
}

/// Result returned by a concrete sub-agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentExecutionResult {
    /// Final agent output.
    pub output: String,
    /// Whether the agent completed successfully.
    pub success: bool,
    /// Number of turns consumed.
    pub turns: u32,
    /// Usage summary for the run.
    #[serde(default)]
    pub usage: UsageSummary,
}

/// Trait for providing LLM completion capability to sub-agents.
///
/// This trait breaks the circular dependency between `rc-tools` and
/// `rc-provider`: `rc-tools` defines the agent tool that needs LLM access,
/// but cannot depend on `rc-provider` directly. Instead, the completion
/// capability is injected via this trait at the TUI/application layer.
#[async_trait::async_trait]
pub trait SubAgentCompletion: Send + Sync {
    /// Send a conversation to the LLM and return the response.
    ///
    /// The implementation is responsible for provider selection, retry logic,
    /// and message formatting.
    async fn complete(
        &self,
        conversation: &[ConversationEntry],
    ) -> anyhow::Result<ProviderResponse>;

    /// Returns `true` when this runtime can execute a fully-resolved agent request.
    fn supports_agent_execution(&self) -> bool {
        false
    }

    /// Execute a fully-resolved agent request using the host runtime.
    ///
    /// Implementations that do not support this richer execution seam can
    /// leave the default behavior in place and only provide `complete(...)`.
    async fn execute_agent(
        &self,
        _request: SubAgentExecutionRequest,
    ) -> anyhow::Result<SubAgentExecutionResult> {
        Err(anyhow::anyhow!(
            "host runtime does not support concrete agent execution"
        ))
    }
}

/// A persisted event in the session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Which session this event belongs to.
    pub session_id: Uuid,
    /// Event type discriminator (e.g. `"prompt"`, `"tool_result"`).
    pub event_type: String,
    /// Optional conversation entry associated with this event.
    #[serde(default)]
    pub conversation: Option<ConversationEntry>,
    /// Optional JSON payload with event-specific data.
    #[serde(default)]
    pub payload: Option<Value>,
}

/// Generate the default system prompt for the given working directory.
#[must_use]
pub fn default_system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "You are Remote Code Rust, a concise coding agent running inside {}. Keep responses practical, prefer safe actions, and preserve compatibility with the Remote Code stream-json runtime where possible. When using shell tools, do not prefix commands with cd or Set-Location; pass the target directory via the tool's cwd field instead.",
        cwd.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{HookCommand, HookEvent, HookShell};

    #[test]
    fn hook_event_round_trips_as_upstream_name() {
        let encoded =
            serde_json::to_string(&HookEvent::PreToolUse).expect("hook event encode should work");
        assert_eq!(encoded, "\"PreToolUse\"");

        let decoded: HookEvent =
            serde_json::from_str(&encoded).expect("hook event decode should work");
        assert_eq!(decoded, HookEvent::PreToolUse);
    }

    #[test]
    fn command_hook_deserializes_upstream_shape() {
        let hook: HookCommand = serde_json::from_str(
            r#"{
                "type": "command",
                "command": "echo ready",
                "if": "Bash(git status *)",
                "shell": "powershell",
                "timeout": 5,
                "once": true
            }"#,
        )
        .expect("command hook decode should work");

        let command = hook.as_command();
        assert_eq!(command.command, "echo ready");
        assert_eq!(command.condition.as_deref(), Some("Bash(git status *)"));
        assert_eq!(command.shell, Some(HookShell::PowerShell));
        assert_eq!(command.timeout, Some(5));
        assert!(command.once);
    }
}
