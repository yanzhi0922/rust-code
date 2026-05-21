use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentId, Message, PermissionMode, SessionId};

/// Mutable permission context propagated alongside tool execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPermissionContext {
    #[serde(default)]
    pub allowlisted_tools: BTreeSet<String>,
    #[serde(default)]
    pub denylisted_tools: BTreeSet<String>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub extra: Value,
}

/// File history snapshot used by compaction/recovery flows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileHistoryState {
    #[serde(default)]
    pub touched_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub checkpoints: Vec<String>,
}

impl FileHistoryState {
    /// Record that a path was touched in the current run.
    pub fn note_path(&mut self, path: impl Into<PathBuf>) {
        self.touched_paths.insert(path.into());
    }

    /// Record a named checkpoint.
    pub fn note_checkpoint(&mut self, checkpoint: impl Into<String>) {
        self.checkpoints.push(checkpoint.into());
    }
}

/// Shared application state snapshot for TUI/GUI/remote surfaces.
///
/// Extended to match Claude Code's `AppState` with 30+ fields covering
/// session management, MCP connections, task tracking, denial history,
/// settings, and more.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    // --- Core session ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_agent_id: Option<AgentId>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub discovered_skills: BTreeSet<String>,
    #[serde(default)]
    pub active_tools: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub queued_task_count: usize,

    // --- Extended fields (Phase 42d, P0 #13-14) ---
    /// Current working directory for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Connected MCP server names.
    #[serde(default)]
    pub mcp_servers: BTreeSet<String>,
    /// Active conversation ID (set by provider response).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Total tokens used in this session.
    #[serde(default)]
    pub total_tokens_used: u64,
    /// Total cost in USD for this session.
    #[serde(default)]
    pub total_cost_usd: f64,
    /// Number of API calls made.
    #[serde(default)]
    pub api_call_count: u32,
    /// Number of completed turns.
    #[serde(default)]
    pub turn_count: u32,
    /// Whether the session is in plan mode.
    #[serde(default)]
    pub is_plan_mode: bool,
    /// Whether the session is in coordinator mode.
    #[serde(default)]
    pub is_coordinator_mode: bool,
    /// Permission mode before entering plan mode (for restoration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_plan_permission_mode: Option<PermissionMode>,
    /// Number of consecutive permission denials.
    #[serde(default)]
    pub consecutive_denials: u32,
    /// Tool names that have been denied in this session.
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,
    /// Whether verbose/debug mode is active.
    #[serde(default)]
    pub is_verbose: bool,
    /// Currently executing tool use IDs.
    #[serde(default)]
    pub in_progress_tool_use_ids: BTreeSet<String>,
    /// Whether there is an interruptible tool in progress.
    #[serde(default)]
    pub has_interruptible_tool_in_progress: bool,
    /// Active task IDs.
    #[serde(default)]
    pub active_task_ids: BTreeSet<String>,
    /// Completed task count.
    #[serde(default)]
    pub completed_task_count: u32,
    /// Failed task count.
    #[serde(default)]
    pub failed_task_count: u32,
    /// Loaded memory file paths (CLAUDE.md etc.).
    #[serde(default)]
    pub loaded_memory_paths: BTreeSet<PathBuf>,
    /// Active output style name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,
    /// Current theme name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Query source (e.g., "user", "resume", "api").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_source: Option<String>,
    /// Feature flags enabled for this session.
    #[serde(default)]
    pub feature_flags: BTreeSet<String>,
    /// Whether the session has been compacted at least once.
    #[serde(default)]
    pub has_been_compacted: bool,
    /// Number of compactions performed.
    #[serde(default)]
    pub compaction_count: u32,
    /// Attribution state for commit tracking.
    #[serde(default)]
    pub file_history: FileHistoryState,
    /// Additional working directories (e.g., for git worktrees).
    #[serde(default)]
    pub additional_working_directories: BTreeSet<PathBuf>,
}

impl AppState {
    /// Push a new message into the state snapshot.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Record a newly discovered skill slug.
    pub fn note_skill(&mut self, skill: impl Into<String>) {
        self.discovered_skills.insert(skill.into());
    }

    /// Record an active tool.
    pub fn note_tool(&mut self, tool_name: impl Into<String>) {
        self.active_tools.insert(tool_name.into());
    }

    /// Record a permission denial for a tool.
    pub fn record_denial(&mut self, tool_name: &str) {
        self.consecutive_denials += 1;
        self.denied_tools.insert(tool_name.to_owned());
    }

    /// Reset consecutive denial counter (e.g., after a successful tool use).
    pub fn reset_denial_counter(&mut self) {
        self.consecutive_denials = 0;
    }

    /// Record an API call.
    pub fn record_api_call(&mut self, tokens: u64, cost_usd: f64) {
        self.api_call_count += 1;
        self.total_tokens_used += tokens;
        self.total_cost_usd += cost_usd;
    }

