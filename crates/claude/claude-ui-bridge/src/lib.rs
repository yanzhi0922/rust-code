//! Abstract UI bridge for multi-frontend support (TUI, GUI, Remote-Control).
//!
//! This crate defines the **trait boundaries** that every frontend must implement
//! to integrate with the remote-code-rust core. By programming against these
//! traits, the core engine remains completely decoupled from any specific UI
//! framework (ratatui, egui, iced, web, etc.).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │                Core Engine                  │
//! │  (rc-provider, rc-tools, rc-session, etc.)  │
//! └──────────────┬──────────────────────────────┘
//!                │  calls UiFrontend trait
//! ┌──────────────┴──────────────────────────────┐
//! │            rc-ui-bridge                     │
//! │  UiFrontend trait + UiEvent enum            │
//! └──────┬──────────┬──────────┬────────────────┘
//!        │          │          │
//!   ┌────┴───┐ ┌───┴───┐ ┌───┴──────────┐
//!   │  TUI   │ │  GUI  │ │Remote-Control│
//!   │(ratatui│ │(egui/ │ │  (HTTP/WS)   │
//!   │/crosst)│ │ iced) │ │              │
//!   └────────┘ └───────┘ └──────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use claude_ui_bridge::{UiFrontend, UiEvent};
//!
//! struct MyGuiFrontend;
//!
//! #[async_trait]
//! impl UiFrontend for MyGuiFrontend {
//!     async fn render_event(&self, event: &UiEvent) -> anyhow::Result<()> {
//!         match event {
//!             UiEvent::AssistantText { text } => println!("{text}"),
//!             _ => {}
//!         }
//!         Ok(())
//!     }
//! }
//! ```

pub mod bridge;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiTaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiTaskKind {
    Background,
    Delegation,
    Batch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTaskNode {
    pub id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub status: UiTaskStatus,
    pub kind: UiTaskKind,
    pub depth: u32,
    pub summary: String,
    pub turns_used: Option<u32>,
    pub output_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiProviderStatusSnapshot {
    pub name: String,
    pub model: Option<String>,
    pub protocol: String,
    pub base_url: Option<String>,
    pub auth_source: Option<String>,
    pub effort: Option<String>,
    pub fallback_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UiRuntimeMcpOriginCounts {
    pub cwd: usize,
    pub profile: usize,
    pub explicit: usize,
    pub plugin: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UiRuntimeMcpServerStatus {
    Connected,
    Failed,
    NeedsAuth,
    Pending,
    Disabled,
}

impl UiRuntimeMcpServerStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Failed => "failed",
            Self::NeedsAuth => "needs-auth",
            Self::Pending => "pending",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UiRuntimeMcpStatusCounts {
    pub connected: usize,
    pub failed: usize,
    pub needs_auth: usize,
    pub pending: usize,
    pub disabled: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UiRuntimeMcpInventorySummary {
    pub total_servers: usize,
    pub enabled_servers: usize,
    pub disabled_servers: usize,
    pub unique_server_names: usize,
    pub ambiguous_server_names: usize,
    pub warning_count: usize,
    pub origins: UiRuntimeMcpOriginCounts,
    pub status_counts: UiRuntimeMcpStatusCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiRuntimeStatusSnapshot {
    pub session_name: Option<String>,
    pub provider: UiProviderStatusSnapshot,
    pub permission_mode: String,
    #[serde(default)]
    pub output_style: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub brief_enabled: bool,
    #[serde(default)]
    pub proactive_active: bool,
    pub setting_sources: Vec<String>,
    pub allowed_setting_sources: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub mcp: UiRuntimeMcpInventorySummary,
}

// ---------------------------------------------------------------------------
// UI Events — the universal language between core and frontends
// ---------------------------------------------------------------------------

/// Events emitted by the core engine for frontends to render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiEvent {
    // ── Session lifecycle ────────────────────────────────────────────
    /// Session initialized with metadata.
    SessionInit {
        /// Session unique identifier.
        session_id: Uuid,
        /// Model being used.
        model: String,
        /// Provider name.
        provider: String,
        /// Current working directory.
        cwd: String,
        /// Permission mode.
        permission_mode: String,
    },
    /// Session is shutting down.
    SessionEnd {
        /// Session unique identifier.
        session_id: Uuid,
        /// Final cost summary.
        cost_summary: Option<String>,
    },

    // ── Conversation ─────────────────────────────────────────────────
    /// User submitted a message.
    UserMessage {
        /// The user's input text.
        text: String,
    },
    /// Assistant is generating text (streaming delta).
    AssistantText {
        /// Incremental text chunk.
        text: String,
    },
    /// Assistant finished generating.
    AssistantComplete {
        /// Full response text.
        text: String,
        /// Stop reason from the provider.
        stop_reason: String,
        /// Token usage for this turn.
        usage: UiUsage,
    },

    // ── Tool execution ───────────────────────────────────────────────
    /// A tool call has started.
    ToolStart {
        /// Tool call ID from the provider.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Tool input parameters (JSON).
        input: serde_json::Value,
    },
    /// A tool call produced intermediate output.
    ToolProgress {
        /// Tool call ID.
        tool_call_id: String,
        /// Progress message.
        message: String,
        /// Optional percentage (0-100).
        percent: Option<u8>,
    },
    /// A tool call completed.
    ToolResult {
        /// Tool call ID.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Whether the tool execution was an error.
        is_error: bool,
        /// Result content (may be truncated).
        output: String,
    },

    // ── Permission ───────────────────────────────────────────────────
    /// Permission request pending user approval.
    PermissionRequest {
        /// Request unique identifier.
        request_id: String,
        /// Tool name requesting permission.
        tool_name: String,
        /// Short description of the action.
        description: String,
    },
    /// Permission decision rendered.
    PermissionDecision {
        /// Request unique identifier.
        request_id: String,
        /// Whether the action was allowed.
        allowed: bool,
        /// Optional explanation.
        reason: Option<String>,
    },

    // ── Context management ───────────────────────────────────────────
    /// Context window compaction occurred.
    ContextCompacted {
        /// Number of entries removed.
        entries_removed: usize,
        /// Remaining context usage ratio (0.0-1.0).
        usage_ratio: f64,
    },
    /// Context usage update.
    ContextUsage {
        /// Current usage ratio (0.0-1.0).
        ratio: f64,
        /// Estimated token count.
        estimated_tokens: u64,
        /// Maximum context tokens.
        max_tokens: u64,
    },

    // ── Cost tracking ────────────────────────────────────────────────
    /// Cost updated after a provider call.
    CostUpdate {
        /// Turn cost in USD.
        turn_cost_usd: f64,
        /// Total session cost in USD.
        total_cost_usd: f64,
        /// Input tokens for this turn.
        input_tokens: u64,
        /// Output tokens for this turn.
        output_tokens: u64,
    },

    // ── Error ────────────────────────────────────────────────────────
    /// An error occurred.
    Error {
        /// Error category.
        category: ErrorCategory,
        /// Human-readable error message.
        message: String,
        /// Suggested recovery action.
        suggestion: Option<String>,
    },

    // ── Status / info ────────────────────────────────────────────────
    /// Status message (spinner text, info line, etc.).
    Status {
        /// Status message text.
        message: String,
    },
    /// Snapshot of the active runtime/provider/permission status.
    StatusSnapshot {
        /// Shared status surface used by CLI, GUI, and remote consumers.
        snapshot: Box<UiRuntimeStatusSnapshot>,
    },
    /// Provider is thinking / processing.
    Thinking {
        /// Optional thinking content (for models that expose it).
        content: Option<String>,
    },

    // ── Multi-agent ──────────────────────────────────────────────────
    /// A sub-agent was dispatched.
    AgentDispatched {
        /// Agent identifier.
        agent_id: String,
        /// Agent task description.
        task: String,
    },
    /// A sub-agent completed.
    AgentComplete {
        /// Agent identifier.
        agent_id: String,
        /// Whether the agent succeeded.
        success: bool,
        /// Agent result summary.
        summary: String,
    },

    // ── Subtask delegation ────────────────────────────────────────────
    /// A subtask delegation started.
    SubtaskStarted {
        /// Unique task identifier.
        task_id: String,
        /// Parent task identifier when this task is nested.
        parent_task_id: Option<String>,
        /// Task description.
        description: String,
        /// Current delegation depth.
        depth: u32,
    },
    /// A subtask made progress (completed a turn).
    SubtaskProgress {
        /// Task identifier.
        task_id: String,
        /// Current turn number.
        turn: u32,
        /// Maximum turns allowed.
        max_turns: u32,
        /// Short summary of what happened this turn.
        summary: String,
    },
    /// A subtask completed.
    SubtaskCompleted {
        /// Task identifier.
        task_id: String,
        /// Whether the subtask succeeded.
        success: bool,
        /// Preview of the output (truncated).
        output_preview: String,
        /// Number of turns used.
        turns_used: u32,
    },
    /// Batch delegation progress update.
    BatchProgress {
        /// Total tasks in the batch.
        total: usize,
        /// Number of completed tasks.
        completed: usize,
        /// Number of currently running tasks.
        running: usize,
    },
    /// Snapshot of the current task tree.
    TaskSnapshot {
        /// Current known tasks.
        tasks: Vec<UiTaskNode>,
    },

    // ── Streaming ────────────────────────────────────────────────────
    /// Streaming started.
    StreamStart {
        /// Provider protocol.
        protocol: String,
    },
    /// Streaming ended.
    StreamEnd {
        /// Total chunks received.
        chunks: u64,
        /// Total duration in milliseconds.
        duration_ms: u64,
    },
}

/// Token usage information for UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiUsage {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
}

/// Error category for structured error display.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCategory {
    /// API / provider error.
    Provider,
    /// Tool execution error.
    Tool,
    /// Permission denied.
    Permission,
    /// Network connectivity.
    Network,
    /// File system error.
    FileSystem,
    /// Configuration error.
    Config,
    /// Context window overflow.
    ContextOverflow,
    /// Internal error.
    Internal,
}

// ---------------------------------------------------------------------------
// UiFrontend trait — the contract every frontend must implement
// ---------------------------------------------------------------------------

/// The core trait that any UI frontend must implement.
///
/// This trait defines the bidirectional interface between the core engine
/// and the user interface. The core calls `render_event` to push information
/// to the frontend, and the frontend can call methods on the core through
/// the `UiAction` channel.
#[async_trait]
pub trait UiFrontend: Send + Sync {
    /// Render a UI event. Called by the core engine whenever something
    /// happens that the user should see.
    ///
    /// # Errors
    /// Returns an error if the frontend fails to render the event.
    async fn render_event(&self, event: &UiEvent) -> Result<()>;

    /// Request user input. Called when the core needs text input from the user.
    ///
    /// # Errors
    /// Returns an error if the input request fails or is cancelled.
    async fn request_input(&self, prompt: &str) -> Result<String>;

    /// Request a permission decision from the user.
    ///
    /// # Errors
    /// Returns an error if the permission request fails.
    async fn request_permission(&self, tool_name: &str, description: &str) -> Result<bool>;

    /// Check if the frontend supports a specific feature.
    fn supports_feature(&self, feature: UiFeature) -> bool;

    /// Get the frontend name for diagnostics.
    fn frontend_name(&self) -> &str;
}

/// Features that a frontend may or may not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFeature {
    /// Rich text / Markdown rendering.
    RichText,
    /// Syntax-highlighted code blocks.
    SyntaxHighlighting,
    /// Inline images.
    Images,
    /// Interactive approval dialogs.
    InteractiveApproval,
    /// Multi-panel layout.
    MultiPanel,
    /// Progress spinners.
    Spinner,
    /// Auto-completion.
    AutoCompletion,
    /// Mouse interaction.
    Mouse,
    /// Resize handling.
    Resize,
    /// Streaming text display.
    Streaming,
    /// Color / themes.
    Color,
}

// ---------------------------------------------------------------------------
// UiAction — requests from frontend to core
// ---------------------------------------------------------------------------

/// Actions that the frontend can request the core to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiAction {
    /// Submit user text input.
    SubmitInput {
        /// The user's text.
        text: String,
    },
    /// Respond to a permission request.
    PermissionResponse {
        /// The request ID.
        request_id: String,
        /// Whether to allow.
        allow: bool,
    },
    /// Interrupt the current operation.
    Interrupt,
    /// Request context compaction.
    CompactContext,
    /// Change the active model.
    ChangeModel {
        /// New model identifier.
        model: String,
    },
    /// Quit the session.
    Quit,
}

// ---------------------------------------------------------------------------
// NullFrontend — a no-op frontend for headless / testing
// ---------------------------------------------------------------------------

/// A no-op frontend that discards all events. Useful for headless mode
/// and testing.
pub struct NullFrontend;

#[async_trait]
impl UiFrontend for NullFrontend {
    async fn render_event(&self, _event: &UiEvent) -> Result<()> {
        Ok(())
    }