    /// Record a completed turn.
    pub fn record_turn(&mut self) {
        self.turn_count += 1;
    }

    /// Record a compaction event.
    pub fn record_compaction(&mut self) {
        self.has_been_compacted = true;
        self.compaction_count += 1;
    }

    /// Add an MCP server connection.
    pub fn add_mcp_server(&mut self, name: impl Into<String>) {
        self.mcp_servers.insert(name.into());
    }

    /// Remove an MCP server connection.
    pub fn remove_mcp_server(&mut self, name: &str) {
        self.mcp_servers.remove(name);
    }

    /// Add an active task.
    pub fn add_active_task(&mut self, task_id: impl Into<String>) {
        self.active_task_ids.insert(task_id.into());
    }

    /// Complete a task.
    pub fn complete_task(&mut self, task_id: &str, success: bool) {
        self.active_task_ids.remove(task_id);
        if success {
            self.completed_task_count += 1;
        } else {
            self.failed_task_count += 1;
        }
    }

    /// Enter plan mode.
    pub fn enter_plan_mode(&mut self) {
        if !self.is_plan_mode {
            self.pre_plan_permission_mode = Some(self.permission_mode);
            self.is_plan_mode = true;
        }
    }

    /// Exit plan mode.
    pub fn exit_plan_mode(&mut self) {
        if self.is_plan_mode {
            self.is_plan_mode = false;
            if let Some(prev) = self.pre_plan_permission_mode.take() {
                self.permission_mode = prev;
            }
        }
    }

    /// Add a loaded memory file path.
    pub fn add_loaded_memory(&mut self, path: impl Into<PathBuf>) {
        self.loaded_memory_paths.insert(path.into());
    }

    /// Check if a memory file has been loaded.
    pub fn is_memory_loaded(&self, path: &PathBuf) -> bool {
        self.loaded_memory_paths.contains(path)
    }

    /// Enable a feature flag.
    pub fn enable_feature(&mut self, flag: impl Into<String>) {
        self.feature_flags.insert(flag.into());
    }

    /// Check if a feature flag is enabled.
    pub fn is_feature_enabled(&self, flag: &str) -> bool {
        self.feature_flags.contains(flag)
    }