    async fn request_input(&self, _prompt: &str) -> Result<String> {
        Ok(String::new())
    }

    async fn request_permission(&self, _tool_name: &str, _description: &str) -> Result<bool> {
        Ok(true)
    }

    fn supports_feature(&self, _feature: UiFeature) -> bool {
        false
    }

    fn frontend_name(&self) -> &str {
        "null"
    }
}

// ---------------------------------------------------------------------------
// CollectingFrontend — collects events for testing
// ---------------------------------------------------------------------------

/// A frontend that collects all events into a Vec for assertion in tests.
pub struct CollectingFrontend {
    events: parking_lot::Mutex<Vec<UiEvent>>,
}

impl CollectingFrontend {
    /// Create a new collecting frontend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Get all collected events.
    #[must_use]
    pub fn events(&self) -> Vec<UiEvent> {
        self.events.lock().clone()
    }

    /// Check if any collected event matches a predicate.
    pub fn has_event(&self, predicate: impl Fn(&UiEvent) -> bool) -> bool {
        self.events.lock().iter().any(predicate)
    }
}

impl Default for CollectingFrontend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UiFrontend for CollectingFrontend {
    async fn render_event(&self, event: &UiEvent) -> Result<()> {
        self.events.lock().push(event.clone());
        Ok(())
    }

    async fn request_input(&self, _prompt: &str) -> Result<String> {
        Ok("test input".to_owned())
    }

    async fn request_permission(&self, _tool_name: &str, _description: &str) -> Result<bool> {
        Ok(true)
    }

    fn supports_feature(&self, feature: UiFeature) -> bool {
        matches!(feature, UiFeature::Streaming)
    }

    fn frontend_name(&self) -> &str {
        "collecting"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_event_serializes_to_json() {
        let event = UiEvent::AssistantText {
            text: "Hello, world!".to_owned(),
        };
        let json = serde_json::to_string(&event).expect("serialize should not fail");
        assert!(json.contains("AssistantText"));
        assert!(json.contains("Hello, world!"));
    }

    #[test]
    fn ui_event_deserializes_from_json() {
        let event = UiEvent::CostUpdate {
            turn_cost_usd: 0.003,
            total_cost_usd: 0.015,
            input_tokens: 500,
            output_tokens: 200,
        };
        let json = serde_json::to_string(&event).expect("serialize should not fail");
        let parsed: UiEvent = serde_json::from_str(&json).expect("deserialize should not fail");
        if let UiEvent::CostUpdate { turn_cost_usd, .. } = parsed {
            assert!((turn_cost_usd - 0.003).abs() < 0.0001);
        } else {
            panic!("Expected CostUpdate variant");
        }
    }

    #[test]
    fn status_snapshot_event_round_trips() {
        let event = UiEvent::StatusSnapshot {
            snapshot: Box::new(UiRuntimeStatusSnapshot {
                session_name: Some("Investigate parity".to_owned()),
                provider: UiProviderStatusSnapshot {
                    name: "glm-coding".to_owned(),
                    model: Some("glm-5.1".to_owned()),
                    protocol: "anthropic".to_owned(),
                    base_url: Some("https://open.bigmodel.cn/api/anthropic/v1/messages".to_owned()),
                    auth_source: Some("env:REMOTE_CODE_API_KEY".to_owned()),
                    effort: Some("medium".to_owned()),
                    fallback_model: Some("glm-5-turbo".to_owned()),
                },
                permission_mode: "default".to_owned(),
                output_style: Some("Explanatory".to_owned()),
                language: Some("Chinese".to_owned()),
                brief_enabled: true,
                proactive_active: true,
                setting_sources: vec!["env:REMOTE_CODE_MODEL".to_owned()],
                allowed_setting_sources: vec!["user".to_owned(), "project".to_owned()],
                allowed_tools: vec!["read_file".to_owned()],
                disallowed_tools: vec!["bash_command".to_owned()],
                mcp: UiRuntimeMcpInventorySummary {
                    total_servers: 4,
                    enabled_servers: 3,
                    disabled_servers: 1,
                    unique_server_names: 3,
                    ambiguous_server_names: 1,
                    warning_count: 2,
                    origins: UiRuntimeMcpOriginCounts {
                        cwd: 1,
                        profile: 1,
                        explicit: 0,
                        plugin: 2,
                    },
                    status_counts: UiRuntimeMcpStatusCounts {
                        connected: 0,
                        failed: 0,
                        needs_auth: 0,
                        pending: 3,
                        disabled: 1,
                    },
                },
            }),
        };
        let json = serde_json::to_string(&event).expect("serialize should not fail");
        let parsed: UiEvent = serde_json::from_str(&json).expect("deserialize should not fail");
        match parsed {
            UiEvent::StatusSnapshot { snapshot } => {
                assert_eq!(snapshot.provider.name, "glm-coding");
                assert_eq!(snapshot.output_style.as_deref(), Some("Explanatory"));
                assert_eq!(snapshot.language.as_deref(), Some("Chinese"));
                assert!(snapshot.brief_enabled);
                assert!(snapshot.proactive_active);
                assert_eq!(snapshot.allowed_setting_sources, vec!["user", "project"]);
                assert_eq!(snapshot.allowed_tools, vec!["read_file"]);
                assert_eq!(snapshot.disallowed_tools, vec!["bash_command"]);
                assert_eq!(snapshot.mcp.total_servers, 4);
                assert_eq!(snapshot.mcp.ambiguous_server_names, 1);
                assert_eq!(snapshot.mcp.origins.plugin, 2);
                assert_eq!(snapshot.mcp.status_counts.pending, 3);
                assert_eq!(snapshot.mcp.status_counts.disabled, 1);
            }
            _ => panic!("Expected StatusSnapshot variant"),
        }
    }

    #[test]
    fn error_category_serializes() {
        let cat = ErrorCategory::Provider;
        let json = serde_json::to_string(&cat).expect("serialize should not fail");
        assert!(json.contains("Provider"));
    }

    #[tokio::test]
    async fn null_frontend_discards_events() {
        let fe = NullFrontend;
        let result = fe
            .render_event(&UiEvent::Status {
                message: "test".to_owned(),
            })
            .await;
        assert!(result.is_ok());
        assert_eq!(fe.frontend_name(), "null");
        assert!(!fe.supports_feature(UiFeature::RichText));
    }

    #[tokio::test]
    async fn collecting_frontend_captures_events() {
        let fe = CollectingFrontend::new();
        fe.render_event(&UiEvent::UserMessage {
            text: "hello".to_owned(),
        })
        .await
        .expect("render should not fail");
        fe.render_event(&UiEvent::AssistantText {
            text: "world".to_owned(),
        })
        .await
        .expect("render should not fail");

        let events = fe.events();
        assert_eq!(events.len(), 2);
        assert!(fe.has_event(|e| matches!(e, UiEvent::UserMessage { .. })));
        assert!(fe.has_event(|e| matches!(e, UiEvent::AssistantText { .. })));
    }

    #[test]
    fn ui_action_serializes() {
        let action = UiAction::SubmitInput {
            text: "do something".to_owned(),
        };
        let json = serde_json::to_string(&action).expect("serialize should not fail");
        assert!(json.contains("SubmitInput"));
    }

    #[test]
    fn ui_feature_coverage() {
        let features = [
            UiFeature::RichText,
            UiFeature::SyntaxHighlighting,
            UiFeature::Images,
            UiFeature::InteractiveApproval,
            UiFeature::MultiPanel,
            UiFeature::Spinner,
            UiFeature::AutoCompletion,
            UiFeature::Mouse,
            UiFeature::Resize,
            UiFeature::Streaming,
            UiFeature::Color,
        ];
        assert_eq!(features.len(), 11);
    }
}