    /// Count total fields (for testing coverage).
    ///
    /// This must be kept in sync with the number of fields defined in
    /// [`AppState`]. When adding or removing a field, update this constant.
    pub fn field_count() -> usize {
        35 // session_id, active_agent_id, permission_mode, messages,
        // discovered_skills, active_tools, model, queued_task_count,
        // cwd, mcp_servers, conversation_id, total_tokens_used,
        // total_cost_usd, api_call_count, turn_count, is_plan_mode,
        // is_coordinator_mode, pre_plan_permission_mode,
        // consecutive_denials, denied_tools, is_verbose,
        // in_progress_tool_use_ids, has_interruptible_tool_in_progress,
        // active_task_ids, completed_task_count, failed_task_count,
        // loaded_memory_paths, output_style, theme, query_source,
        // feature_flags, has_been_compacted, compaction_count,
        // file_history, additional_working_directories
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::AppState;
    use crate::{ConversationEntry, Message};

    #[test]
    fn app_state_tracks_messages_skills_and_tools() {
        let mut state = AppState::default();
        state.push_message(Message::from(ConversationEntry::user("hello")));
        state.note_skill("openai-docs");
        state.note_tool("read_file");

        assert_eq!(state.messages.len(), 1);
        assert!(state.discovered_skills.contains("openai-docs"));
        assert!(state.active_tools.contains("read_file"));
    }

    #[test]
    fn file_history_tracks_paths_and_checkpoints() {
        let mut history = super::FileHistoryState::default();
        history.note_path("src/main.rs");
        history.note_checkpoint("before_compact");

        assert!(
            history
                .touched_paths
                .iter()
                .any(|path| path.ends_with("src/main.rs"))
        );
        assert_eq!(history.checkpoints, vec!["before_compact"]);
    }

    // ---- Extended AppState tests ----

    #[test]
    fn denial_tracking() {
        let mut state = AppState::default();
        state.record_denial("bash");
        state.record_denial("bash");
        assert_eq!(state.consecutive_denials, 2);
        assert!(state.denied_tools.contains("bash"));
        state.reset_denial_counter();
        assert_eq!(state.consecutive_denials, 0);
    }

    #[test]
    fn api_call_tracking() {
        let mut state = AppState::default();
        state.record_api_call(1000, 0.05);
        state.record_api_call(2000, 0.10);
        assert_eq!(state.api_call_count, 2);
        assert_eq!(state.total_tokens_used, 3000);
        assert!((state.total_cost_usd - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn turn_tracking() {
        let mut state = AppState::default();
        state.record_turn();
        state.record_turn();
        assert_eq!(state.turn_count, 2);
    }

    #[test]
    fn compaction_tracking() {
        let mut state = AppState::default();
        assert!(!state.has_been_compacted);
        state.record_compaction();
        assert!(state.has_been_compacted);
        assert_eq!(state.compaction_count, 1);
        state.record_compaction();
        assert_eq!(state.compaction_count, 2);
    }

    #[test]
    fn mcp_server_management() {
        let mut state = AppState::default();
        state.add_mcp_server("memory");
        state.add_mcp_server("context7");
        assert!(state.mcp_servers.contains("memory"));
        assert!(state.mcp_servers.contains("context7"));
        state.remove_mcp_server("memory");
        assert!(!state.mcp_servers.contains("memory"));
        assert!(state.mcp_servers.contains("context7"));
    }

    #[test]
    fn task_lifecycle() {
        let mut state = AppState::default();
        state.add_active_task("task-1");
        state.add_active_task("task-2");
        assert!(state.active_task_ids.contains("task-1"));
        assert_eq!(state.active_task_ids.len(), 2);
        state.complete_task("task-1", true);
        assert!(!state.active_task_ids.contains("task-1"));
        assert_eq!(state.completed_task_count, 1);
        state.complete_task("task-2", false);
        assert_eq!(state.failed_task_count, 1);
    }

    #[test]
    fn plan_mode_toggle() {
        let mut state = AppState::default();
        assert!(!state.is_plan_mode);
        state.enter_plan_mode();
        assert!(state.is_plan_mode);
        assert!(state.pre_plan_permission_mode.is_some());
        state.exit_plan_mode();
        assert!(!state.is_plan_mode);
        assert!(state.pre_plan_permission_mode.is_none());
    }

    #[test]
    fn memory_loading() {
        let mut state = AppState::default();
        let path = std::path::PathBuf::from("CLAUDE.md");
        state.add_loaded_memory(&path);
        assert!(state.is_memory_loaded(&path));
        assert!(!state.is_memory_loaded(&std::path::PathBuf::from("OTHER.md")));
    }

    #[test]
    fn feature_flags() {
        let mut state = AppState::default();
        state.enable_feature("tool_search");
        assert!(state.is_feature_enabled("tool_search"));
        assert!(!state.is_feature_enabled("nonexistent"));
    }

    #[test]
    fn extended_fields_default() {
        let state = AppState::default();
        assert!(state.cwd.is_none());
        assert!(state.mcp_servers.is_empty());
        assert!(state.conversation_id.is_none());
        assert_eq!(state.total_tokens_used, 0);
        assert_eq!(state.total_cost_usd, 0.0);
        assert_eq!(state.api_call_count, 0);
        assert_eq!(state.turn_count, 0);
        assert!(!state.is_plan_mode);
        assert!(!state.is_coordinator_mode);
        assert!(state.pre_plan_permission_mode.is_none());
        assert_eq!(state.consecutive_denials, 0);
        assert!(state.denied_tools.is_empty());
        assert!(!state.is_verbose);
        assert!(state.in_progress_tool_use_ids.is_empty());
        assert!(!state.has_interruptible_tool_in_progress);
        assert!(state.active_task_ids.is_empty());
        assert_eq!(state.completed_task_count, 0);
        assert_eq!(state.failed_task_count, 0);
        assert!(state.loaded_memory_paths.is_empty());
        assert!(state.output_style.is_none());
        assert!(state.theme.is_none());
        assert!(state.query_source.is_none());
        assert!(state.feature_flags.is_empty());
        assert!(!state.has_been_compacted);
        assert_eq!(state.compaction_count, 0);
        assert!(state.additional_working_directories.is_empty());
    }

    #[test]
    fn field_count_matches() {
        assert_eq!(AppState::field_count(), 35);
    }

    #[test]
    fn serde_roundtrip_extended() {
        let mut state = AppState {
            cwd: Some(PathBuf::from("/home/user/project")),
            model: Some("claude-3.5-sonnet".to_owned()),
            ..AppState::default()
        };
        state.add_mcp_server("memory");
        state.enable_feature("tool_search");
        state.record_api_call(500, 0.02);
        state.is_coordinator_mode = true;

        let json = serde_json::to_string(&state).expect("serialize state");
        let parsed: AppState = serde_json::from_str(&json).expect("deserialize state");
        assert_eq!(parsed.cwd, Some(PathBuf::from("/home/user/project")));
        assert_eq!(parsed.model, Some("claude-3.5-sonnet".to_owned()));
        assert!(parsed.mcp_servers.contains("memory"));
        assert!(parsed.is_feature_enabled("tool_search"));
        assert_eq!(parsed.api_call_count, 1);
        assert!(parsed.is_coordinator_mode);
    }
}
