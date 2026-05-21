//! Built-in tool registry and execution engine.
//!
//! Provides 30+ built-in tools (file I/O, search, shell, web, LSP, tasks, etc.)
//! with a [`ToolRegistry`] that supports BM25-based tool search and OpenAI /
//! Anthropic schema generation.

pub mod agent;
pub mod command;
pub mod delegate;
pub mod discover_skills;
pub mod file_ops;
pub mod file_state_cache;
pub mod git;
pub mod hooks;
pub mod lsp;
pub mod mcp_catalog;
pub mod mcp_output_storage;
pub mod mcp_runtime;
pub mod mcp_tools;
pub mod misc;
pub mod plan_mode;
pub mod review_artifact;
pub mod runtime_plan_mode;
pub mod sandbox;
pub mod search;
pub mod send_message;
pub mod send_user_file;
pub mod shell;
pub mod specs;
pub mod streaming_executor;
pub mod system;
pub mod task_output;
pub mod tasks;
pub mod team_runtime;
pub mod team_tools;
pub mod tool_hooks;
pub mod tool_orchestration;
pub mod tool_progress;
pub mod tool_prompts;
pub mod tool_result_storage;
pub mod tool_result_summary;
pub mod web;
pub mod web_browser;
pub mod workflow;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use anyhow::{Result, anyhow};
use claude_config::ActiveWorktreeSession;
use claude_context::RuntimeIdentityContext;
use claude_core::task_stack::TaskStack;
use claude_core::{
    ConversationEntry, ConversationRole, HookEvent, HookShell, SubAgentCompletion, ToolCall,
    ToolResult,
};
use claude_permissions::{
    PermissionBroker, PermissionClass, PermissionDecision, PermissionRequest, auto_allows,
    classify_tool,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use file_state_cache::{FileState, FileStateCache};

// Re-export specs::builtin_tool_specs at crate root for backward compatibility.
pub use specs::builtin_tool_specs;

// Re-export hooks::execute_command_hook at crate root for backward compatibility.
pub use hooks::execute_command_hook;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "dist",
    "coverage",
    ".next",
];

static TOOL_RUNTIME_POLICY: Lazy<Mutex<ToolRuntimePolicy>> =
    Lazy::new(|| Mutex::new(ToolRuntimePolicy::default()));

#[derive(Debug, Clone, Default)]
pub(crate) struct ToolRuntimeHints {
    pub always_load: bool,
    pub search_hints: Vec<String>,
    pub destructive_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

static TOOL_RUNTIME_HINTS: Lazy<Mutex<std::collections::BTreeMap<String, ToolRuntimeHints>>> =
    Lazy::new(|| Mutex::new(std::collections::BTreeMap::new()));

pub(crate) fn register_tool_runtime_hints(name: impl Into<String>, hints: ToolRuntimeHints) {
    TOOL_RUNTIME_HINTS.lock().insert(name.into(), hints);
}

pub(crate) fn clear_tool_runtime_hints() {
    TOOL_RUNTIME_HINTS.lock().clear();
}

fn runtime_hints_for_tool(name: &str) -> Option<ToolRuntimeHints> {
    TOOL_RUNTIME_HINTS.lock().get(name).cloned()
}

tokio::task_local! {
    static TOOL_RUNTIME_POLICY_OVERLAY: ToolRuntimePolicyOverlay;
}

tokio::task_local! {
    static TOOL_RUNTIME_MCP_STATE_PROVIDER: Arc<RuntimeMcpStateProvider>;
}

tokio::task_local! {
    static TOOL_RUNTIME_MCP_OBSERVATION_PROVIDER: Arc<RuntimeMcpObservationProvider>;
}

tokio::task_local! {
    static TOOL_RUNTIME_AGENT_PROMPT_CONTEXT_PROVIDER: Arc<RuntimeAgentPromptContextProvider>;
}

tokio::task_local! {
    static TOOL_RUNTIME_FORK_SNAPSHOT_PROVIDER: Arc<RuntimeForkSnapshotProvider>;
}

pub type RuntimeMcpStateProvider = dyn Fn() -> claude_mcp::McpCliState + Send + Sync;
pub type RuntimeMcpObservationProvider =
    dyn Fn() -> mcp_runtime::RuntimeMcpObservation + Send + Sync;
pub type RuntimeAgentPromptContextProvider = dyn Fn() -> RuntimeAgentPromptContext + Send + Sync;
pub type RuntimeForkSnapshotProvider = dyn Fn() -> claude_core::SubAgentForkSnapshot + Send + Sync;

/// Task-local context used to build the dynamic Agent tool prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeAgentPromptContext {
    #[serde(default)]
    pub user_agents_dir: Option<PathBuf>,
    #[serde(default)]
    pub project_agents_dir: Option<PathBuf>,
    #[serde(default)]
    pub additional_working_directories: Vec<PathBuf>,
    #[serde(default)]
    pub allowed_agent_types: Option<Vec<String>>,
    #[serde(default)]
    pub denied_agent_types: Vec<String>,
    #[serde(default)]
    pub is_coordinator: bool,
    #[serde(default)]
    pub is_non_interactive: bool,
    #[serde(default)]
    pub list_via_attachment: bool,
    #[serde(default)]
    pub runtime_identity: RuntimeIdentityContext,
    #[serde(default)]
    pub scratchpad_dir: Option<PathBuf>,
    #[serde(default)]
    pub session_memory_dir: Option<PathBuf>,
    #[serde(default)]
    pub tasks_dir: Option<PathBuf>,
    #[serde(default)]
    pub tool_results_dir: Option<PathBuf>,
    #[serde(default)]
    pub auto_memory_dir: Option<PathBuf>,
    #[serde(default)]
    pub auto_memory_read_dir: Option<PathBuf>,
    #[serde(default)]
    pub team_memory_read_dir: Option<PathBuf>,
    #[serde(default)]
    pub project_temp_dir: Option<PathBuf>,
    #[serde(default)]
    pub preview_launch_config_path: Option<PathBuf>,
    #[serde(default)]
    pub teams_dir: Option<PathBuf>,
    #[serde(default)]
    pub agent_memory_dirs: Vec<PathBuf>,
}

/// Process-scoped runtime policy for tool exposure and task artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMcpServerPolicyEntry {
    pub origin_kind: String,
    pub origin_name: String,
    pub config_path: PathBuf,
    pub server: claude_mcp::McpServerConfig,
}

/// Process-scoped runtime policy for tool exposure and task artifacts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolRuntimePolicy {
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub task_output_dir: Option<PathBuf>,
    #[serde(default)]
    pub tasks_dir: Option<PathBuf>,
    #[serde(default)]
    pub tool_results_dir: Option<PathBuf>,
    #[serde(default)]
    pub mcp_servers: Vec<RuntimeMcpServerPolicyEntry>,
    #[serde(default)]
    pub shell_policy: shell::ShellExecutionPolicy,
}

/// Task-local overlay for a single query/agent run.
///
/// This mirrors Claude Code's per-run tool-pool filtering without mutating the
/// process-wide policy, so concurrent background agents keep independent tool
/// surfaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolRuntimePolicyOverlay {
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
}

/// Configure the current process-wide tool policy.
///
/// # Errors
/// Returns an error if task-output persistence cannot be configured.
pub fn configure_tool_runtime_policy(policy: ToolRuntimePolicy) -> Result<()> {
    tasks::configure_task_output_dir(policy.task_output_dir.clone())?;
    tasks::configure_task_list_base_dir(policy.tasks_dir.clone())?;
    let mut current = TOOL_RUNTIME_POLICY.lock();
    *current = normalize_tool_runtime_policy(policy);
    Ok(())
}

/// Return the active process-wide tool policy.
#[must_use]
pub fn current_tool_runtime_policy() -> ToolRuntimePolicy {
    let base = TOOL_RUNTIME_POLICY.lock().clone();
    TOOL_RUNTIME_POLICY_OVERLAY
        .try_with(|overlay| apply_tool_runtime_policy_overlay(base.clone(), overlay.clone()))
        .unwrap_or(base)
}

pub async fn with_tool_runtime_policy_overlay<F, T>(
    overlay: ToolRuntimePolicyOverlay,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    TOOL_RUNTIME_POLICY_OVERLAY
        .scope(normalize_tool_runtime_policy_overlay(overlay), future)
        .await
}

pub async fn with_runtime_mcp_state_provider<F, T>(
    provider: Arc<RuntimeMcpStateProvider>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    TOOL_RUNTIME_MCP_STATE_PROVIDER
        .scope(provider, future)
        .await
}

pub async fn with_runtime_mcp_observation_provider<F, T>(
    provider: Arc<RuntimeMcpObservationProvider>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    TOOL_RUNTIME_MCP_OBSERVATION_PROVIDER
        .scope(provider, future)
        .await
}

pub async fn with_runtime_agent_prompt_context_provider<F, T>(
    provider: Arc<RuntimeAgentPromptContextProvider>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    TOOL_RUNTIME_AGENT_PROMPT_CONTEXT_PROVIDER
        .scope(provider, future)
        .await
}

pub async fn with_runtime_fork_snapshot_provider<F, T>(
    provider: Arc<RuntimeForkSnapshotProvider>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    TOOL_RUNTIME_FORK_SNAPSHOT_PROVIDER
        .scope(provider, future)
        .await
}

#[must_use]
pub fn current_runtime_mcp_cli_state() -> Option<claude_mcp::McpCliState> {
    TOOL_RUNTIME_MCP_STATE_PROVIDER
        .try_with(|provider| provider())
        .ok()
}

#[must_use]
pub fn current_runtime_mcp_observation() -> Option<mcp_runtime::RuntimeMcpObservation> {
    TOOL_RUNTIME_MCP_OBSERVATION_PROVIDER
        .try_with(|provider| provider())
        .ok()
}

#[must_use]
pub fn current_runtime_agent_prompt_context() -> Option<RuntimeAgentPromptContext> {
    TOOL_RUNTIME_AGENT_PROMPT_CONTEXT_PROVIDER
        .try_with(|provider| provider())
        .ok()
}

#[must_use]
pub fn current_runtime_fork_snapshot() -> Option<claude_core::SubAgentForkSnapshot> {
    TOOL_RUNTIME_FORK_SNAPSHOT_PROVIDER
        .try_with(|provider| provider())
        .ok()
}

fn apply_tool_runtime_policy_overlay(
    mut base: ToolRuntimePolicy,
    overlay: ToolRuntimePolicyOverlay,
) -> ToolRuntimePolicy {
    let overlay = normalize_tool_runtime_policy_overlay(overlay);
    if let Some(overlay_allowed) = overlay.allowed_tools {
        base.allowed_tools = if base.allowed_tools.is_empty() {
            overlay_allowed
        } else {
            base.allowed_tools
                .into_iter()
                .filter(|tool| overlay_allowed.contains(tool))
                .collect()
        };
    }
    base.disallowed_tools.extend(overlay.disallowed_tools);
    normalize_tool_runtime_policy(base)
}

fn normalize_tool_runtime_policy_overlay(
    mut overlay: ToolRuntimePolicyOverlay,
) -> ToolRuntimePolicyOverlay {
    overlay.allowed_tools = overlay.allowed_tools.map(|tools| {
        let mut normalized = tools
            .into_iter()
            .map(|tool| tool.trim().to_ascii_lowercase())
            .filter(|tool| !tool.is_empty())
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        normalized
    });
    overlay.disallowed_tools = overlay
        .disallowed_tools
        .into_iter()
        .map(|tool| tool.trim().to_ascii_lowercase())
        .filter(|tool| !tool.is_empty())
        .collect::<Vec<_>>();
    overlay.disallowed_tools.sort();
    overlay.disallowed_tools.dedup();
    overlay
}

/// Return the built-in tool specs visible under the active runtime policy.
#[must_use]
pub fn runtime_builtin_tool_specs() -> Vec<ToolSpec> {
    let policy = current_tool_runtime_policy();
    builtin_tool_specs()
        .into_iter()
        .filter(|spec| tool_allowed_by_policy(&spec.name, &policy))
        .collect()
}

fn sort_tool_specs_by_name(specs: &mut [ToolSpec]) {
    specs.sort_by(|left, right| left.name.cmp(&right.name));
}

fn merge_provider_tool_specs(
    mut builtin_specs: Vec<ToolSpec>,
    mut dynamic_specs: Vec<ToolSpec>,
) -> Vec<ToolSpec> {
    sort_tool_specs_by_name(&mut builtin_specs);
    sort_tool_specs_by_name(&mut dynamic_specs);

    let mut merged = Vec::with_capacity(builtin_specs.len() + dynamic_specs.len());
    let mut seen = std::collections::BTreeSet::new();

    for spec in builtin_specs.into_iter().chain(dynamic_specs) {
        if seen.insert(spec.name.clone()) {
            merged.push(spec);
        }
    }

    merged
}

#[must_use]
pub fn is_runtime_dynamic_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__") || matches!(name, "list_mcp_resources" | "read_mcp_resource")
}

fn runtime_policy_supports_mcp_resources() -> bool {
    if let Some(observation) = current_runtime_mcp_observation() {
        return observation.servers.iter().any(|server| {
            server.entry.server.enabled
                && (server.entry.server.capabilities.supports_resources
                    || server.inspection.as_ref().is_some_and(|inspection| {
                        inspection.capabilities.get("resources").is_some()
                    }))
        });
    }
    current_tool_runtime_policy()
        .mcp_servers
        .iter()
        .any(|entry| entry.server.enabled && entry.server.capabilities.supports_resources)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceKind {
    Builtin,
    Mcp,
    McpResource,
}

pub const DEFAULT_TOOL_MAX_RESULT_SIZE_CHARS: usize = 100_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultSizePolicy {
    Finite(usize),
    NeverPersist,
}

impl Default for ToolResultSizePolicy {
    fn default() -> Self {
        Self::Finite(DEFAULT_TOOL_MAX_RESULT_SIZE_CHARS)
    }
}

impl ToolResultSizePolicy {
    #[must_use]
    pub fn finite(chars: usize) -> Self {
        Self::Finite(chars)
    }

    #[must_use]
    pub fn is_never_persist(self) -> bool {
        matches!(self, Self::NeverPersist)
    }
}

const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
const TOOL_SEARCH_COMPAT_NAME: &str = "toolsearch";

fn normalize_tool_alias_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn canonical_builtin_tool_name(internal_name: &str) -> Option<&'static str> {
    match internal_name {
        "list_directory" => Some("LS"),
        "read_file" => Some("Read"),
        "write_file" => Some("Write"),
        "replace_in_file" => Some("ReplaceInFile"),
        "edit_file" => Some("Edit"),
        "bash_command" => Some("Bash"),
        "glob" => Some("Glob"),
        "grep" => Some("Grep"),
        "web_fetch" => Some("WebFetch"),
        "ask_user" => Some("AskUserQuestion"),
        "todo_write" => Some("TodoWrite"),
        "config_read" => Some("Config"),
        "agent" => Some("Agent"),
        "web_search" => Some("WebSearch"),
        "lsp" => Some("LSP"),
        "task_create" => Some("TaskCreate"),
        "task_get" => Some("TaskGet"),
        "task_list" => Some("TaskList"),
        "task_update" => Some("TaskUpdate"),
        "task_output" => Some("TaskOutput"),
        "task_stop" => Some("TaskStop"),
        "notebook_edit" => Some("NotebookEdit"),
        "discover_skills" => Some("DiscoverSkills"),
        "skill_execute" => Some("Skill"),
        "send_message" => Some("SendMessage"),
        "enter_plan_mode" => Some("EnterPlanMode"),
        "exit_plan_mode" => Some("ExitPlanMode"),
        "sleep" => Some("Sleep"),
        "snip" => Some("snip"),
        "team_create" => Some("TeamCreate"),
        "team_status" => Some("TeamStatus"),
        "web_browser" => Some("WebBrowser"),
        "tool_search" => Some("ToolSearch"),
        "verify_plan" => Some("verify_plan_execution"),
        "terminal_capture" => Some("terminal_capture"),
        "powershell" => Some("PowerShell"),
        "repl" => Some("REPL"),
        "monitor" => Some("Monitor"),
        "schedule_cron" => Some("CronCreate"),
        "cron_delete" => Some("CronDelete"),
        "cron_list" => Some("CronList"),
        "remote_trigger" => Some("RemoteTrigger"),
        "workflow" => Some("workflow"),
        "suggest_pr" => Some("SuggestPR"),
        "enter_worktree" => Some("EnterWorktree"),
        "exit_worktree" => Some("ExitWorktree"),
        "list_worktrees" => Some("ListWorktrees"),
        "brief" => Some("SendUserMessage"),
        "ctx_inspect" => Some("CtxInspect"),
        "list_peers" => Some("ListPeers"),
        "tungsten" => Some("Tungsten"),
        "overflow_test" => Some("OverflowTest"),
        "synthetic_output" => Some("SyntheticOutput"),
        "mcp_auth" => Some("McpAuth"),
        "mcp_call" => Some("McpCall"),
        "voice_input" => Some("VoiceInput"),
        "daemon" => Some("Daemon"),
        "team_delete" => Some("TeamDelete"),
        "team_list" => Some("TeamList"),
        "broadcast_message" => Some("BroadcastMessage"),
        "review_artifact" => Some("ReviewArtifact"),
        "send_user_file" => Some("send_user_file"),
        "list_mcp_resources" => Some("ListMcpResourcesTool"),
        "read_mcp_resource" => Some("ReadMcpResourceTool"),
        _ => None,
    }
}

fn builtin_tool_aliases(internal_name: &str) -> &'static [&'static str] {
    match internal_name {
        "list_directory" => &["ListDirectory", "LS"],
        "read_file" => &["ReadFile", "Read"],
        "write_file" => &["WriteFile", "Write"],
        "replace_in_file" => &["ReplaceInFile"],
        "edit_file" => &["EditFile", "Edit"],
        "agent" => &["Task"],
        "skill_execute" => &["ExecuteSkill"],
        "tool_search" => &["ToolSearch", "tool_search", "toolsearch"],
        "verify_plan" => &["VerifyPlan"],
        "terminal_capture" => &["TerminalCapture"],
        "schedule_cron" => &["ScheduleCron", "CronCreate"],
        "cron_delete" => &["CronDelete"],
        "cron_list" => &["CronList"],
        "brief" => &["Brief"],
        "list_mcp_resources" => &["ListMcpResources", "ListMcpResourcesTool"],
        "read_mcp_resource" => &["ReadMcpResource", "ReadMcpResourceTool"],
        _ => &[],
    }
}

fn builtin_tool_is_deferred(name: &str) -> bool {
    matches!(
        name,
        "ask_user"
            | "config_read"
            | "enter_plan_mode"
            | "exit_plan_mode"
            | "enter_worktree"
            | "exit_worktree"
            | "lsp"
            | "notebook_edit"
            | "todo_write"
            | "task_create"
            | "task_get"
            | "task_list"
            | "task_update"
            | "task_output"
            | "send_message"
            | "team_create"
            | "team_delete"
            | "web_fetch"
            | "web_search"
            | "remote_trigger"
            | "schedule_cron"
            | "cron_delete"
            | "cron_list"
            | "list_mcp_resources"
            | "read_mcp_resource"
    )
}

fn builtin_tool_search_hints(name: &str) -> &'static [&'static str] {
    match name {
        "agent" => &["delegate work to a subagent"],
        "ask_user" => &["prompt the user with a multiple-choice question"],
        "config_read" => &["get or set remote-code settings"],
        "enter_plan_mode" => &["switch to plan mode to design an approach before coding"],
        "exit_plan_mode" => &["present plan for approval and start coding"],
        "enter_worktree" => &["create an isolated git worktree and switch into it"],
        "exit_worktree" => &["exit a worktree session and return to the original directory"],
        "glob" => &["find files by name pattern or wildcard"],
        "grep" => &["search file contents with regex ripgrep"],
        "lsp" => &["code intelligence definitions references symbols hover"],
        "cron_delete" => &["cancel scheduled cron job"],
        "cron_list" => &["list scheduled cron jobs"],
        "notebook_edit" => &["edit jupyter notebook cells ipynb"],
        "read_file" => &["read files images pdfs notebooks"],
        "remote_trigger" => &["manage scheduled remote agent triggers"],
        "send_message" => &["send messages to agent teammates swarm protocol"],
        "task_create" => &["create a task in the task list"],
        "task_get" => &["retrieve a task by id"],
        "task_list" => &["list all tasks"],
        "task_output" => &["read output logs from a background task"],
        "task_update" => &["update a task"],
        "task_stop" => &["stop a running background task"],
        "team_create" => &["create a multi-agent swarm team"],
        "team_delete" => &["disband a swarm team and clean up"],
        "todo_write" => &["manage the session task checklist"],
        "web_fetch" => &["fetch and extract content from a url"],
        "web_search" => &["search the web for current information"],
        "list_mcp_resources" => &["list resources from connected mcp servers"],
        "read_mcp_resource" => &["read a specific mcp resource by uri"],
        _ => &[],
    }
}

fn collect_tool_search_terms(raw: &str, terms: &mut std::collections::BTreeSet<String>) {
    if raw.trim().is_empty() {
        return;
    }
    terms.insert(raw.to_owned());
    for token in raw
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| !token.is_empty())
    {
        terms.insert(token.to_ascii_lowercase());
    }
}

fn is_tool_search_tool_name(name: &str) -> bool {
    matches!(name, TOOL_SEARCH_TOOL_NAME | TOOL_SEARCH_COMPAT_NAME)
        || normalize_tool_alias_key(name) == "toolsearch"
}

pub async fn runtime_provider_tool_specs() -> Vec<ToolSpec> {
    let builtin_specs = runtime_builtin_tool_specs();
    let catalog = mcp_catalog::runtime_mcp_catalog().await;
    let policy = current_tool_runtime_policy();
    let mut dynamic_specs = Vec::new();

    if catalog
        .clients
        .iter()
        .any(|client| client.supports_resources)
    {
        dynamic_specs.extend(
            specs::mcp_resource_tool_specs()
                .into_iter()
                .filter(|spec| tool_allowed_by_policy(&spec.name, &policy)),
        );
    }

    dynamic_specs.extend(catalog.tools.into_iter().map(|tool| tool.tool_spec));

    merge_provider_tool_specs(builtin_specs, dynamic_specs)
}

pub async fn runtime_tool_search_candidate_specs() -> Vec<ToolSpec> {
    runtime_provider_tool_specs()
        .await
        .into_iter()
        .filter(|spec| spec.is_deferred())
        .collect()
}

pub fn extract_discovered_tool_names_from_conversation(
    conversation: &[ConversationEntry],
) -> std::collections::BTreeSet<String> {
    extract_discovered_tool_names(conversation, &std::collections::BTreeSet::new())
}

#[must_use]
pub fn extract_discovered_tool_names(
    conversation: &[ConversationEntry],
    carried_discovered_tools: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let mut discovered = carried_discovered_tools.clone();

    for entry in conversation {
        if entry.role != ConversationRole::Tool {
            continue;
        }
        let Some(tool_name) = entry.name.as_deref() else {
            continue;
        };
        if !is_tool_search_tool_name(tool_name) {
            continue;
        }

        for block in &entry.content_blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_reference") {
                continue;
            }
            if let Some(name) = block.get("tool_name").and_then(Value::as_str) {
                discovered
                    .insert(normalize_provider_tool_name(name).unwrap_or_else(|| name.to_owned()));
            }
        }

        let Ok(payload) = serde_json::from_str::<Value>(&entry.text) else {
            continue;
        };

        if let Some(matches) = payload
            .get("data")
            .and_then(|value| value.get("matches"))
            .and_then(Value::as_array)
        {
            for tool_name in matches.iter().filter_map(Value::as_str) {
                discovered.insert(
                    normalize_provider_tool_name(tool_name).unwrap_or_else(|| tool_name.to_owned()),
                );
            }
        }

        for array_key in ["results", "found_tools"] {
            let Some(results) = payload.get(array_key).and_then(Value::as_array) else {
                continue;
            };
            for result in results {
                if let Some(name) = result.get("name").and_then(Value::as_str) {
                    discovered.insert(
                        normalize_provider_tool_name(name).unwrap_or_else(|| name.to_owned()),
                    );
                }
            }
        }
    }

    discovered
}

pub async fn runtime_visible_provider_tool_specs(
    conversation: &[ConversationEntry],
) -> Vec<ToolSpec> {
    runtime_visible_provider_tool_specs_with_discovered_tools(
        conversation,
        &std::collections::BTreeSet::new(),
    )
    .await
}

pub async fn runtime_visible_provider_tool_specs_with_discovered_tools(
    conversation: &[ConversationEntry],
    carried_discovered_tools: &std::collections::BTreeSet<String>,
) -> Vec<ToolSpec> {
    let specs = runtime_provider_tool_specs().await;
    let has_tool_search = specs.iter().any(|spec| spec.is_tool_search());
    if !has_tool_search {
        return specs;
    }

    let has_deferred_tools = specs.iter().any(ToolSpec::is_deferred);
    if !has_deferred_tools {
        return specs
            .into_iter()
            .filter(|spec| !spec.is_tool_search())
            .collect();
    }

    let discovered = extract_discovered_tool_names(conversation, carried_discovered_tools);
    specs
        .into_iter()
        .filter(|spec| {
            spec.is_tool_search()
                || !spec.is_deferred()
                || discovered.contains(spec.name.as_str())
                || discovered.contains(spec.provider_wire_name())
        })
        .collect()
}

pub async fn runtime_provider_tool_spec(name: &str) -> Option<ToolSpec> {
    if matches!(
        normalize_provider_tool_name(name).as_deref(),
        Some("list_mcp_resources" | "read_mcp_resource")
    ) && runtime_policy_supports_mcp_resources()
    {
        return specs::mcp_resource_tool_specs()
            .into_iter()
            .find(|spec| spec.matches_tool_name(name));
    }

    runtime_provider_tool_specs()
        .await
        .into_iter()
        .find(|spec| spec.matches_tool_name(name))
}

/// Normalize provider-facing or legacy tool names to the internal execution name.
#[must_use]
pub fn normalize_provider_tool_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    builtin_tool_specs()
        .into_iter()
        .chain(specs::mcp_resource_tool_specs())
        .find(|spec| spec.matches_tool_name(trimmed))
        .map(|spec| spec.name)
        .or_else(|| {
            if trimmed.starts_with("mcp__") {
                Some(trimmed.to_owned())
            } else {
                None
            }
        })
}

#[must_use]
pub fn provider_wire_tool_name_for(name: &str) -> String {
    builtin_tool_specs()
        .into_iter()
        .chain(specs::mcp_resource_tool_specs())
        .find(|spec| spec.matches_tool_name(name))
        .map_or_else(
            || name.to_owned(),
            |spec| spec.provider_wire_name().to_owned(),
        )
}

#[must_use]
pub fn runtime_tool_result_persistence_skip_names() -> std::collections::HashSet<String> {
    runtime_builtin_tool_specs()
        .into_iter()
        .filter(|spec| spec.tool_result_size_policy().is_never_persist())
        .map(|spec| spec.name)
        .collect()
}

/// Specification for a single built-in tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Internal tool name (e.g. `"read_file"`).
    pub name: String,
    /// Name used in provider protocol messages.
    pub protocol_name: String,
    /// Name used for permission classification.
    pub permission_tool_name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// Whether this tool requires user permission to execute.
    pub requires_permission: bool,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Value,
}

impl ToolSpec {
    #[must_use]
    pub fn tool_result_size_policy(&self) -> ToolResultSizePolicy {
        match self.name.as_str() {
            // Research Read declares maxResultSizeChars: Infinity. Its output is
            // already bounded by file-reading limits; persisting it would create
            // a circular "read this file to read that file" flow.
            "read_file" => ToolResultSizePolicy::NeverPersist,
            "bash_command" | "powershell" => ToolResultSizePolicy::finite(30_000),
            "grep" => ToolResultSizePolicy::finite(20_000),
            "mcp_auth" => ToolResultSizePolicy::finite(10_000),
            "read_mcp_resource" => ToolResultSizePolicy::finite(100_000),
            _ => ToolResultSizePolicy::default(),
        }
    }

    #[must_use]
    pub fn source_kind(&self) -> ToolSourceKind {
        if matches!(
            self.name.as_str(),
            "list_mcp_resources" | "read_mcp_resource"
        ) {
            ToolSourceKind::McpResource
        } else if self.name.starts_with("mcp__") {
            ToolSourceKind::Mcp
        } else {
            ToolSourceKind::Builtin
        }
    }

    #[must_use]
    pub fn is_tool_search(&self) -> bool {
        is_tool_search_tool_name(&self.name)
    }

    #[must_use]
    pub fn is_always_loaded(&self) -> bool {
        self.is_tool_search()
            || runtime_hints_for_tool(&self.name)
                .map(|hints| hints.always_load)
                .unwrap_or(false)
    }

    #[must_use]
    pub fn is_deferred(&self) -> bool {
        if self.is_always_loaded() {
            return false;
        }

        match self.source_kind() {
            ToolSourceKind::Builtin => builtin_tool_is_deferred(&self.name),
            ToolSourceKind::Mcp | ToolSourceKind::McpResource => true,
        }
    }

    #[must_use]
    pub fn destructive_hint(&self) -> Option<bool> {
        runtime_hints_for_tool(&self.name).and_then(|hints| hints.destructive_hint)
    }

    #[must_use]
    pub fn open_world_hint(&self) -> Option<bool> {
        runtime_hints_for_tool(&self.name).and_then(|hints| hints.open_world_hint)
    }

    #[must_use]
    pub fn tool_search_terms(&self) -> Vec<String> {
        let mut terms = std::collections::BTreeSet::new();
        for raw in [
            self.name.as_str(),
            self.protocol_name.as_str(),
            self.permission_tool_name.as_str(),
            self.provider_wire_name(),
        ] {
            collect_tool_search_terms(raw, &mut terms);
        }
        for alias in builtin_tool_aliases(&self.name) {
            collect_tool_search_terms(alias, &mut terms);
        }

        if let Some(stripped) = self.name.strip_prefix("mcp__") {
            for segment in stripped.split("__") {
                collect_tool_search_terms(segment, &mut terms);
            }
        }

        for hint in builtin_tool_search_hints(&self.name) {
            collect_tool_search_terms(hint, &mut terms);
        }

        if let Some(hints) = runtime_hints_for_tool(&self.name) {
            for hint in hints.search_hints {
                collect_tool_search_terms(&hint, &mut terms);
            }
        }

        terms.into_iter().collect()
    }

    /// Provider-facing name exposed in tool schemas and Anthropic tool_use blocks.
    #[must_use]
    pub fn provider_wire_name(&self) -> &str {
        canonical_builtin_tool_name(&self.name).unwrap_or(self.protocol_name.as_str())
    }

    #[must_use]
    pub fn matches_tool_name(&self, requested: &str) -> bool {
        let requested_key = normalize_tool_alias_key(requested);
        if requested_key.is_empty() {
            return false;
        }
        [
            self.name.as_str(),
            self.protocol_name.as_str(),
            self.provider_wire_name(),
        ]
        .into_iter()
        .any(|candidate| normalize_tool_alias_key(candidate) == requested_key)
            || builtin_tool_aliases(&self.name)
                .iter()
                .any(|alias| normalize_tool_alias_key(alias) == requested_key)
    }

    /// Convert to an OpenAI-compatible function-calling schema.
    #[must_use]
    pub fn to_openai_schema(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.provider_wire_name(),
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    /// Convert to an Anthropic-compatible tool-use schema.
    #[must_use]
    pub fn to_anthropic_schema(&self) -> Value {
        self.to_anthropic_schema_with_options(false)
    }

    /// Convert to an Anthropic-compatible tool-use schema with per-request overlays.
    #[must_use]
    pub fn to_anthropic_schema_with_options(&self, defer_loading: bool) -> Value {
        let mut schema = serde_json::json!({
            "name": self.provider_wire_name(),
            "description": self.description,
            "input_schema": self.input_schema,
        });
        if defer_loading {
            schema["defer_loading"] = Value::Bool(true);
        }
        schema
    }
}

/// Callback type for subtask progress events.
///
/// Receives a human-readable status string that frontends can display.
pub type ProgressCallback = dyn Fn(&str) + Send + Sync;

/// Execution context passed to every tool implementation.
#[derive(Clone)]
pub struct ToolExecutionContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Original working directory before any worktree switching.
    pub original_cwd: PathBuf,
    /// Active worktree session for the current runtime, if any.
    pub active_worktree_session: Option<ActiveWorktreeSession>,
    /// Timeout in milliseconds for tool execution.
    pub timeout_ms: u64,
    /// Optional sub-agent completion provider for the agent tool.
    /// When `None`, the agent tool falls back to returning a delegation JSON.
    pub sub_agent: Option<Arc<dyn SubAgentCompletion>>,
    /// Optional progress callback for subtask delegation events.
    /// Frontends (TUI, GUI) provide this to render subtask progress.
    pub progress_cb: Option<Arc<ProgressCallback>>,
    /// Task stack for tracking nested subtask delegation depth.
    /// Shared across tool executions within the same conversation loop.
    pub task_stack: Arc<parking_lot::Mutex<TaskStack>>,
    /// Read-file state cache shared by tools in a single query loop.
    ///
    /// Fork/subagent contexts should use [`FileStateCache::clone_isolated`]
    /// unless a caller explicitly wants to pass a setup cache into the child.
    pub read_file_state: FileStateCache,
    /// Accumulated output tokens from sub-agent executions in this turn.
    /// The query engine reads this after each tool call to track budget carryover.
    pub sub_agent_output_tokens: Arc<std::sync::atomic::AtomicU64>,
}

/// Returns the default bash timeout in milliseconds.
///
/// Reads `BASH_DEFAULT_TIMEOUT_MS` from the environment to override the
/// built-in 120 000 ms (2 minutes) default, and clamps the result against
/// `BASH_MAX_TIMEOUT_MS` (env) or the 600 000 ms (10 minutes) hard ceiling.
fn default_bash_timeout_ms() -> u64 {
    const BUILTIN_DEFAULT: u64 = 120_000;
    const BUILTIN_MAX: u64 = 600_000;
    let val = std::env::var("BASH_DEFAULT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(BUILTIN_DEFAULT);
    let max = std::env::var("BASH_MAX_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(BUILTIN_MAX);
    val.clamp(1_000, max)
}

impl Default for ToolExecutionContext {
    fn default() -> Self {
        Self {
            cwd: PathBuf::new(),
            original_cwd: PathBuf::new(),
            active_worktree_session: None,
            timeout_ms: default_bash_timeout_ms(),
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(TaskStack::default())),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl ToolExecutionContext {
    /// Build a context from a runtime config, preserving worktree session state.
    pub fn from_runtime_config(config: &claude_config::RuntimeConfig) -> Self {
        Self {
            cwd: config.cwd.clone(),
            original_cwd: config.original_cwd.clone(),
            active_worktree_session: config.active_worktree_session.clone(),
            timeout_ms: config.provider.timeout_ms,
            ..Self::default()
        }
    }

    /// Build a test/local context rooted at a specific working directory.
    pub fn from_cwd(cwd: PathBuf) -> Self {
        Self {
            original_cwd: cwd.clone(),
            cwd,
            ..Self::default()
        }
    }

    /// Create an isolated subagent/fork context from this context.
    ///
    /// Mutable read-file state is cloned by value so child reads and writes do
    /// not leak back into the parent unless the caller passes an explicit
    /// override.
    #[must_use]
    pub fn create_subagent_context(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            original_cwd: self.original_cwd.clone(),
            active_worktree_session: self.active_worktree_session.clone(),
            timeout_ms: self.timeout_ms,
            sub_agent: self.sub_agent.clone(),
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(TaskStack::default())),
            read_file_state: self.read_file_state.clone_isolated(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Return a copy that uses a specific read-file state cache.
    #[must_use]
    pub fn with_read_file_state(mut self, read_file_state: FileStateCache) -> Self {
        self.read_file_state = read_file_state;
        self
    }
}

/// Request to execute a command hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandHookExecutionRequest {
    /// The hook event that triggered this execution.
    pub event: HookEvent,
    /// The shell command to execute.
    pub command: String,
    /// Working directory for the command.
    pub cwd: PathBuf,
    /// JSON input passed via stdin.
    pub input: Value,
    /// Shell interpreter to use.
    pub shell: Option<HookShell>,
    /// Timeout in seconds.
    pub timeout_secs: Option<u64>,
}

/// Result of a command hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandHookExecutionResult {
    /// The hook event that was executed.
    pub event: HookEvent,
    pub command: String,
    pub shell: HookShell,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

// ---------------------------------------------------------------------------
// Tool registry with lazy loading
// ---------------------------------------------------------------------------

/// Names of tools that are always included in API requests (eager loading).
const EAGER_TOOL_NAMES: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "replace_in_file",
    "bash_command",
    "glob",
    "grep",
    "list_directory",
    "search_text",
    "ask_user",
    "todo_write",
];

/// Tool registry with eager/lazy loading support and BM25 search.
///
/// * **Eager tools** are always sent to the provider in every API request.
/// * **Lazy tools** are only included when explicitly needed (e.g. after a
///   `tool_search` call discovers them).
/// * The built-in BM25 search index allows semantic tool discovery.
pub struct ToolRegistry {
    /// Eagerly loaded tools (always included in API requests).
    eager_tools: Vec<ToolSpec>,
    /// Lazily loaded tools (included only when needed).
    lazy_tools: Vec<ToolSpec>,
    /// BM25 search engine for tool discovery.
    search_engine: search::ToolSearchEngine,
}

impl ToolRegistry {
    /// Create a new registry populated with all built-in tools.
    #[must_use]
    pub fn new() -> Self {
        let all = runtime_builtin_tool_specs();
        let mut eager = Vec::new();
        let mut lazy = Vec::new();
        let mut engine = search::ToolSearchEngine::new();

        for spec in &all {
            let search_terms = spec.tool_search_terms();
            let tags = search_terms.iter().map(String::as_str).collect::<Vec<_>>();
            engine.add_tool(spec.provider_wire_name(), &spec.description, &tags);

            if EAGER_TOOL_NAMES.contains(&spec.name.as_str()) {
                eager.push(spec.clone());
            } else {
                lazy.push(spec.clone());
            }
        }

        Self {
            eager_tools: eager,
            lazy_tools: lazy,
            search_engine: engine,
        }
    }

    /// Returns the eagerly loaded tool specs (for API requests).
    #[must_use]
    pub fn eager_specs(&self) -> &[ToolSpec] {
        &self.eager_tools
    }

    /// Returns all tool specs (eager + lazy).
    pub fn all_specs(&self) -> Vec<ToolSpec> {
        let mut all = self.eager_tools.clone();
        all.extend(self.lazy_tools.clone());
        all
    }

    /// Search tools using BM25 ranking.
    pub fn search(&self, query: &str, max: usize) -> Vec<search::SearchResult> {
        self.search_engine.search(query, max)
    }

    /// Convert eager tools to OpenAI-compatible `tools` parameter.
    #[must_use]
    pub fn to_api_tools(&self) -> Vec<Value> {
        self.eager_tools
            .iter()
            .map(|spec| spec.to_openai_schema())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct EffectivePermission {
    class: PermissionClass,
    requires_permission: bool,
    requires_explicit_approval: bool,
    blocked_path: Option<String>,
}

#[derive(Debug, Clone)]
struct FilesystemPermissionPrecheck {
    blocked_path: String,
    message: String,
    requires_confirmation: bool,
    suggestions: Vec<Value>,
    cause: claude_permissions::FilesystemCheckCause,
}

fn effective_permission_for_call(call: &ToolCall, spec: &ToolSpec) -> EffectivePermission {
    let default = EffectivePermission {
        class: classify_tool(&spec.permission_tool_name),
        requires_permission: spec.requires_permission,
        requires_explicit_approval: false,
        blocked_path: path_input_for_filesystem_tool(&spec.name, &call.input)
            .map(ToOwned::to_owned),
    };

    match spec.name.as_str() {
        "workflow" => match call.input.get("action").and_then(Value::as_str) {
            Some("run") => EffectivePermission {
                class: PermissionClass::Bash,
                requires_permission: true,
                requires_explicit_approval: false,
                blocked_path: None,
            },
            Some("create") | Some("delete") => EffectivePermission {
                class: PermissionClass::Edit,
                requires_permission: true,
                requires_explicit_approval: false,
                blocked_path: Some(".remote-code-rust/workflows.json".to_owned()),
            },
            Some("list") | Some("status") => EffectivePermission {
                class: PermissionClass::Read,
                requires_permission: false,
                requires_explicit_approval: false,
                blocked_path: Some(".remote-code-rust/workflows.json".to_owned()),
            },
            _ => default,
        },
        "enter_plan_mode" | "exit_plan_mode" => EffectivePermission {
            class: default.class,
            requires_permission: true,
            requires_explicit_approval: true,
            blocked_path: None,
        },
        _ => default,
    }
}

fn filesystem_operation_for_tool(
    tool_name: &str,
) -> Option<claude_permissions::FilesystemOperation> {
    match tool_name {
        "list_directory" | "read_file" | "search_text" | "glob" | "grep" => {
            Some(claude_permissions::FilesystemOperation::Read)
        }
        "write_file" => Some(claude_permissions::FilesystemOperation::Create),
        "replace_in_file" | "edit_file" | "notebook_edit" => {
            Some(claude_permissions::FilesystemOperation::Write)
        }
        _ => None,
    }
}

fn path_input_for_filesystem_tool<'a>(tool_name: &str, input: &'a Value) -> Option<&'a str> {
    match tool_name {
        "list_directory" | "search_text" | "glob" | "grep" => input
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .or(Some(".")),
        "read_file" | "write_file" | "replace_in_file" | "edit_file" => {
            file_ops::file_path_input(input)
        }
        "notebook_edit" => input
            .get("notebook_path")
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty()),
        _ => None,
    }
}

pub(crate) fn filesystem_access_options() -> claude_permissions::FilesystemAccessOptions {
    let runtime_context = current_runtime_agent_prompt_context();
    let policy = current_tool_runtime_policy();
    let mut internal_read_dirs = Vec::new();
    let mut internal_write_dirs = Vec::new();
    let mut internal_read_files = Vec::new();
    let mut internal_write_files = Vec::new();

    if let Some(task_output_dir) = policy.task_output_dir {
        internal_read_dirs.push(task_output_dir);
    }
    if let Some(tasks_dir) = policy.tasks_dir {
        internal_read_dirs.push(tasks_dir);
    }
    if let Some(tool_results_dir) = policy.tool_results_dir {
        internal_read_dirs.push(tool_results_dir);
    }
    if let Some(shell_output_dir) = policy.shell_policy.output_dir {
        internal_read_dirs.push(shell_output_dir);
    }

    if let Some(context) = runtime_context.as_ref() {
        if let Some(scratchpad_dir) = &context.scratchpad_dir {
            internal_read_dirs.push(scratchpad_dir.clone());
            internal_write_dirs.push(scratchpad_dir.clone());
        }
        if let Some(session_memory_dir) = &context.session_memory_dir {
            internal_read_dirs.push(session_memory_dir.clone());
        }
        if let Some(tasks_dir) = &context.tasks_dir {
            internal_read_dirs.push(tasks_dir.clone());
        }
        if let Some(tool_results_dir) = &context.tool_results_dir {
            internal_read_dirs.push(tool_results_dir.clone());
        }
        if let Some(auto_memory_dir) = &context.auto_memory_dir {
            internal_write_dirs.push(auto_memory_dir.clone());
        }
        if let Some(auto_memory_read_dir) = &context.auto_memory_read_dir {
            internal_read_dirs.push(auto_memory_read_dir.clone());
        } else if let Some(auto_memory_dir) = &context.auto_memory_dir {
            internal_read_dirs.push(auto_memory_dir.clone());
        }
        if let Some(team_memory_read_dir) = &context.team_memory_read_dir {
            internal_read_dirs.push(team_memory_read_dir.clone());
            internal_write_dirs.push(team_memory_read_dir.clone());
        }
        if let Some(project_temp_dir) = &context.project_temp_dir {
            internal_read_dirs.push(project_temp_dir.clone());
        }
        if let Some(teams_dir) = &context.teams_dir {
            internal_read_dirs.push(teams_dir.clone());
        }
        internal_read_dirs.extend(context.agent_memory_dirs.clone());
        internal_write_dirs.extend(context.agent_memory_dirs.clone());
        if let Some(preview_launch_config_path) = &context.preview_launch_config_path {
            internal_read_files.push(preview_launch_config_path.clone());
            internal_write_files.push(preview_launch_config_path.clone());
        }
    }

    claude_permissions::FilesystemAccessOptions {
        additional_dirs: runtime_context
            .map(|context| context.additional_working_directories)
            .unwrap_or_default(),
        current_mode: None,
        plan_file: plan_mode::current_plan_file_path(),
        internal_read_dirs,
        internal_write_dirs,
        internal_read_files,
        internal_write_files,
    }
}

fn precheck_filesystem_permission(
    spec: &ToolSpec,
    call: &ToolCall,
    context: &ToolExecutionContext,
    broker: &dyn PermissionBroker,
) -> Option<FilesystemPermissionPrecheck> {
    let operation = filesystem_operation_for_tool(&spec.name)?;
    let raw_path = path_input_for_filesystem_tool(&spec.name, &call.input)?;
    let mut options = filesystem_access_options();
    let mut additional_dirs = broker.additional_working_directories();
    if !additional_dirs.is_empty() {
        options.additional_dirs.append(&mut additional_dirs);
    }
    options.current_mode = broker.mode();
    let check =
        claude_permissions::assess_filesystem_access(raw_path, &context.cwd, &options, operation);
    if check.allowed {
        return None;
    }

    let message = check
        .reason
        .clone()
        .unwrap_or_else(|| "Path is not allowed.".to_owned());
    Some(FilesystemPermissionPrecheck {
        blocked_path: check.normalized_path.to_string_lossy().into_owned(),
        message,
        requires_confirmation: check.requires_confirmation,
        suggestions: check
            .suggestions
            .iter()
            .filter_map(|suggestion| serde_json::to_value(suggestion).ok())
            .collect(),
        cause: check.cause,
    })
}

fn filesystem_permission_request(
    spec: &ToolSpec,
    call: &ToolCall,
    context: &ToolExecutionContext,
    permission: &EffectivePermission,
    filesystem_precheck: Option<&FilesystemPermissionPrecheck>,
) -> PermissionRequest {
    PermissionRequest {
        tool_name: spec.name.clone(),
        permission_class: Some(permission.class),
        tool_input: call.input.clone(),
        working_directory: Some(context.cwd.to_string_lossy().into_owned()),
        tool_use_id: Some(call.id.clone()),
        title: Some(format!("Allow {}", spec.protocol_name)),
        description: Some(spec.description.clone()),
        blocked_path: filesystem_precheck
            .map(|precheck| precheck.blocked_path.clone())
            .or(permission.blocked_path.clone()),
        permission_suggestions: filesystem_precheck
            .map(|precheck| precheck.suggestions.clone())
            .unwrap_or_default(),
    }
}

fn apply_permission_decision_effects(
    broker: &dyn PermissionBroker,
    decision: &PermissionDecision,
    effective_call: &mut ToolCall,
    follow_up_user_blocks: &mut Vec<Value>,
) -> Result<()> {
    if let Some(updated_input) = decision.updated_input.clone() {
        effective_call.input = updated_input;
    }
    if !decision.permission_updates.is_empty() {
        let _ = broker.apply_permission_updates(&decision.permission_updates)?;
    }
    follow_up_user_blocks.extend(permission_decision_follow_up_blocks(decision, true));
    Ok(())
}

fn permission_decision_follow_up_blocks(
    decision: &PermissionDecision,
    include_feedback: bool,
) -> Vec<Value> {
    let mut blocks = Vec::new();
    if include_feedback
        && let Some(feedback) = decision.feedback.as_deref()
        && !feedback.is_empty()
    {
        blocks.push(serde_json::json!({
            "type": "text",
            "text": feedback,
        }));
    }
    blocks.extend(decision.content_blocks.clone());
    blocks
}

fn permission_denied_tool_result(tool_name: &str, decision: &PermissionDecision) -> ToolResult {
    ToolResult {
        content: decision
            .message
            .clone()
            .unwrap_or_else(|| format!("Permission denied for {}.", tool_name)),
        is_error: true,
        content_blocks: Vec::new(),
        follow_up_user_blocks: permission_decision_follow_up_blocks(decision, false),
    }
}

fn append_follow_up_user_blocks(
    mut result: ToolResult,
    follow_up_user_blocks: &[Value],
) -> ToolResult {
    if !follow_up_user_blocks.is_empty() {
        result
            .follow_up_user_blocks
            .extend(follow_up_user_blocks.iter().cloned());
    }
    result
}

fn session_claude_folder_allow_rule(rule: &claude_permissions::SourceAwarePermissionRule) -> bool {
    if rule.action != claude_permissions::RuleAction::Allow
        || rule.source != claude_permissions::RuleSource::Session
    {
        return false;
    }

    let Some(open) = rule.tool_pattern.find('(') else {
        return false;
    };
    let Some(close) = rule.tool_pattern.rfind(')') else {
        return false;
    };
    if open >= close {
        return false;
    }

    let content = rule.tool_pattern[open + 1..close].trim();
    (content.starts_with("/.claude/") || content.starts_with("~/.claude/"))
        && content.ends_with("/**")
        && !content.contains("..")
}

fn allow_rule_can_bypass_filesystem_precheck(
    matching_rule: Option<&claude_permissions::SourceAwarePermissionRule>,
    filesystem_precheck: Option<&FilesystemPermissionPrecheck>,
) -> bool {
    let Some(rule) = matching_rule else {
        return false;
    };
    let Some(precheck) = filesystem_precheck else {
        return true;
    };

    match precheck.cause {
        claude_permissions::FilesystemCheckCause::Allowed
        | claude_permissions::FilesystemCheckCause::OutsideWorkingDirectories => true,
        claude_permissions::FilesystemCheckCause::DangerousEdit => {
            session_claude_folder_allow_rule(rule)
        }
        claude_permissions::FilesystemCheckCause::ManualApprovalRequired
        | claude_permissions::FilesystemCheckCause::InvalidPath => false,
    }
}

/// Execute a tool call after checking permissions.
///
/// Looks up the tool by name, checks permissions via the broker, and dispatches
/// to the appropriate handler.
///
/// # Errors
/// Returns an error if the tool is unknown, permission is denied, or execution fails.
pub async fn execute_tool_call(
    call: &ToolCall,
    context: &ToolExecutionContext,
    broker: &dyn PermissionBroker,
) -> Result<ToolResult> {
    let mut effective_call = call.clone();
    let spec = runtime_provider_tool_spec(&call.name)
        .await
        .unwrap_or_else(|| ToolSpec {
            name: call.name.clone(),
            protocol_name: call.name.clone(),
            permission_tool_name: call.name.clone(),
            description: String::new(),
            requires_permission: true,
            input_schema: Value::Null,
        });

    if spec.description.is_empty() {
        return Err(anyhow!("unknown tool {}", call.name));
    }
    effective_call.name = spec.name.clone();

    if !tool_allowed_by_runtime(&spec.name) {
        return Ok(ToolResult {
            content: format!(
                "Tool {} is disallowed by the current runtime policy.",
                spec.name
            ),
            is_error: true,
            content_blocks: Vec::new(),
            follow_up_user_blocks: Vec::new(),
        });
    }

    if let Some(rejected) = plan_mode::plan_mode_guard(&spec, call, context, broker.mode()) {
        return Ok(rejected);
    }

    let permission = effective_permission_for_call(&effective_call, &spec);
    let filesystem_precheck =
        precheck_filesystem_permission(&spec, &effective_call, context, broker);
    let filesystem_rule_relevant = filesystem_operation_for_tool(&spec.name).is_some()
        && path_input_for_filesystem_tool(&spec.name, &effective_call.input).is_some();

    if let Some(precheck) = filesystem_precheck
        .as_ref()
        .filter(|precheck| !precheck.requires_confirmation)
    {
        return Ok(ToolResult {
            content: precheck.message.clone(),
            is_error: true,
            content_blocks: Vec::new(),
            follow_up_user_blocks: Vec::new(),
        });
    }

    let mut follow_up_user_blocks = Vec::new();

    if permission.requires_permission || filesystem_precheck.is_some() || filesystem_rule_relevant {
        // Fast-path: if the broker exposes a mode and auto_allows covers this class, skip.
        let broker_mode = broker.mode();
        let permission_request = filesystem_permission_request(
            &spec,
            &effective_call,
            context,
            &permission,
            filesystem_precheck.as_ref(),
        );
        let explicit_approval_handled = if permission.requires_explicit_approval {
            let decision = broker
                .decide_forced_prompt(permission_request.clone())
                .await;
            if !decision.allowed {
                return Ok(permission_denied_tool_result(&spec.name, &decision));
            }
            apply_permission_decision_effects(
                broker,
                &decision,
                &mut effective_call,
                &mut follow_up_user_blocks,
            )?;
            true
        } else {
            false
        };
        let filesystem_rule = if filesystem_operation_for_tool(&spec.name).is_some() {
            broker.matching_rule(&permission_request)
        } else {
            None
        };
        let filesystem_rule_action = filesystem_rule.as_ref().map(|rule| rule.action);
        if matches!(
            filesystem_rule_action,
            Some(claude_permissions::RuleAction::Deny | claude_permissions::RuleAction::Ask)
        ) {
            let decision = broker.decide_forced_prompt(permission_request).await;
            if !decision.allowed {
                return Ok(permission_denied_tool_result(&spec.name, &decision));
            }
            apply_permission_decision_effects(
                broker,
                &decision,
                &mut effective_call,
                &mut follow_up_user_blocks,
            )?;
        } else if matches!(
            filesystem_rule_action,
            Some(claude_permissions::RuleAction::Allow)
        ) && allow_rule_can_bypass_filesystem_precheck(
            filesystem_rule.as_ref(),
            filesystem_precheck.as_ref(),
        ) {
            // Explicit filesystem allow rules are enough to proceed; keep the
            // dispatch guard marked confirmed because the path precheck asked.
        } else if matches!(
            filesystem_rule_action,
            Some(claude_permissions::RuleAction::Allow)
        ) && filesystem_precheck.is_some()
        {
            let decision = broker.decide_forced_prompt(permission_request).await;
            if !decision.allowed {
                return Ok(permission_denied_tool_result(&spec.name, &decision));
            }
            apply_permission_decision_effects(
                broker,
                &decision,
                &mut effective_call,
                &mut follow_up_user_blocks,
            )?;
        } else if !explicit_approval_handled
            && (permission.requires_permission || filesystem_precheck.is_some())
        {
            let skip_broker = filesystem_precheck.is_none()
                && broker_mode.is_some_and(|m| auto_allows(m, permission.class));
            if !skip_broker {
                let decision = broker.decide(permission_request).await;
                if !decision.allowed {
                    return Ok(permission_denied_tool_result(&spec.name, &decision));
                }
                apply_permission_decision_effects(
                    broker,
                    &decision,
                    &mut effective_call,
                    &mut follow_up_user_blocks,
                )?;
            }
        }
    }

    file_ops::with_filesystem_permission_confirmed(filesystem_precheck.is_some(), async {
        if spec.name == "tool_search" {
            return match system::tool_search_tool(&effective_call.input).await {
                Ok(result) => Ok(append_follow_up_user_blocks(result, &follow_up_user_blocks)),
                Err(error) => Ok(ToolResult {
                    content: error.to_string(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: follow_up_user_blocks.clone(),
                }),
            };
        }

        if spec.name == "mcp_call" {
            return match mcp_tools::mcp_call_tool(&effective_call.input, context).await {
                Ok(result) => Ok(append_follow_up_user_blocks(result, &follow_up_user_blocks)),
                Err(error) => Ok(ToolResult {
                    content: error.to_string(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: follow_up_user_blocks.clone(),
                }),
            };
        }

        if spec.name == "read_mcp_resource" {
            return match mcp_tools::read_mcp_resource_tool(&call.id, &effective_call.input, context)
                .await
            {
                Ok(result) => Ok(append_follow_up_user_blocks(result, &follow_up_user_blocks)),
                Err(error) => Ok(ToolResult {
                    content: error.to_string(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: follow_up_user_blocks.clone(),
                }),
            };
        }

        if effective_call.name.starts_with("mcp__") {
            return match mcp_catalog::execute_runtime_mcp_tool(
                &effective_call.name,
                &effective_call.input,
                context,
            )
            .await
            {
                Ok(result) => Ok(append_follow_up_user_blocks(result, &follow_up_user_blocks)),
                Err(error) => Ok(ToolResult {
                    content: error.to_string(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: follow_up_user_blocks.clone(),
                }),
            };
        }

        // read_file returns Result<ToolResult> directly (for multimodal image support)
        // so handle it separately before the String-returning match.
        if spec.name == "read_file" {
            let result = file_ops::read_file(&effective_call.input, context);
            return match result {
                Ok(tool_result) => Ok(append_follow_up_user_blocks(
                    tool_result,
                    &follow_up_user_blocks,
                )),
                Err(error) => Ok(ToolResult {
                    content: error.to_string(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: follow_up_user_blocks.clone(),
                }),
            };
        }

        let result = match spec.name.as_str() {
            "list_directory" => file_ops::list_directory(&effective_call.input, context),
            "search_text" => file_ops::search_text(&effective_call.input, context),
            "write_file" => file_ops::write_file(&effective_call.input, context),
            "replace_in_file" => file_ops::replace_in_file(&effective_call.input, context),
            "edit_file" => file_ops::edit_file(&effective_call.input, context),
            "bash_command" => command::bash_command(&effective_call.input, context).await,
            "glob" => file_ops::glob_files(&effective_call.input, context),
            "grep" => file_ops::grep_files(&effective_call.input, context),
            "web_fetch" => web::web_fetch(&effective_call.input, context).await,
            "ask_user" => misc::ask_user(&effective_call.input, context),
            "todo_write" => system::todo_write(&effective_call.input, context),
            "config_read" => system::config_read(&effective_call.input, context),
            "agent" => agent::agent_tool(&effective_call.input, context).await,
            "web_search" => web::web_search(&effective_call.input, context).await,
            // ── Phase 2 tools ──────────────────────────────────────────────
            "lsp" => misc::lsp_tool(&effective_call.input, context).await,
            "task_create" => tasks::task_create(&effective_call.input),
            "task_get" => tasks::task_get(&effective_call.input),
            "task_list" => tasks::task_list(&effective_call.input),
            "task_update" => tasks::task_update(&effective_call.input),
            "task_output" => tasks::task_output(&effective_call.input),
            "task_stop" => tasks::task_stop(&effective_call.input),
            "notebook_edit" => misc::notebook_edit(&effective_call.input, context),
            "discover_skills" => misc::skill_discover(&effective_call.input, context),
            "send_message" => send_message::send_message(&effective_call.input, context).await,
            "enter_plan_mode" => plan_mode::enter_plan_mode(&effective_call.input, context),
            "exit_plan_mode" => plan_mode::exit_plan_mode(&effective_call.input, context),
            "sleep" => system::sleep_tool(&effective_call.input).await,
            "snip" => system::snip_tool(&effective_call.input, context),
            // ── Phase 3 tools ──────────────────────────────────────────────
            "team_create" => misc::team_create_tool(&effective_call.input, context).await,
            "team_status" => misc::team_status_tool(&effective_call.input).await,
            "web_browser" => web::web_browser_tool(&effective_call.input, context).await,
            "verify_plan" => system::verify_plan_tool(&effective_call.input),
            "terminal_capture" => {
                system::terminal_capture_tool(&effective_call.input, context).await
            }
            // ── Phase 4: Upstream gap-fill tools ────────────────────────────
            "powershell" => command::powershell_tool(&effective_call.input, context).await,
            "repl" => command::repl_tool(&effective_call.input, context).await,
            "monitor" => system::monitor_tool(&effective_call.input),
            "schedule_cron" => workflow::schedule_cron_tool(&effective_call.input, context),
            "cron_delete" => workflow::cron_delete_tool(&effective_call.input, context),
            "cron_list" => workflow::cron_list_tool(&effective_call.input, context),
            "remote_trigger" => misc::remote_trigger_tool(&effective_call.input).await,
            "workflow" => workflow::workflow_tool(&effective_call.input, context),
            "suggest_pr" => git::suggest_pr_tool(context),
            "enter_worktree" => git::enter_worktree_tool(&effective_call.input, context),
            "exit_worktree" => git::exit_worktree_tool(&effective_call.input, context),
            "list_worktrees" => git::list_worktrees_tool(context),
            "brief" => system::brief_tool(&effective_call.input),
            "ctx_inspect" => system::ctx_inspect_tool(&effective_call.input, context).await,
            "list_peers" => system::list_peers_tool(&effective_call.input).await,
            "tungsten" => misc::tungsten_tool(&effective_call.input, context).await,
            "overflow_test" => misc::overflow_test_tool(&effective_call.input),
            "synthetic_output" => misc::synthetic_output_tool(&effective_call.input),
            "mcp_auth" => mcp_tools::mcp_auth_tool(&effective_call.input, context).await,
            "list_mcp_resources" => {
                mcp_tools::list_mcp_resources_tool(&effective_call.input, context).await
            }
            "skill_execute" => misc::skill_execute_tool(&effective_call.input, context),
            "voice_input" => misc::voice_input_tool(&effective_call.input),
            "daemon" => workflow::daemon_tool(&effective_call.input, context),
            // ── Phase 9: New dedicated tool modules ────────────────────────────
            "team_delete" => team_tools::team_delete(&effective_call.input, context),
            "team_list" => team_tools::team_list(&effective_call.input, context),
            "broadcast_message" => {
                send_message::broadcast_message(&effective_call.input, context).await
            }
            "review_artifact" => review_artifact::review_artifact(&effective_call.input, context),
            "send_user_file" => send_user_file::send_user_file(&effective_call.input, context),
            _ => Err(anyhow!("unsupported tool {}", spec.name)),
        };

        match result {
            Ok(content) => Ok(ToolResult {
                content,
                is_error: false,
                content_blocks: Vec::new(),
                follow_up_user_blocks: follow_up_user_blocks.clone(),
            }),
            Err(error) => Ok(ToolResult {
                content: error.to_string(),
                is_error: true,
                content_blocks: Vec::new(),
                follow_up_user_blocks: follow_up_user_blocks.clone(),
            }),
        }
    })
    .await
}

fn normalize_tool_runtime_policy(mut policy: ToolRuntimePolicy) -> ToolRuntimePolicy {
    policy.allowed_tools = policy
        .allowed_tools
        .into_iter()
        .filter_map(|tool| normalize_runtime_tool_filter(&tool, ToolFilterBehavior::Allow))
        .collect::<Vec<_>>();
    policy.allowed_tools.sort();
    policy.allowed_tools.dedup();

    policy.disallowed_tools = policy
        .disallowed_tools
        .into_iter()
        .filter_map(|tool| normalize_runtime_tool_filter(&tool, ToolFilterBehavior::Deny))
        .collect::<Vec<_>>();
    policy.disallowed_tools.sort();
    policy.disallowed_tools.dedup();

    policy.mcp_servers.sort_by(|left, right| {
        left.server
            .name
            .cmp(&right.server.name)
            .then_with(|| left.origin_kind.cmp(&right.origin_kind))
            .then_with(|| left.origin_name.cmp(&right.origin_name))
            .then_with(|| left.config_path.cmp(&right.config_path))
    });
    policy.mcp_servers.dedup_by(|left, right| {
        left.config_path == right.config_path && left.server == right.server
    });
    policy
}

#[derive(Debug, Clone, Copy)]
enum ToolFilterBehavior {
    Allow,
    Deny,
}

fn normalize_runtime_tool_filter(raw: &str, behavior: ToolFilterBehavior) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let has_matcher = trimmed.contains('(');
    let tool_name = trimmed
        .split_once('(')
        .map_or(trimmed, |(name, _)| name.trim());

    if has_matcher && matches!(behavior, ToolFilterBehavior::Deny) {
        return Some(trimmed.to_ascii_lowercase());
    }

    canonical_runtime_tool_name(tool_name).or_else(|| Some(tool_name.to_ascii_lowercase()))
}

fn canonical_runtime_tool_name(name: &str) -> Option<String> {
    let key = normalize_tool_alias_key(name);
    if key.is_empty() {
        return None;
    }
    for spec in builtin_tool_specs() {
        if normalize_tool_alias_key(&spec.name) == key
            || normalize_tool_alias_key(&spec.protocol_name) == key
            || canonical_builtin_tool_name(&spec.name)
                .is_some_and(|canonical| normalize_tool_alias_key(canonical) == key)
            || builtin_tool_aliases(&spec.name)
                .iter()
                .any(|alias| normalize_tool_alias_key(alias) == key)
        {
            return Some(spec.name);
        }
    }
    None
}

fn tool_allowed_by_runtime(tool_name: &str) -> bool {
    let policy = current_tool_runtime_policy();
    tool_allowed_by_policy(tool_name, &policy)
}

pub(crate) fn tool_allowed_by_policy(tool_name: &str, policy: &ToolRuntimePolicy) -> bool {
    let normalized_name = tool_name.trim().to_ascii_lowercase();
    if !policy.allowed_tools.is_empty() && !policy.allowed_tools.contains(&normalized_name) {
        return false;
    }
    !policy.disallowed_tools.contains(&normalized_name)
}

#[cfg(test)]
mod tests {
    use super::{
        CommandHookExecutionRequest, HookShell, RuntimeMcpServerPolicyEntry, ToolExecutionContext,
        ToolResultSizePolicy, ToolRuntimePolicy, ToolRuntimePolicyOverlay, ToolSpec,
        builtin_tool_specs, configure_tool_runtime_policy, execute_command_hook, execute_tool_call,
        extract_discovered_tool_names, extract_discovered_tool_names_from_conversation,
        runtime_provider_tool_spec, runtime_provider_tool_specs,
        runtime_tool_result_persistence_skip_names, runtime_tool_search_candidate_specs,
        runtime_visible_provider_tool_specs,
        runtime_visible_provider_tool_specs_with_discovered_tools,
        with_runtime_agent_prompt_context_provider, with_runtime_fork_snapshot_provider,
        with_tool_runtime_policy_overlay,
    };
    use crate::specs;
    use claude_core::{
        ConversationEntry, HookEvent, PermissionMode, ProviderResponse, SubAgentCompletion,
        SubAgentExecutionRequest, SubAgentForkSnapshot, ToolCall, UsageSummary,
    };
    use claude_mcp::{
        McpCapabilityMatrix, McpPromptArgument, McpPromptDescriptor, McpServerConfig,
        McpServerInspection, McpToolDescriptor, McpToolPolicy, McpTransportConfig,
    };
    use claude_permissions::{
        LayeredPermissionBroker, PermissionBroker, PermissionDecision, PermissionRequest,
        PermissionUpdate, PermissionUpdateDestination, StaticPermissionBroker,
    };
    use claude_swarm::{TeamFile, TeamMember, mailbox, team_helpers};
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as ProcessCommand;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex as AsyncMutex;

    static RUNTIME_POLICY_TEST_MUTEX: Lazy<AsyncMutex<()>> = Lazy::new(|| AsyncMutex::new(()));
    static PLAN_MODE_RUNTIME_TEST_MUTEX: Lazy<AsyncMutex<()>> = Lazy::new(|| AsyncMutex::new(()));

    fn python_command() -> Option<(String, Vec<String>)> {
        let probe = |cmd: &str, args: &[&str]| -> bool {
            let mut cmd = ProcessCommand::new(cmd);
            cmd.args(args).args(["-c", "import json"]);
            cmd.output().is_ok_and(|output| output.status.success())
        };

        if let Ok(path) = std::env::var("PYTHON")
            && probe(&path, &[])
        {
            return Some((path, Vec::new()));
        }

        for candidate in ["python", "python3"] {
            if probe(candidate, &[]) {
                return Some((candidate.to_owned(), Vec::new()));
            }
        }

        if cfg!(windows) && probe("py", &["-3"]) {
            return Some(("py".to_owned(), vec!["-3".to_owned()]));
        }

        None
    }

    fn mock_mcp_resource_server_script() -> &'static str {
        r#"
import json
import sys

while True:
    raw = sys.stdin.readline()
    if not raw:
        break
    raw = raw.strip()
    if not raw:
        continue
    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"resources": {}},
                "serverInfo": {"name": "mock-resource-mcp", "version": "0.1.0"}
            }
        }), flush=True)
    elif method == "notifications/initialized":
        continue
    elif method == "resources/read":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "contents": [{
                    "uri": message["params"]["uri"],
                    "mimeType": "text/plain",
                    "text": "hello from resource"
                }]
            }
        }), flush=True)
    else:
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {"code": -32601, "message": "unknown method"}
        }), flush=True)
"#
    }

    #[test]
    fn builtin_tool_result_size_policies_match_research_overrides() {
        let specs = builtin_tool_specs();
        let read_file = specs
            .iter()
            .find(|spec| spec.name == "read_file")
            .expect("read_file spec");
        let bash = specs
            .iter()
            .find(|spec| spec.name == "bash_command")
            .expect("bash_command spec");
        let powershell = specs
            .iter()
            .find(|spec| spec.name == "powershell")
            .expect("powershell spec");
        let grep = specs
            .iter()
            .find(|spec| spec.name == "grep")
            .expect("grep spec");
        let read_mcp_resource = specs::mcp_resource_tool_specs()
            .into_iter()
            .find(|spec| spec.name == "read_mcp_resource")
            .expect("read_mcp_resource spec");
        let skip_tool_names = runtime_tool_result_persistence_skip_names();

        assert_eq!(
            read_file.tool_result_size_policy(),
            ToolResultSizePolicy::NeverPersist
        );
        assert_eq!(
            bash.tool_result_size_policy(),
            ToolResultSizePolicy::Finite(30_000)
        );
        assert_eq!(
            powershell.tool_result_size_policy(),
            ToolResultSizePolicy::Finite(30_000)
        );
        assert_eq!(
            grep.tool_result_size_policy(),
            ToolResultSizePolicy::Finite(20_000)
        );
        assert_eq!(
            read_mcp_resource.tool_result_size_policy(),
            ToolResultSizePolicy::Finite(100_000)
        );
        assert!(skip_tool_names.contains("read_file"));
        assert!(!skip_tool_names.contains("bash_command"));
    }

    #[derive(Debug)]
    struct RecordingPermissionBroker {
        allow: bool,
        requests: Arc<Mutex<Vec<PermissionRequest>>>,
        allow_feedback: Option<String>,
        allow_content_blocks: Vec<Value>,
        deny_content_blocks: Vec<Value>,
    }

    #[async_trait::async_trait]
    impl PermissionBroker for RecordingPermissionBroker {
        async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
            self.requests.lock().push(request);
            if self.allow {
                let mut decision = PermissionDecision::allow();
                decision.feedback = self.allow_feedback.clone();
                decision.content_blocks = self.allow_content_blocks.clone();
                decision
            } else {
                let mut decision = PermissionDecision::deny("recorded denial");
                decision.content_blocks = self.deny_content_blocks.clone();
                decision
            }
        }
    }

    #[derive(Debug)]
    struct RuleAwareBroker {
        mode: claude_core::PermissionMode,
        action: Option<claude_permissions::RuleAction>,
        allow: bool,
        matching_rule: Option<claude_permissions::SourceAwarePermissionRule>,
        requests: Arc<Mutex<Vec<PermissionRequest>>>,
        forced_requests: Arc<Mutex<Vec<PermissionRequest>>>,
    }

    #[async_trait::async_trait]
    impl PermissionBroker for RuleAwareBroker {
        async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
            self.requests.lock().push(request);
            if self.allow {
                PermissionDecision::allow()
            } else {
                PermissionDecision::deny("recorded denial")
            }
        }

        async fn decide_forced_prompt(&self, request: PermissionRequest) -> PermissionDecision {
            self.forced_requests.lock().push(request);
            if self.allow {
                PermissionDecision::allow()
            } else {
                PermissionDecision::deny("forced recorded denial")
            }
        }

        fn mode(&self) -> Option<claude_core::PermissionMode> {
            Some(self.mode)
        }

        fn matching_rule(
            &self,
            _request: &PermissionRequest,
        ) -> Option<claude_permissions::SourceAwarePermissionRule> {
            self.matching_rule.clone().or_else(|| {
                self.action
                    .map(|action| claude_permissions::SourceAwarePermissionRule {
                        tool_pattern: "*".to_owned(),
                        action,
                        source: claude_permissions::RuleSource::Session,
                    })
            })
        }

        fn matching_rule_action(
            &self,
            _request: &PermissionRequest,
        ) -> Option<claude_permissions::RuleAction> {
            self.action
        }
    }

    #[derive(Debug)]
    struct RecordingPlanModeRuntime {
        enter_calls: Arc<Mutex<Vec<String>>>,
        exit_calls: Arc<Mutex<Vec<crate::plan_mode::ExitPlanModeInput>>>,
        plan_file_path: PathBuf,
    }

    impl crate::plan_mode::PlanModeRuntime for RecordingPlanModeRuntime {
        fn enter_plan_mode(&self, objective: &str) -> anyhow::Result<String> {
            self.enter_calls.lock().push(objective.to_owned());
            Ok(format!("entered plan mode for {objective}"))
        }

        fn exit_plan_mode(
            &self,
            input: crate::plan_mode::ExitPlanModeInput,
        ) -> anyhow::Result<String> {
            self.exit_calls.lock().push(input);
            Ok("exited plan mode".to_owned())
        }

        fn snapshot(&self) -> crate::plan_mode::PlanModeRuntimeSnapshot {
            crate::plan_mode::PlanModeRuntimeSnapshot {
                permission_mode: PermissionMode::Plan,
                plan_file_path: Some(self.plan_file_path.clone()),
            }
        }

        fn persist_plan_snapshot(&self) -> anyhow::Result<()> {
            // Recording stub: no-op for tests
            Ok(())
        }
    }

    struct PlanModeRuntimeGuard;

    impl Drop for PlanModeRuntimeGuard {
        fn drop(&mut self) {
            let _ = crate::plan_mode::configure_plan_mode_runtime(None);
        }
    }

    fn install_recording_plan_mode_runtime(
        enter_calls: Arc<Mutex<Vec<String>>>,
        exit_calls: Arc<Mutex<Vec<crate::plan_mode::ExitPlanModeInput>>>,
    ) -> PlanModeRuntimeGuard {
        crate::plan_mode::configure_plan_mode_runtime(Some(Arc::new(RecordingPlanModeRuntime {
            enter_calls,
            exit_calls,
            plan_file_path: PathBuf::from("plan.md"),
        })))
        .expect("configure plan mode runtime");
        PlanModeRuntimeGuard
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    struct TaskListDirGuard {
        _runtime_policy_guard: tokio::sync::MutexGuard<'static, ()>,
        _guard: crate::tasks::TaskTestGuard,
    }

    impl Drop for TaskListDirGuard {
        fn drop(&mut self) {
            crate::tasks::configure_task_list_context(None, None).expect("reset task list context");
            crate::tasks::set_leader_team_name(None).expect("reset leader team name");
        }
    }

    async fn configure_task_list_for_tests(
        base: &std::path::Path,
        task_list_id: Option<&str>,
    ) -> TaskListDirGuard {
        let runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let guard = crate::tasks::test_guard_for_tests();
        crate::tasks::configure_task_list_context(
            task_list_id.map(ToOwned::to_owned),
            Some(base.join("tasks")),
        )
        .expect("configure task list context");
        crate::tasks::set_leader_team_name(None).expect("clear leader team name");
        TaskListDirGuard {
            _runtime_policy_guard: runtime_policy_guard,
            _guard: guard,
        }
    }

    struct TeamDirGuard {
        _task_list_guard: TaskListDirGuard,
    }

    impl Drop for TeamDirGuard {
        fn drop(&mut self) {
            team_helpers::set_base_dir_override(None);
            crate::team_tools::set_base_dir_override(None);
        }
    }

    async fn seed_team(base: &std::path::Path, team_name: &str) -> TeamDirGuard {
        team_helpers::set_base_dir_override(Some(base.to_path_buf()));
        crate::team_tools::set_base_dir_override(Some(base.to_path_buf()));
        let task_list_guard = configure_task_list_for_tests(base, None).await;
        let mut team = TeamFile::new(team_name, "lead");
        team.description = Some("test objective".to_owned());
        team.members
            .push(TeamMember::new("worker-1", "agent-007", "pane-1", "."));
        team_helpers::create_team(&team)
            .await
            .expect("team should be created");
        TeamDirGuard {
            _task_list_guard: task_list_guard,
        }
    }

    async fn create_team_via_tool(
        context: &ToolExecutionContext,
        broker: &StaticPermissionBroker,
        team_name: &str,
    ) {
        let result = execute_tool_call(
            &ToolCall {
                id: format!("team-create-{team_name}"),
                name: "team_create".to_owned(),
                input: json!({
                    "team_name": team_name,
                    "description": format!("Coordinate work for {team_name}"),
                }),
            },
            context,
            broker,
        )
        .await
        .expect("team_create should work");
        assert!(!result.is_error, "team_create error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("team_create should return json");
        assert_eq!(parsed["team_name"], team_name);
        assert!(parsed["team_file_path"].as_str().is_some());
        assert!(parsed["lead_agent_id"].as_str().is_some());
    }

    async fn add_team_member_for_test(
        team_name: &str,
        agent_name: &str,
        agent_type: &str,
        cwd: &std::path::Path,
        color: Option<&str>,
    ) {
        let mut team = team_helpers::read_team(team_name)
            .await
            .expect("team should exist");
        let mut member = TeamMember::new(
            format!("agent-{agent_name}"),
            agent_name,
            format!("pane-{agent_name}"),
            cwd.to_string_lossy().to_string(),
        );
        member.agent_type = Some(agent_type.to_owned());
        member.color = color.map(ToOwned::to_owned);
        member.is_active = Some(true);
        team.members.push(member);
        team_helpers::update_team(&team)
            .await
            .expect("team should update");
    }

    fn fake_runtime_mcp_policy_entry(name: &str) -> RuntimeMcpServerPolicyEntry {
        RuntimeMcpServerPolicyEntry {
            origin_kind: "cwd".to_owned(),
            origin_name: "workspace".to_owned(),
            config_path: PathBuf::from(".mcp.json"),
            server: McpServerConfig {
                name: name.to_owned(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "definitely-missing-mcp-command".to_owned(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                },
                capabilities: McpCapabilityMatrix::default(),
                startup_timeout_secs: Some(1),
                request_timeout_secs: Some(1),
                metadata: BTreeMap::new(),
                oauth: None,
                tool_policy: McpToolPolicy::default(),
            },
        }
    }

    fn live_mcp_observation_provider(
        observation: crate::mcp_runtime::RuntimeMcpObservation,
    ) -> Arc<crate::RuntimeMcpObservationProvider> {
        Arc::new(move || observation.clone())
    }

    #[derive(Clone)]
    struct RecordingAgentRuntime {
        requests: Arc<Mutex<Vec<SubAgentExecutionRequest>>>,
        result: claude_core::SubAgentExecutionResult,
    }

    #[async_trait::async_trait]
    impl SubAgentCompletion for RecordingAgentRuntime {
        async fn complete(
            &self,
            _conversation: &[claude_core::ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            panic!("complete() should not be used when execute_agent is supported")
        }

        fn supports_agent_execution(&self) -> bool {
            true
        }

        async fn execute_agent(
            &self,
            request: SubAgentExecutionRequest,
        ) -> anyhow::Result<claude_core::SubAgentExecutionResult> {
            self.requests.lock().push(request);
            Ok(self.result.clone())
        }
    }

    #[tokio::test]
    async fn read_and_search_tools_work() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let file = tempdir.path().join("notes.txt");
        if let Err(error) = std::fs::write(&file, "hello\nremote code\n") {
            panic!("failed to seed file: {error}");
        }
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let read = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path":"notes.txt"}),
            },
            &context,
            &broker,
        )
        .await;
        assert!(read.is_ok());
        let read = read.unwrap_or_else(|error| panic!("read failed: {error}"));
        assert!(read.content.contains("remote code"));

        let search = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "search_text".to_owned(),
                input: json!({"pattern":"remote","path":"."}),
            },
            &context,
            &broker,
        )
        .await;
        assert!(search.is_ok());
        let search = search.unwrap_or_else(|error| panic!("search failed: {error}"));
        assert!(search.content.contains("notes.txt:2"));

        assert!(
            builtin_tool_specs()
                .iter()
                .any(|spec| spec.protocol_name == "Bash")
        );
    }

    #[tokio::test]
    async fn provider_tool_schemas_expose_claude_wire_names() {
        let specs = runtime_provider_tool_specs().await;
        let read = specs
            .iter()
            .find(|spec| spec.name == "read_file")
            .expect("read_file spec");
        let bash = specs
            .iter()
            .find(|spec| spec.name == "bash_command")
            .expect("bash_command spec");
        let tool_search = specs
            .iter()
            .find(|spec| spec.name == "tool_search")
            .expect("tool_search spec");

        assert_eq!(read.provider_wire_name(), "Read");
        assert_eq!(bash.provider_wire_name(), "Bash");
        assert_eq!(tool_search.provider_wire_name(), "ToolSearch");
        assert_eq!(read.to_anthropic_schema()["name"], "Read");
        assert_eq!(bash.to_openai_schema()["function"]["name"], "Bash");
        assert_eq!(
            runtime_provider_tool_spec("Read")
                .await
                .expect("Read alias")
                .name,
            "read_file"
        );
        assert_eq!(
            runtime_provider_tool_spec("ToolSearch")
                .await
                .expect("ToolSearch alias")
                .name,
            "tool_search"
        );
    }

    #[tokio::test]
    async fn execute_tool_call_accepts_provider_wire_name() {
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "bash-provider-name".to_owned(),
                name: "Bash".to_owned(),
                input: json!({
                    "command": "echo provider-wire-ok",
                    "timeout_ms": 10_000,
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("Bash alias should execute");

        assert!(!result.is_error, "Bash alias failed: {}", result.content);
        assert!(result.content.contains("provider-wire-ok"));
    }

    #[tokio::test]
    async fn read_file_outside_workspace_routes_through_permission_broker() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&outside, "secret").expect("outside file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RecordingPermissionBroker {
            allow: false,
            requests: requests.clone(),
            allow_feedback: None,
            allow_content_blocks: Vec::new(),
            deny_content_blocks: Vec::new(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-outside".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": outside.to_string_lossy().to_string()}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tool_name, "read_file");
        assert_eq!(
            requests[0].blocked_path.as_deref(),
            Some(outside.to_string_lossy().as_ref())
        );
        assert_eq!(
            requests[0].working_directory.as_deref(),
            Some(context.cwd.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn denied_permission_preserves_follow_up_user_blocks() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&outside, "secret").expect("outside file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let deny_blocks = vec![json!({
            "type": "text",
            "text": "Use an approved path instead.",
        })];
        let broker = RecordingPermissionBroker {
            allow: false,
            requests,
            allow_feedback: None,
            allow_content_blocks: Vec::new(),
            deny_content_blocks: deny_blocks.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-outside-denied".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path": outside.to_string_lossy().to_string()}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert_eq!(result.follow_up_user_blocks, deny_blocks);
    }

    #[tokio::test]
    async fn allowed_permission_appends_feedback_and_content_blocks() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&outside, "secret").expect("outside file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let extra_blocks = vec![json!({
            "type": "text",
            "text": "Extra approval detail.",
        })];
        let broker = RecordingPermissionBroker {
            allow: true,
            requests,
            allow_feedback: Some("Proceed with the approved path.".to_owned()),
            allow_content_blocks: extra_blocks.clone(),
            deny_content_blocks: Vec::new(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-outside-allowed".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path": outside.to_string_lossy().to_string()}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error);
        assert!(result.content.contains("secret"));
        assert_eq!(result.follow_up_user_blocks.len(), 2);
        assert_eq!(result.follow_up_user_blocks[0]["type"], "text");
        assert_eq!(
            result.follow_up_user_blocks[0]["text"],
            "Proceed with the approved path."
        );
        assert_eq!(result.follow_up_user_blocks[1], extra_blocks[0]);
    }

    #[tokio::test]
    async fn write_file_symlink_parent_escape_requires_explicit_permission() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        let link = workspace.join("linked");
        if create_dir_symlink(&outside, &link).is_err() {
            return;
        }
        let target = link.join("new.txt");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RecordingPermissionBroker {
            allow: false,
            requests: requests.clone(),
            allow_feedback: None,
            allow_content_blocks: Vec::new(),
            deny_content_blocks: Vec::new(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "write-symlink".to_owned(),
                name: "write_file".to_owned(),
                input: json!({
                    "file_path": target.to_string_lossy().to_string(),
                    "content": "escaped",
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert!(!outside.join("new.txt").exists());
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .blocked_path
                .as_deref()
                .is_some_and(|path| path.ends_with("new.txt"))
        );
    }

    #[tokio::test]
    async fn read_file_allows_runtime_additional_working_directory() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let extra = tempdir.path().join("extra");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&extra).expect("extra");
        let target = extra.join("allowed.txt");
        std::fs::write(&target, "allowed").expect("extra file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(false);
        let runtime_context = crate::RuntimeAgentPromptContext {
            additional_working_directories: vec![extra],
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                execute_tool_call(
                    &ToolCall {
                        id: "read-extra".to_owned(),
                        name: "read_file".to_owned(),
                        input: json!({"file_path": target.to_string_lossy().to_string()}),
                    },
                    &context,
                    &broker,
                )
                .await
            },
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("allowed"));
    }

    #[tokio::test]
    async fn read_file_allows_runtime_teams_directory() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let teams = tempdir.path().join("teams");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&teams).expect("teams");
        let target = teams.join("alpha").join("config.json");
        std::fs::create_dir_all(target.parent().expect("team dir")).expect("team dir");
        std::fs::write(&target, "{\"name\":\"alpha\"}").expect("team file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(false);
        let runtime_context = crate::RuntimeAgentPromptContext {
            teams_dir: Some(teams),
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                execute_tool_call(
                    &ToolCall {
                        id: "read-team".to_owned(),
                        name: "read_file".to_owned(),
                        input: json!({"file_path": target.to_string_lossy().to_string()}),
                    },
                    &context,
                    &broker,
                )
                .await
            },
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("\"name\":\"alpha\""));
    }

    #[tokio::test]
    async fn read_file_allows_runtime_tool_results_directory() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let tool_results = tempdir.path().join("tool-results");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&tool_results).expect("tool results");
        let target = tool_results.join("call-1.txt");
        std::fs::write(&target, "persisted").expect("tool result");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(false);
        let runtime_context = crate::RuntimeAgentPromptContext {
            tool_results_dir: Some(tool_results),
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                execute_tool_call(
                    &ToolCall {
                        id: "read-tool-result".to_owned(),
                        name: "read_file".to_owned(),
                        input: json!({"file_path": target.to_string_lossy().to_string()}),
                    },
                    &context,
                    &broker,
                )
                .await
            },
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("persisted"));
    }

    #[tokio::test]
    async fn read_file_allows_runtime_tasks_directory() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let tasks = tempdir.path().join("tasks");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let target = tasks.join("team-alpha").join("1.json");
        std::fs::create_dir_all(target.parent().expect("task dir")).expect("task dir");
        std::fs::write(&target, "{\"id\":\"1\"}").expect("task file");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(false);
        let runtime_context = crate::RuntimeAgentPromptContext {
            tasks_dir: Some(tasks),
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                execute_tool_call(
                    &ToolCall {
                        id: "read-task".to_owned(),
                        name: "read_file".to_owned(),
                        input: json!({"file_path": target.to_string_lossy().to_string()}),
                    },
                    &context,
                    &broker,
                )
                .await
            },
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("\"id\":\"1\""));
    }

    #[tokio::test]
    async fn read_file_explicit_ask_rule_bypasses_fast_path() {
        let tempdir = tempdir().expect("tempdir");
        std::fs::write(tempdir.path().join("notes.txt"), "notes").expect("notes");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: claude_core::PermissionMode::DontAsk,
            action: Some(claude_permissions::RuleAction::Ask),
            allow: false,
            matching_rule: None,
            requests,
            forced_requests: forced_requests.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-ask".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": "notes.txt"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert!(result.content.contains("forced recorded denial"));
        let forced = forced_requests.lock();
        assert_eq!(forced.len(), 1);
        assert_eq!(forced[0].tool_name, "read_file");
    }

    #[tokio::test]
    async fn edit_explicit_deny_rule_blocks_accept_edits_fast_path() {
        let tempdir = tempdir().expect("tempdir");
        let target = tempdir.path().join("notes.txt");
        std::fs::write(&target, "before").expect("notes");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: claude_core::PermissionMode::AcceptEdits,
            action: Some(claude_permissions::RuleAction::Deny),
            allow: false,
            matching_rule: None,
            requests: Arc::new(Mutex::new(Vec::new())),
            forced_requests: forced_requests.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "edit-deny".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"file_path": "notes.txt", "content": "after"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "before");
        assert_eq!(forced_requests.lock().len(), 1);
    }

    #[tokio::test]
    async fn outside_read_permission_request_carries_suggestions() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&outside, "secret").expect("outside");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RecordingPermissionBroker {
            allow: false,
            requests: requests.clone(),
            allow_feedback: None,
            allow_content_blocks: Vec::new(),
            deny_content_blocks: Vec::new(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-suggest".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": outside.to_string_lossy().to_string()}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        let requests = requests.lock();
        assert!(!requests[0].permission_suggestions.is_empty());
    }

    #[tokio::test]
    async fn dangerous_write_allow_rule_still_routes_through_forced_prompt() {
        let tempdir = tempdir().expect("tempdir");
        let git_dir = tempdir.path().join(".git");
        std::fs::create_dir_all(&git_dir).expect("git dir");
        let target = git_dir.join("config");
        std::fs::write(&target, "[core]\n").expect("git config");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: claude_core::PermissionMode::AcceptEdits,
            action: Some(claude_permissions::RuleAction::Allow),
            allow: false,
            matching_rule: Some(claude_permissions::SourceAwarePermissionRule {
                tool_pattern: "Edit(*)".to_owned(),
                action: claude_permissions::RuleAction::Allow,
                source: claude_permissions::RuleSource::Session,
            }),
            requests: Arc::new(Mutex::new(Vec::new())),
            forced_requests: forced_requests.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "dangerous-allow".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"file_path": target.to_string_lossy().to_string(), "content": "[core]\n\teditor = vim\n"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert_eq!(forced_requests.lock().len(), 1);
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "[core]\n");
    }

    #[tokio::test]
    async fn session_claude_allow_rule_bypasses_dangerous_write_precheck() {
        let tempdir = tempdir().expect("tempdir");
        let claude_dir = tempdir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("claude dir");
        let target = claude_dir.join("notes.json");
        std::fs::write(&target, "{\"before\":true}").expect("claude file");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: claude_core::PermissionMode::DontAsk,
            action: Some(claude_permissions::RuleAction::Allow),
            allow: true,
            matching_rule: Some(claude_permissions::SourceAwarePermissionRule {
                tool_pattern: "Edit(/.claude/**)".to_owned(),
                action: claude_permissions::RuleAction::Allow,
                source: claude_permissions::RuleSource::Session,
            }),
            requests: Arc::new(Mutex::new(Vec::new())),
            forced_requests: forced_requests.clone(),
        };

        let read = execute_tool_call(
            &ToolCall {
                id: "claude-read".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": target.to_string_lossy().to_string()}),
            },
            &context,
            &broker,
        )
        .await
        .expect("read before write should return result");
        assert!(!read.is_error, "{}", read.content);

        let result = execute_tool_call(
            &ToolCall {
                id: "claude-allow".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"file_path": target.to_string_lossy().to_string(), "content": "{\"after\":true}"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(!result.is_error, "{}", result.content);
        assert!(forced_requests.lock().is_empty());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read"),
            "{\"after\":true}"
        );
    }

    #[tokio::test]
    async fn relative_parent_read_routes_through_permission_broker() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&outside, "secret").expect("outside");
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RecordingPermissionBroker {
            allow: false,
            requests: requests.clone(),
            allow_feedback: None,
            allow_content_blocks: Vec::new(),
            deny_content_blocks: Vec::new(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "read-relative-parent".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path": "../outside.txt"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].blocked_path.as_deref(),
            Some(outside.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn command_hook_receives_json_input_over_stdin() {
        let tempdir = tempdir().expect("tempdir should work");
        let (shell, command) = if cfg!(windows) {
            (
                HookShell::PowerShell,
                "$inputJson = [Console]::In.ReadToEnd(); Write-Output $inputJson".to_owned(),
            )
        } else {
            (
                HookShell::Bash,
                "payload=$(cat); printf '%s' \"$payload\"".to_owned(),
            )
        };

        let result = execute_command_hook(&CommandHookExecutionRequest {
            event: HookEvent::SessionStart,
            command,
            cwd: tempdir.path().to_path_buf(),
            input: json!({"hello":"world"}),
            shell: Some(shell),
            timeout_secs: Some(5),
        })
        .await
        .expect("hook execution should work");

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("\"hello\":\"world\""));
    }

    // ── New tool tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        std::fs::write(tempdir.path().join("foo.rs"), "fn main() {}").expect("write should work");
        std::fs::write(tempdir.path().join("bar.txt"), "hello").expect("write should work");
        std::fs::create_dir(tempdir.path().join("src")).expect("mkdir should work");
        std::fs::write(tempdir.path().join("src/mod.rs"), "mod foo;").expect("write should work");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "glob".to_owned(),
                input: json!({"pattern": "**/*.rs"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("glob should work");

        assert!(!result.is_error, "glob error: {}", result.content);
        assert!(result.content.contains("foo.rs"), "should find foo.rs");
        assert!(result.content.contains("mod.rs"), "should find src/mod.rs");
        assert!(
            !result.content.contains("bar.txt"),
            "should not find bar.txt"
        );
    }

    #[tokio::test]
    async fn grep_finds_pattern_in_files() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        std::fs::write(
            tempdir.path().join("code.rs"),
            "fn hello() {}\nfn world() {}\n",
        )
        .expect("write should work");
        std::fs::write(tempdir.path().join("notes.txt"), "hello world\n")
            .expect("write should work");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        // Search with include filter and content mode to verify matching lines
        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "grep".to_owned(),
                input: json!({
                    "pattern": "fn hello",
                    "include": "*.rs",
                    "output_mode": "content"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("grep should work");

        assert!(!result.is_error, "grep error: {}", result.content);
        assert!(result.content.contains("code.rs"), "should find code.rs");
        assert!(
            result.content.contains("fn hello"),
            "should show matching line"
        );
        assert!(
            !result.content.contains("notes.txt"),
            "should not match notes.txt due to include filter"
        );
    }

    #[tokio::test]
    async fn web_fetch_handles_invalid_url() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "web_fetch".to_owned(),
                input: json!({"url": "not-a-valid-url"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("web_fetch should not panic");

        assert!(result.is_error, "invalid URL should produce an error");
    }

    #[tokio::test]
    async fn ask_user_returns_json_structure() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "ask_user".to_owned(),
                input: json!({
                    "questions": [{
                        "question": "What is your name?",
                        "header": "Name",
                        "options": [
                            {"label": "Alice", "description": "Use Alice as the name."},
                            {"label": "Bob", "description": "Use Bob as the name."}
                        ],
                        "multiSelect": false
                    }]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("ask_user should work");

        assert!(!result.is_error, "ask_user error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["type"], "ask_user");
        assert_eq!(parsed["questions"][0]["question"], "What is your name?");
        assert_eq!(parsed["questions"][0]["header"], "Name");
        let options = parsed["questions"][0]["options"]
            .as_array()
            .expect("should be array");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0]["label"], "Alice");
    }

    #[tokio::test]
    async fn ask_user_accepts_legacy_question_alias() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "ask_user".to_owned(),
                input: json!({
                    "question": "Which option?",
                    "suggestions": ["A", "B"]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("ask_user should work");

        assert!(!result.is_error, "ask_user error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["questions"][0]["question"], "Which option?");
        assert_eq!(parsed["questions"][0]["options"][0]["label"], "A");
    }

    #[tokio::test]
    async fn todo_write_persists_and_reads() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "todo_write".to_owned(),
                input: json!({
                    "todos": [
                        {"id": "1", "content": "Task 1", "status": "completed"},
                        {"id": "2", "content": "Task 2", "status": "in_progress"},
                        {"content": "Task 3", "status": "pending", "priority": "high"}
                    ]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("todo_write should work");

        assert!(!result.is_error, "todo_write error: {}", result.content);
        assert!(result.content.contains("3 todo items"));

        // Verify the file was persisted
        let todos_path = tempdir.path().join(".remote-code-rust").join("todos.json");
        assert!(todos_path.exists(), "todos.json should exist");
        let content = std::fs::read_to_string(&todos_path).expect("should read todos.json");
        let todos: Vec<serde_json::Value> =
            serde_json::from_str(&content).expect("should parse JSON");
        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0]["id"], "1");
        assert_eq!(todos[0]["status"], "completed");
        assert_eq!(todos[1]["status"], "in_progress");
        assert_eq!(todos[2]["status"], "pending");
        assert_eq!(todos[2]["priority"], "high");
    }

    #[tokio::test]
    async fn config_read_returns_current_settings() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        // Set a config value
        let set_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "config_read".to_owned(),
                input: json!({
                    "action": "set",
                    "key": "test_key",
                    "value": "test_value"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("config set should work");

        assert!(
            !set_result.is_error,
            "config set error: {}",
            set_result.content
        );

        // Get the config value
        let get_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "config_read".to_owned(),
                input: json!({
                    "action": "get",
                    "key": "test_key"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("config get should work");

        assert!(
            !get_result.is_error,
            "config get error: {}",
            get_result.content
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&get_result.content).expect("should be valid JSON");
        assert_eq!(parsed["test_key"], "test_value");
    }

    // ── Phase 2 tool tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn lsp_definitions_finds_function() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        std::fs::write(
            tempdir.path().join("lib.rs"),
            "fn my_function() -> i32 { 42 }\nstruct MyStruct { x: i32 }\n",
        )
        .expect("write should work");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "lsp".to_owned(),
                input: json!({
                    "action": "definitions",
                    "file_path": "lib.rs",
                    "symbol": "my_function"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("lsp definitions should work");

        assert!(!result.is_error, "lsp error: {}", result.content);
        assert!(
            result.content.contains("my_function"),
            "should find my_function definition"
        );
    }

    #[tokio::test]
    async fn lsp_references_finds_usages() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        std::fs::write(
            tempdir.path().join("main.rs"),
            "fn helper() {}\nfn main() { helper(); }\n",
        )
        .expect("write should work");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "lsp".to_owned(),
                input: json!({
                    "action": "references",
                    "file_path": "main.rs",
                    "symbol": "helper"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("lsp references should work");

        assert!(!result.is_error, "lsp error: {}", result.content);
        assert!(
            result.content.contains("helper"),
            "should find helper references"
        );
    }

    #[tokio::test]
    async fn lsp_hover_returns_context() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        std::fs::write(
            tempdir.path().join("code.rs"),
            "/// A great function.\nfn awesome_thing() -> bool { true }\n",
        )
        .expect("write should work");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "lsp".to_owned(),
                input: json!({
                    "action": "hover",
                    "file_path": "code.rs",
                    "symbol": "awesome_thing"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("lsp hover should work");

        assert!(!result.is_error, "lsp error: {}", result.content);
        assert!(
            result.content.contains("awesome_thing"),
            "should contain hover info for awesome_thing"
        );
    }

    #[tokio::test]
    async fn task_tools_crud_workflow() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let _task_list_guard =
            configure_task_list_for_tests(tempdir.path(), Some("task-tools-crud")).await;
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        // Create a task
        let create_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "task_create".to_owned(),
                input: json!({
                    "subject": "Build feature",
                    "description": "Implement the feature end-to-end"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("task_create should work");

        assert!(
            !create_result.is_error,
            "task_create error: {}",
            create_result.content
        );
        assert_eq!(
            create_result.content,
            "Task #1 created successfully: Build feature"
        );

        // Get the task
        let get_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "task_get".to_owned(),
                input: json!({"taskId": "1"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("task_get should work");

        assert!(
            !get_result.is_error,
            "task_get error: {}",
            get_result.content
        );
        assert!(
            get_result.content.contains("Build feature"),
            "should contain task title"
        );

        // Update the task
        let update_result = execute_tool_call(
            &ToolCall {
                id: "3".to_owned(),
                name: "task_update".to_owned(),
                input: json!({"taskId": "1", "status": "in_progress"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("task_update should work");

        assert!(
            !update_result.is_error,
            "task_update error: {}",
            update_result.content
        );

        // List tasks
        let list_result = execute_tool_call(
            &ToolCall {
                id: "4".to_owned(),
                name: "task_list".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("task_list should work");

        assert!(
            !list_result.is_error,
            "task_list error: {}",
            list_result.content
        );
        assert!(
            !list_result.content.contains("No tasks found"),
            "should have at least one task"
        );
        assert!(
            list_result
                .content
                .contains("#1 [in_progress] Build feature")
        );
    }

    #[tokio::test]
    async fn notebook_edit_modifies_cell() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let notebook = json!({
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["print('hello')"],
                    "outputs": [],
                    "execution_count": 1
                },
                {
                    "cell_type": "markdown",
                    "source": ["# Title"]
                }
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        });
        let nb_path = tempdir.path().join("test.ipynb");
        std::fs::write(
            &nb_path,
            serde_json::to_string_pretty(&notebook).expect("serialize notebook"),
        )
        .expect("write notebook");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let read = execute_tool_call(
            &ToolCall {
                id: "read-notebook".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": "test.ipynb"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("read notebook before edit");
        assert!(!read.is_error, "{}", read.content);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "notebook_edit".to_owned(),
                input: json!({
                    "notebook_path": "test.ipynb",
                    "cell_number": 0,
                    "new_source": "print('world')",
                    "cell_type": "code"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("notebook_edit should work");

        assert!(!result.is_error, "notebook_edit error: {}", result.content);

        // Verify the notebook was modified
        let updated: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&nb_path).expect("read notebook"))
                .expect("parse notebook");
        assert_eq!(
            updated["cells"][0]["source"],
            json!("print('world')"),
            "cell source should be updated"
        );
    }

    #[tokio::test]
    async fn notebook_edit_insert_and_delete_follow_research_fields() {
        let tempdir = tempdir().expect("tempdir");
        let notebook = json!({
            "cells": [
                {
                    "id": "first",
                    "cell_type": "code",
                    "source": "print('hello')",
                    "outputs": [],
                    "execution_count": 1
                }
            ],
            "metadata": {"language_info": {"name": "python"}},
            "nbformat": 4,
            "nbformat_minor": 5
        });
        let nb_path = tempdir.path().join("test.ipynb");
        std::fs::write(
            &nb_path,
            serde_json::to_string_pretty(&notebook).expect("serialize notebook"),
        )
        .expect("write notebook file");

        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let read = execute_tool_call(
            &ToolCall {
                id: "read-notebook".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"file_path": "test.ipynb"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("read notebook before edit");
        assert!(!read.is_error, "{}", read.content);

        let insert = execute_tool_call(
            &ToolCall {
                id: "insert".to_owned(),
                name: "notebook_edit".to_owned(),
                input: json!({
                    "notebook_path": "test.ipynb",
                    "cell_number": 0,
                    "new_source": "## Inserted",
                    "cell_type": "markdown",
                    "edit_mode": "insert"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("insert notebook cell");
        assert!(!insert.is_error, "{}", insert.content);

        let after_insert: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&nb_path).expect("read notebook after insert"),
        )
        .expect("parse notebook after insert");
        assert_eq!(after_insert["cells"][1]["cell_type"], json!("markdown"));
        assert_eq!(after_insert["cells"][1]["source"], json!("## Inserted"));

        let delete = execute_tool_call(
            &ToolCall {
                id: "delete".to_owned(),
                name: "notebook_edit".to_owned(),
                input: json!({
                    "notebook_path": "test.ipynb",
                    "cell_number": 0,
                    "new_source": "",
                    "edit_mode": "delete"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("delete notebook cell");
        assert!(!delete.is_error, "{}", delete.content);

        let after_delete: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&nb_path).expect("read notebook after delete"),
        )
        .expect("parse notebook after delete");
        assert_eq!(
            after_delete["cells"]
                .as_array()
                .expect("cells should be array")
                .len(),
            1
        );
        assert_eq!(after_delete["cells"][0]["source"], json!("## Inserted"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_message_returns_json_structure() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let _guard = seed_team(tempdir.path(), "message-team").await;
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "send_message".to_owned(),
                input: json!({
                    "team_name": "message-team",
                    "to": "agent-007",
                    "summary": "share greeting",
                    "message": "Hello from test"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("send_message should work");

        assert!(!result.is_error, "send_message error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["routing"]["target"], "@agent-007");
        assert_eq!(parsed["routing"]["summary"], "share greeting");
        assert_eq!(parsed["routing"]["content"], "Hello from test");
        let stored = mailbox::read_messages("message-team", "agent-007")
            .await
            .expect("read mailbox");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].summary.as_deref(), Some("share greeting"));
        assert_eq!(stored[0].content, "Hello from test");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn team_create_status_and_broadcast_round_trip() {
        let tempdir = tempdir().expect("tempdir should work");
        let _guard = seed_team(tempdir.path(), "seed-team").await;
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        create_team_via_tool(&context, &broker, "review-team").await;
        add_team_member_for_test(
            "review-team",
            "agent-1",
            "worker",
            tempdir.path(),
            Some("blue"),
        )
        .await;
        add_team_member_for_test("review-team", "agent-2", "reviewer", tempdir.path(), None).await;

        let created_team = team_helpers::read_team("review-team")
            .await
            .expect("read created team");
        assert_eq!(created_team.team_allowed_paths.len(), 1);
        assert_eq!(
            created_team.team_allowed_paths[0].path,
            tempdir.path().to_string_lossy()
        );

        let status_result = execute_tool_call(
            &ToolCall {
                id: "team-status".to_owned(),
                name: "team_status".to_owned(),
                input: json!({"team_name": "review-team"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("team_status should work");
        assert!(
            !status_result.is_error,
            "team_status error: {}",
            status_result.content
        );
        let status: serde_json::Value =
            serde_json::from_str(&status_result.content).expect("valid team_status json");
        assert_eq!(status["count"], 1);
        assert_eq!(
            status["teams"][0]["members"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(status["teams"][0]["lead"]["unread_messages"], 0);

        let peers_result = execute_tool_call(
            &ToolCall {
                id: "list-peers".to_owned(),
                name: "list_peers".to_owned(),
                input: json!({"team_name": "review-team"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("list_peers should work");
        assert!(
            !peers_result.is_error,
            "list_peers error: {}",
            peers_result.content
        );
        let peers: serde_json::Value =
            serde_json::from_str(&peers_result.content).expect("valid peers json");
        assert_eq!(peers["count"], 3);

        let broadcast_result = execute_tool_call(
            &ToolCall {
                id: "broadcast".to_owned(),
                name: "broadcast_message".to_owned(),
                input: json!({
                    "team_name": "review-team",
                    "sender": "lead",
                    "message": "All hands check-in",
                    "priority": "high"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("broadcast_message should work");
        assert!(
            !broadcast_result.is_error,
            "broadcast_message error: {}",
            broadcast_result.content
        );
        let broadcast: serde_json::Value =
            serde_json::from_str(&broadcast_result.content).expect("valid broadcast json");
        assert_eq!(broadcast["recipients"].as_array().map(Vec::len), Some(2));
        assert_eq!(broadcast["message_ids"].as_array().map(Vec::len), Some(2));

        let agent_1 = mailbox::read_messages("review-team", "agent-1")
            .await
            .expect("agent-1 mailbox");
        let agent_2 = mailbox::read_messages("review-team", "agent-2")
            .await
            .expect("agent-2 mailbox");
        assert_eq!(agent_1.len(), 1);
        assert_eq!(agent_2.len(), 1);
        assert_eq!(agent_1[0].priority.as_deref(), Some("high"));

        let follow_up = execute_tool_call(
            &ToolCall {
                id: "status-after-broadcast".to_owned(),
                name: "team_status".to_owned(),
                input: json!({"team_name": "review-team"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("team_status should work");
        let follow_up_status: serde_json::Value =
            serde_json::from_str(&follow_up.content).expect("valid team_status json");
        let members = follow_up_status["teams"][0]["members"]
            .as_array()
            .expect("members array");
        assert!(members.iter().all(|member| member["unread_messages"] == 1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn team_list_and_delete_cover_cleanup_and_permissions() {
        let tempdir = tempdir().expect("tempdir should work");
        let _guard = seed_team(tempdir.path(), "seed-team").await;
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let allow_broker = StaticPermissionBroker::new(true);
        create_team_via_tool(&context, &allow_broker, "cleanup-team").await;
        add_team_member_for_test("cleanup-team", "agent-1", "worker", tempdir.path(), None).await;

        let list_result = execute_tool_call(
            &ToolCall {
                id: "team-list".to_owned(),
                name: "team_list".to_owned(),
                input: json!({}),
            },
            &context,
            &allow_broker,
        )
        .await
        .expect("team_list should work");
        assert!(
            !list_result.is_error,
            "team_list error: {}",
            list_result.content
        );
        let list: serde_json::Value =
            serde_json::from_str(&list_result.content).expect("valid team_list json");
        assert!(
            list["teams"]
                .as_array()
                .expect("teams array")
                .iter()
                .any(|team| team["name"] == "cleanup-team")
        );

        let deny_broker = StaticPermissionBroker::new(false);
        let denied = execute_tool_call(
            &ToolCall {
                id: "team-delete-denied".to_owned(),
                name: "team_delete".to_owned(),
                input: json!({}),
            },
            &context,
            &deny_broker,
        )
        .await
        .expect("team_delete should return tool result");
        assert!(denied.is_error);
        assert!(team_helpers::read_team("cleanup-team").await.is_ok());

        let mut cleanup_team = team_helpers::read_team("cleanup-team")
            .await
            .expect("cleanup team should exist");
        for member in &mut cleanup_team.members {
            member.is_active = Some(false);
        }
        team_helpers::update_team(&cleanup_team)
            .await
            .expect("should mark cleanup team inactive");

        let deleted = execute_tool_call(
            &ToolCall {
                id: "team-delete".to_owned(),
                name: "team_delete".to_owned(),
                input: json!({}),
            },
            &context,
            &allow_broker,
        )
        .await
        .expect("team_delete should work");
        assert!(!deleted.is_error, "team_delete error: {}", deleted.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&deleted.content).expect("valid delete json");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["team_name"], "cleanup-team");
        assert!(
            parsed["message"]
                .as_str()
                .expect("message string")
                .contains("Cleaned up directories and worktrees")
        );
        assert!(team_helpers::read_team("cleanup-team").await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_tool_call_routes_agent_tool_to_host_runtime() {
        let tempdir = tempdir().expect("tempdir should work");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "verified".to_owned(),
                success: true,
                turns: 3,
                usage: UsageSummary::default(),
            },
        });
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: Some(runtime),
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "agent".to_owned(),
                input: json!({
                    "prompt": "Review the Rust changes and report the wiring path.",
                    "description": "Inspect agent runtime wiring"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("agent tool should execute");

        assert!(!result.is_error, "agent tool error: {}", result.content);
        assert_eq!(result.content, "verified");

        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "general-purpose");
        assert_eq!(request.max_turns, 200);
        assert!(!request.skip_transcript);
        assert!(request.allowed_tools.contains(&"read_file".to_owned()));
        assert!(request.allowed_tools.contains(&"write_file".to_owned()));
        assert!(request.allowed_tools.contains(&"edit_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"agent".to_owned()));
    }

    #[tokio::test]
    async fn execute_tool_call_returns_structured_agent_request_without_provider() {
        let tempdir = tempdir().expect("tempdir should work");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "agent-no-provider".to_owned(),
                name: "agent".to_owned(),
                input: json!({
                    "prompt": "Inspect the project and report one refactor target.",
                    "description": "Plan inspection",
                    "subagent_type": "Plan",
                    "model": "minimax-m2.7",
                    "tools": ["Read", "Grep"]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("agent tool should succeed");

        assert!(!result.is_error, "agent tool error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("valid sub_agent_request json");
        assert_eq!(parsed["type"], "sub_agent_request");
        assert_eq!(parsed["description"], "Plan inspection");
        assert_eq!(parsed["subagent_type"], "Plan");
        assert_eq!(parsed["model"], "minimax-m2.7");
        assert_eq!(parsed["allowed_tools"].as_array().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn execute_tool_call_routes_plan_agent_and_emits_progress() {
        let tempdir = tempdir().expect("tempdir should work");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "plan ready".to_owned(),
                success: true,
                turns: 3,
                usage: UsageSummary::default(),
            },
        });
        let progress = Arc::new(Mutex::new(Vec::<String>::new()));
        let progress_sink = Arc::clone(&progress);
        let progress_cb: Arc<super::ProgressCallback> = Arc::new(move |message| {
            progress_sink.lock().push(message.to_owned());
        });
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: Some(runtime),
            progress_cb: Some(progress_cb),
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "agent-plan".to_owned(),
                name: "agent".to_owned(),
                input: json!({
                    "prompt": "Inspect the current Rust project and identify one concrete refactor target.",
                    "description": "Plan refactor target",
                    "subagent_type": "Plan",
                    "model": "minimax-m2.7",
                    "tools": ["Read", "Grep"]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("agent tool should succeed");

        assert!(!result.is_error, "agent tool error: {}", result.content);
        assert_eq!(result.content, "plan ready");

        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "Plan");
        assert_eq!(request.description.as_deref(), Some("Plan refactor target"));
        assert_eq!(request.model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(request.working_dir, PathBuf::from(tempdir.path()));
        assert!(request.allowed_tools.contains(&"read_file".to_owned()));
        assert!(request.allowed_tools.contains(&"grep".to_owned()));
        assert!(!request.allowed_tools.contains(&"write_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"edit_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"agent".to_owned()));
        drop(requests);

        let progress = progress.lock();
        assert_eq!(progress.len(), 2);
        let started = crate::agent::parse_delegate_progress_event(&progress[0])
            .expect("start progress event");
        let completed = crate::agent::parse_delegate_progress_event(&progress[1])
            .expect("completed progress event");
        match started {
            crate::agent::DelegateProgressEvent::SubtaskStarted { description, .. } => {
                assert_eq!(description, "Plan refactor target");
            }
            other => panic!("expected started event, got {other:?}"),
        }
        match completed {
            crate::agent::DelegateProgressEvent::SubtaskCompleted {
                success,
                turns_used,
                ..
            } => {
                assert!(success);
                assert_eq!(turns_used, 3);
            }
            other => panic!("expected completed event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_tool_call_rejects_unknown_subagent_type() {
        let tempdir = tempdir().expect("tempdir should work");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: String::new(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: Some(runtime),
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "agent-unknown".to_owned(),
                name: "agent".to_owned(),
                input: json!({
                    "prompt": "Do work.",
                    "description": "Unknown agent",
                    "subagent_type": "does-not-exist"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("agent tool should return tool result");

        assert!(result.is_error);
        assert!(result.content.contains("unknown subagent_type"));
        assert!(requests.lock().is_empty());
    }

    #[tokio::test]
    async fn execute_tool_call_blocks_agent_runtime_when_permission_denied() {
        let tempdir = tempdir().expect("tempdir should work");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "should not run".to_owned(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: Some(runtime),
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(false);

        let result = execute_tool_call(
            &ToolCall {
                id: "agent-denied".to_owned(),
                name: "agent".to_owned(),
                input: json!({"prompt": "Inspect the project"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("agent tool should return tool result");

        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
        assert!(requests.lock().is_empty());
    }

    #[tokio::test]
    async fn plan_mode_tools_return_human_readable_messages() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let enter_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "enter_plan_mode".to_owned(),
                input: json!({"objective": "Plan the architecture"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("enter_plan_mode should work");

        assert!(
            !enter_result.is_error,
            "enter_plan_mode error: {}",
            enter_result.content
        );
        assert!(enter_result.content.contains("Entered plan mode"));
        assert!(enter_result.content.contains("Plan the architecture"));

        let exit_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "exit_plan_mode".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("exit_plan_mode should work");

        assert!(
            !exit_result.is_error,
            "exit_plan_mode error: {}",
            exit_result.content
        );
        assert!(
            exit_result
                .content
                .contains("User has approved exiting plan mode")
                || exit_result.content.contains("User has approved your plan")
        );
    }

    #[tokio::test]
    async fn enter_plan_mode_uses_forced_prompt_and_denial_blocks_runtime_transition() {
        let _guard = PLAN_MODE_RUNTIME_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let enter_calls = Arc::new(Mutex::new(Vec::new()));
        let exit_calls = Arc::new(Mutex::new(Vec::new()));
        let _runtime_guard =
            install_recording_plan_mode_runtime(enter_calls.clone(), exit_calls.clone());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: PermissionMode::DontAsk,
            action: None,
            allow: false,
            matching_rule: None,
            requests: requests.clone(),
            forced_requests: forced_requests.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "enter-plan-denied".to_owned(),
                name: "enter_plan_mode".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert!(result.content.contains("forced recorded denial"));
        assert!(requests.lock().is_empty());
        assert_eq!(forced_requests.lock().len(), 1);
        assert!(enter_calls.lock().is_empty());
        assert!(exit_calls.lock().is_empty());
    }

    #[tokio::test]
    async fn exit_plan_mode_uses_forced_prompt_and_denial_blocks_runtime_transition() {
        let _guard = PLAN_MODE_RUNTIME_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let enter_calls = Arc::new(Mutex::new(Vec::new()));
        let exit_calls = Arc::new(Mutex::new(Vec::new()));
        let _runtime_guard =
            install_recording_plan_mode_runtime(enter_calls.clone(), exit_calls.clone());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let forced_requests = Arc::new(Mutex::new(Vec::new()));
        let broker = RuleAwareBroker {
            mode: PermissionMode::Plan,
            action: None,
            allow: false,
            matching_rule: None,
            requests: requests.clone(),
            forced_requests: forced_requests.clone(),
        };

        let result = execute_tool_call(
            &ToolCall {
                id: "exit-plan-denied".to_owned(),
                name: "exit_plan_mode".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tool call should return result");

        assert!(result.is_error);
        assert!(result.content.contains("forced recorded denial"));
        assert!(requests.lock().is_empty());
        assert_eq!(forced_requests.lock().len(), 1);
        assert!(enter_calls.lock().is_empty());
        assert!(exit_calls.lock().is_empty());
    }

    #[tokio::test]
    async fn sleep_tool_completes() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "sleep".to_owned(),
                input: json!({"seconds": 0}),
            },
            &context,
            &broker,
        )
        .await
        .expect("sleep should work");

        assert!(!result.is_error, "sleep error: {}", result.content);
        assert!(
            result.content.contains("Slept for 0"),
            "should report sleeping"
        );
    }

    #[tokio::test]
    async fn snip_tool_saves_file() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "snip".to_owned(),
                input: json!({
                    "content": "fn main() { println!(\"hello\"); }",
                    "label": "hello world"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("snip should work");

        assert!(!result.is_error, "snip error: {}", result.content);
        assert!(
            result.content.contains("snippets"),
            "should mention snippets dir"
        );

        // Verify the file was created
        let snippets_dir = tempdir.path().join(".remote-code-rust").join("snippets");
        assert!(snippets_dir.exists(), "snippets dir should exist");
        let entries: Vec<_> = std::fs::read_dir(&snippets_dir)
            .expect("read snippets dir")
            .filter_map(Result::ok)
            .collect();
        assert!(!entries.is_empty(), "should have at least one snippet file");

        // Verify content
        let file_content = std::fs::read_to_string(entries[0].path()).expect("read snippet");
        assert!(
            file_content.contains("fn main()"),
            "snippet should contain the saved content"
        );
    }

    #[tokio::test]
    async fn discover_skills_returns_result() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "discover_skills".to_owned(),
                input: json!({"query": "test"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("discover_skills should work");

        assert!(
            !result.is_error,
            "discover_skills error: {}",
            result.content
        );
        // Empty workspace should return "No skills found"
        assert!(
            result.content.contains("No skills found"),
            "empty workspace should report no skills"
        );
    }

    #[tokio::test]
    async fn all_phase2_tools_are_registered() {
        let specs = builtin_tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();

        // Phase 2 tools
        assert!(names.contains(&"lsp"), "lsp should be registered");
        assert!(
            names.contains(&"task_create"),
            "task_create should be registered"
        );
        assert!(names.contains(&"task_get"), "task_get should be registered");
        assert!(
            names.contains(&"task_list"),
            "task_list should be registered"
        );
        assert!(
            names.contains(&"task_update"),
            "task_update should be registered"
        );
        assert!(
            names.contains(&"notebook_edit"),
            "notebook_edit should be registered"
        );
        assert!(
            names.contains(&"discover_skills"),
            "discover_skills should be registered"
        );
        assert!(
            names.contains(&"send_message"),
            "send_message should be registered"
        );
        assert!(
            names.contains(&"enter_plan_mode"),
            "enter_plan_mode should be registered"
        );
        assert!(
            names.contains(&"exit_plan_mode"),
            "exit_plan_mode should be registered"
        );
        assert!(names.contains(&"sleep"), "sleep should be registered");
        assert!(names.contains(&"snip"), "snip should be registered");
    }

    // ── Phase 4: Upstream gap-fill tool tests ────────────────────────────

    #[tokio::test]
    async fn powershell_tool_returns_result() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "powershell".to_owned(),
                input: json!({
                    "command": "Write-Output hello",
                    "timeout_ms": 30_000
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("powershell should work");

        if cfg!(windows) {
            assert!(!result.is_error, "powershell error: {}", result.content);
            assert!(
                result.content.contains("hello"),
                "should contain hello, got: {}",
                result.content
            );
        } else {
            assert!(
                result.content.contains("only available on Windows"),
                "non-windows should return hint"
            );
        }
    }

    #[tokio::test]
    async fn repl_tool_requires_language_and_code() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "repl".to_owned(),
                input: json!({"language": "python", "code": "print(42)"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("repl should not panic");

        // May succeed or fail depending on whether python is installed,
        // but should not panic.
        let _ = result;
    }

    #[tokio::test]
    async fn monitor_tool_returns_snapshot() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "monitor".to_owned(),
                input: json!({"target": "tasks"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("monitor should work");

        assert!(!result.is_error, "monitor error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["target"], "tasks");
    }

    #[tokio::test]
    async fn schedule_cron_saves_to_file() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "schedule_cron".to_owned(),
                input: json!({
                    "cron": "*/5 * * * *",
                    "prompt": "echo hello",
                    "recurring": true
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("schedule_cron should work");

        assert!(!result.is_error, "schedule_cron error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("schedule result should be JSON");
        assert_eq!(parsed["id"], "cron-1");
        assert_eq!(parsed["recurring"], true);
        let crons_path = tempdir.path().join(".remote-code-rust").join("crons.json");
        assert!(crons_path.exists(), "crons.json should exist");
    }

    #[tokio::test]
    async fn cron_list_and_delete_are_dispatched() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let create = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "CronCreate".to_owned(),
                input: json!({
                    "cron": "*/10 * * * *",
                    "prompt": "check status",
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("cron create should dispatch");
        assert!(!create.is_error, "cron create error: {}", create.content);

        let list = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "CronList".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("cron list should dispatch");
        assert!(!list.is_error, "cron list error: {}", list.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&list.content).expect("cron list should be JSON");
        assert_eq!(parsed["jobs"].as_array().expect("jobs array").len(), 1);

        let delete = execute_tool_call(
            &ToolCall {
                id: "3".to_owned(),
                name: "CronDelete".to_owned(),
                input: json!({"id": "cron-1"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("cron delete should dispatch");
        assert!(!delete.is_error, "cron delete error: {}", delete.content);

        let list_after = execute_tool_call(
            &ToolCall {
                id: "4".to_owned(),
                name: "CronList".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("cron list after delete should dispatch");
        let parsed: serde_json::Value =
            serde_json::from_str(&list_after.content).expect("cron list should be JSON");
        assert_eq!(parsed["jobs"].as_array().expect("jobs array").len(), 0);
    }

    #[tokio::test]
    async fn remote_trigger_sends_post() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "remote_trigger".to_owned(),
                input: json!({
                    "url": "http://127.0.0.1:1/does-not-exist",
                    "event": "test"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("remote_trigger should not panic");

        // Will likely fail to connect, but should not panic.
        let _ = result;
    }

    #[tokio::test]
    async fn workflow_create_and_status() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let create_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "workflow".to_owned(),
                input: json!({
                    "action": "create",
                    "name": "test-wf",
                    "steps": ["step1", "step2"]
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("workflow create should work");

        assert!(
            !create_result.is_error,
            "workflow create error: {}",
            create_result.content
        );

        let status_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "workflow".to_owned(),
                input: json!({
                    "action": "status",
                    "name": "test-wf"
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("workflow status should work");

        assert!(
            !status_result.is_error,
            "workflow status error: {}",
            status_result.content
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&status_result.content).expect("should be valid JSON");
        assert_eq!(parsed["name"], "test-wf");
        assert_eq!(parsed["status"], "created");
    }

    #[tokio::test]
    async fn workflow_run_requires_bash_permission_even_in_accept_edits_mode() {
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let allow_broker = StaticPermissionBroker::new(true);
        let create_result = execute_tool_call(
            &ToolCall {
                id: "wf-create".to_owned(),
                name: "workflow".to_owned(),
                input: json!({
                    "action": "create",
                    "name": "test-wf",
                    "steps": ["echo hello"]
                }),
            },
            &context,
            &allow_broker,
        )
        .await
        .expect("workflow create should work");
        assert!(
            !create_result.is_error,
            "workflow create error: {}",
            create_result.content
        );

        let accept_edits_broker = StaticPermissionBroker::from_mode(PermissionMode::AcceptEdits);
        let run_result = execute_tool_call(
            &ToolCall {
                id: "wf-run".to_owned(),
                name: "workflow".to_owned(),
                input: json!({
                    "action": "run",
                    "name": "test-wf"
                }),
            },
            &context,
            &accept_edits_broker,
        )
        .await
        .expect("workflow run should return a tool result");
        assert!(run_result.is_error, "workflow run unexpectedly succeeded");
        assert!(
            run_result.content.contains("Permission denied"),
            "unexpected workflow denial message: {}",
            run_result.content
        );

        let status_result = execute_tool_call(
            &ToolCall {
                id: "wf-status".to_owned(),
                name: "workflow".to_owned(),
                input: json!({
                    "action": "status",
                    "name": "test-wf"
                }),
            },
            &context,
            &accept_edits_broker,
        )
        .await
        .expect("workflow status should still work");
        assert!(
            !status_result.is_error,
            "workflow status error: {}",
            status_result.content
        );
    }

    #[tokio::test]
    async fn suggest_pr_returns_json() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "suggest_pr".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("suggest_pr should work");

        assert!(!result.is_error, "suggest_pr error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert!(parsed["suggested_title"].is_string());
    }

    #[tokio::test]
    async fn enter_and_exit_worktree_use_session_contract() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let init = ProcessCommand::new("git")
            .args(["-c", "init.defaultBranch=main", "init"])
            .current_dir(tempdir.path())
            .output()
            .expect("git init should run");
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        let commit = ProcessCommand::new("git")
            .args([
                "-c",
                "user.name=Remote Code Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .current_dir(tempdir.path())
            .output()
            .expect("git commit should run");
        assert!(
            commit.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr)
        );
        let context = ToolExecutionContext {
            cwd: tempdir.path().to_path_buf(),
            original_cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let enter_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "enter_worktree".to_owned(),
                input: json!({"name": "feature-x"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("enter_worktree should work");

        assert!(
            enter_result.is_error || enter_result.content.contains("worktreePath"),
            "enter_worktree should now return session worktree payload or a real git error"
        );

        let exit_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "exit_worktree".to_owned(),
                input: json!({"action": "keep"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("exit_worktree should work");

        assert!(
            !exit_result.is_error,
            "exit_worktree error: {}",
            exit_result.content
        );
        assert!(
            exit_result.content.contains("No-op"),
            "exit_worktree without active session should be a no-op payload"
        );
    }

    #[tokio::test]
    async fn brief_tool_truncates_content() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let long_content: String = "x".repeat(1000);
        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "brief".to_owned(),
                input: json!({
                    "content": long_content,
                    "max_length": 50
                }),
            },
            &context,
            &broker,
        )
        .await
        .expect("brief should work");

        assert!(!result.is_error, "brief error: {}", result.content);
        assert!(
            result.content.contains("truncated"),
            "should mention truncation"
        );
    }

    #[tokio::test]
    async fn ctx_inspect_lists_tools() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "ctx_inspect".to_owned(),
                input: json!({"action": "tools"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("ctx_inspect should work");

        assert!(!result.is_error, "ctx_inspect error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert!(parsed["total_tools"].as_u64().unwrap_or(0) > 40);
    }

    #[tokio::test]
    async fn ctx_inspect_reports_task_stack_context_metrics() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let task_stack = Arc::new(Mutex::new(claude_core::task_stack::TaskStack::default()));
        task_stack.lock().push_root(vec![
            ConversationEntry::system("system prompt"),
            ConversationEntry::user("hello from user"),
            ConversationEntry::assistant("assistant response"),
            ConversationEntry::tool("tool-1", "read_file", "file contents", false),
        ]);
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack,
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let tokens = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "ctx_inspect".to_owned(),
                input: json!({"action": "tokens"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("ctx_inspect tokens should work");
        let parsed_tokens: serde_json::Value =
            serde_json::from_str(&tokens.content).expect("tokens should be valid JSON");
        assert_eq!(parsed_tokens["status"], "available");
        assert_eq!(parsed_tokens["source"], "task_stack");
        assert_eq!(parsed_tokens["message_count"], 4);
        assert!(parsed_tokens["estimated_tokens"].as_u64().unwrap_or(0) > 0);
        assert_eq!(parsed_tokens["by_role"]["tool"], 1);

        let messages = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "ctx_inspect".to_owned(),
                input: json!({"action": "messages"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("ctx_inspect messages should work");
        let parsed_messages: serde_json::Value =
            serde_json::from_str(&messages.content).expect("messages should be valid JSON");
        assert_eq!(parsed_messages["status"], "available");
        assert_eq!(parsed_messages["last_role"], "tool");
        assert_eq!(parsed_messages["last_message"]["tool_name"], "read_file");
    }

    #[tokio::test]
    async fn ctx_inspect_reports_runtime_fork_snapshot_metrics() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let snapshot = SubAgentForkSnapshot {
            fork_context_messages: vec![
                claude_core::Message::from(ConversationEntry::user("hello")),
                claude_core::Message::from(ConversationEntry::assistant("response")),
            ],
            system_prompt: None,
            user_context: BTreeMap::new(),
            system_context: BTreeMap::new(),
        };

        let result = with_runtime_fork_snapshot_provider(
            Arc::new(move || snapshot.clone()),
            execute_tool_call(
                &ToolCall {
                    id: "1".to_owned(),
                    name: "ctx_inspect".to_owned(),
                    input: json!({"action": "messages"}),
                },
                &context,
                &broker,
            ),
        )
        .await
        .expect("ctx_inspect should work");

        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("messages should be valid JSON");
        assert_eq!(parsed["status"], "available");
        assert_eq!(parsed["source"], "runtime_fork_snapshot");
        assert_eq!(parsed["message_count"], 2);
        assert_eq!(parsed["by_role"]["assistant"], 1);
    }

    #[tokio::test]
    async fn ctx_inspect_reports_unavailable_without_snapshot() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "ctx_inspect".to_owned(),
                input: json!({"action": "tokens"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("ctx_inspect should work");

        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("tokens should be valid JSON");
        assert_eq!(parsed["status"], "unavailable");
        assert_eq!(parsed["action"], "tokens");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_peers_returns_json() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let _guard = seed_team(tempdir.path(), "peers-team").await;
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "list_peers".to_owned(),
                input: json!({"team_name": "peers-team"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("list_peers should work");

        assert!(!result.is_error, "list_peers error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert!(parsed["peers"].is_array());
        assert!(
            parsed["peers"]
                .as_array()
                .expect("peer array")
                .iter()
                .any(|peer| peer["name"] == "agent-007")
        );
    }

    #[tokio::test]
    async fn tungsten_tool_detects_project() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        // Create a Cargo.toml to trigger Rust detection.
        std::fs::write(
            tempdir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .expect("write should work");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "tungsten".to_owned(),
                input: json!({"action": "compile", "target": "."}),
            },
            &context,
            &broker,
        )
        .await
        .expect("tungsten should work");

        // May succeed or fail depending on cargo availability, but should not panic.
        let _ = result;
    }

    #[tokio::test]
    async fn overflow_test_generates_data() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "overflow_test".to_owned(),
                input: json!({"scenario": "large_output"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("overflow_test should work");

        assert!(!result.is_error, "overflow_test error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["scenario"], "large_output");
    }

    #[tokio::test]
    async fn synthetic_output_generates_csv() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "synthetic_output".to_owned(),
                input: json!({"type": "csv", "rows": 3}),
            },
            &context,
            &broker,
        )
        .await
        .expect("synthetic_output should work");

        assert!(
            !result.is_error,
            "synthetic_output error: {}",
            result.content
        );
        assert!(
            result.content.contains("id,name,value,active"),
            "should have CSV header"
        );
        assert!(result.content.contains("item_0"), "should have data rows");
    }

    #[tokio::test]
    async fn mcp_auth_login_and_status() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("mcp.toml"),
                server: McpServerConfig {
                    name: "test-server".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "python".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let login_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "mcp_auth".to_owned(),
                input: json!({"server": "test-server", "action": "login"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("mcp_auth tool dispatch should succeed");

        assert!(
            !login_result.is_error,
            "mcp_auth login should return a structured unsupported result: {}",
            login_result.content
        );
        let login_payload: serde_json::Value =
            serde_json::from_str(&login_result.content).expect("login should be valid JSON");
        assert_eq!(login_payload["status"], "unsupported");

        // Status should succeed and report not_authenticated.
        let status_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "mcp_auth".to_owned(),
                input: json!({"server": "test-server", "action": "status"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("mcp_auth status should work");

        assert!(
            !status_result.is_error,
            "mcp_auth status error: {}",
            status_result.content
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&status_result.content).expect("should be valid JSON");
        assert_eq!(parsed["status"], "not_authenticated");
        assert_eq!(parsed["transport"], "stdio");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");
    }

    #[tokio::test]
    async fn mcp_call_uses_runtime_inventory_from_policy() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("mcp.toml"),
                server: McpServerConfig {
                    name: "demo".to_owned(),
                    enabled: false,
                    transport: McpTransportConfig::Stdio {
                        command: "python".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "mcp_call".to_owned(),
                input: json!({"server": "demo", "tool": "search", "arguments": {"q": "rust"}}),
            },
            &context,
            &broker,
        )
        .await
        .expect("mcp_call should return a tool result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(
            result.is_error,
            "mcp_call should fail for disabled inventory"
        );
        assert!(
            result
                .content
                .contains("disabled by the current runtime inventory"),
            "unexpected error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn mcp_call_requires_runtime_inventory_in_policy() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("alpha.toml"),
                server: McpServerConfig {
                    name: "alpha".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "definitely-missing-mcp-command".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix {
                        supports_resources: true,
                        ..Default::default()
                    },
                    startup_timeout_secs: Some(1),
                    request_timeout_secs: Some(1),
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "mcp_call".to_owned(),
                input: json!({"server": "demo", "tool": "search", "arguments": {"q": "rust"}}),
            },
            &context,
            &broker,
        )
        .await
        .expect("mcp_call should return a tool result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(
            result.is_error,
            "mcp_call should fail without runtime inventory"
        );
        assert!(
            result
                .content
                .contains("is not available in the current runtime inventory"),
            "unexpected error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn mcp_call_rejects_ambiguous_runtime_inventory_entries() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let original_policy = super::current_tool_runtime_policy();
        let demo_server =
            |origin_kind: &str, origin_name: &str, config_name: &str| RuntimeMcpServerPolicyEntry {
                origin_kind: origin_kind.to_owned(),
                origin_name: origin_name.to_owned(),
                config_path: tempdir.path().join(config_name),
                server: McpServerConfig {
                    name: "demo".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "python".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            };
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![
                demo_server("cwd", "workspace", "cwd-mcp.toml"),
                demo_server("profile", "profile", "profile-mcp.toml"),
            ],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "mcp_call".to_owned(),
                input: json!({"server": "demo", "tool": "search", "arguments": {"q": "rust"}}),
            },
            &context,
            &broker,
        )
        .await
        .expect("mcp_call should return a tool result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(
            result.is_error,
            "mcp_call should fail for ambiguous runtime inventory"
        );
        assert!(
            result.content.contains("ambiguous across"),
            "unexpected error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn list_mcp_resources_returns_json() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("mcp.toml"),
                server: McpServerConfig {
                    name: "test".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "python".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix {
                        supports_resources: true,
                        ..Default::default()
                    },
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "list_mcp_resources".to_owned(),
                input: json!({"server": "test"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("list_mcp_resources should work");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(
            !result.is_error,
            "list_mcp_resources error: {}",
            result.content
        );
        let parsed: Value = serde_json::from_str(&result.content).expect("json result");
        assert!(
            parsed.is_array(),
            "list_mcp_resources should return a flat resource array"
        );
        assert!(
            parsed
                .as_array()
                .expect("array")
                .iter()
                .all(|resource| resource.get("server").and_then(Value::as_str) == Some("test")),
            "resources should be annotated with their provider server"
        );
    }

    #[tokio::test]
    async fn list_mcp_resources_without_server_returns_flat_array() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        let make_server = |name: &str| RuntimeMcpServerPolicyEntry {
            origin_kind: "profile".to_owned(),
            origin_name: "profile".to_owned(),
            config_path: tempdir.path().join(format!("{name}.toml")),
            server: McpServerConfig {
                name: name.to_owned(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "definitely-missing-mcp-command".to_owned(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                },
                capabilities: McpCapabilityMatrix {
                    supports_resources: true,
                    ..Default::default()
                },
                startup_timeout_secs: Some(1),
                request_timeout_secs: Some(1),
                metadata: Default::default(),
                oauth: None,
                tool_policy: McpToolPolicy::default(),
            },
        };
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![make_server("alpha"), make_server("beta")],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "list-all".to_owned(),
                name: "list_mcp_resources".to_owned(),
                input: json!({}),
            },
            &context,
            &broker,
        )
        .await
        .expect("list_mcp_resources should return result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(!result.is_error, "{}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).expect("json array");
        assert!(parsed.is_array(), "unexpected result: {}", result.content);
    }

    #[tokio::test]
    async fn list_mcp_resources_unknown_server_errors_like_upstream() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("alpha.toml"),
                server: McpServerConfig {
                    name: "alpha".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "definitely-missing-mcp-command".to_owned(),
                        args: Vec::new(),
                        cwd: None,
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix {
                        supports_resources: true,
                        ..Default::default()
                    },
                    startup_timeout_secs: Some(1),
                    request_timeout_secs: Some(1),
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "missing-server".to_owned(),
                name: "list_mcp_resources".to_owned(),
                input: json!({"server": "missing"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("list_mcp_resources should return result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(result.is_error);
        assert!(result.content.contains("Server \"missing\" not found"));
        assert!(result.content.contains("Available servers:"));
    }

    #[tokio::test]
    async fn read_mcp_resource_returns_json() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping read_mcp_resource test because Python is unavailable.");
            return;
        };
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let script = tempdir.path().join("mock_mcp_resource.py");
        fs::write(&script, mock_mcp_resource_server_script()).expect("mock mcp resource script");
        prefix_args.push(script.display().to_string());
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "profile".to_owned(),
                origin_name: "profile".to_owned(),
                config_path: tempdir.path().join("mcp.toml"),
                server: McpServerConfig {
                    name: "test".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: python,
                        args: prefix_args,
                        cwd: Some(tempdir.path().to_path_buf()),
                        env: Default::default(),
                    },
                    capabilities: McpCapabilityMatrix {
                        supports_resources: true,
                        ..Default::default()
                    },
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: Default::default(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "read_mcp_resource".to_owned(),
                input: json!({"server": "test", "uri": "test://resource"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("read_mcp_resource should work");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(
            !result.is_error,
            "read_mcp_resource error: {}",
            result.content
        );
        let payload: Value = serde_json::from_str(&result.content).expect("resource payload json");
        assert_eq!(
            payload["contents"][0]["text"].as_str(),
            Some("hello from resource")
        );
    }

    #[tokio::test]
    async fn read_mcp_resource_rejects_servers_without_resource_capability() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let tempdir = tempdir().expect("tempdir");
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![
                RuntimeMcpServerPolicyEntry {
                    origin_kind: "profile".to_owned(),
                    origin_name: "profile".to_owned(),
                    config_path: tempdir.path().join("mcp.toml"),
                    server: McpServerConfig {
                        name: "no-resources".to_owned(),
                        enabled: true,
                        transport: McpTransportConfig::Stdio {
                            command: "definitely-missing-mcp-command".to_owned(),
                            args: Vec::new(),
                            cwd: None,
                            env: Default::default(),
                        },
                        capabilities: McpCapabilityMatrix::default(),
                        startup_timeout_secs: Some(1),
                        request_timeout_secs: Some(1),
                        metadata: Default::default(),
                        oauth: None,
                        tool_policy: McpToolPolicy::default(),
                    },
                },
                RuntimeMcpServerPolicyEntry {
                    origin_kind: "profile".to_owned(),
                    origin_name: "profile".to_owned(),
                    config_path: tempdir.path().join("resourceful.toml"),
                    server: McpServerConfig {
                        name: "resourceful".to_owned(),
                        enabled: true,
                        transport: McpTransportConfig::Stdio {
                            command: "definitely-missing-mcp-command".to_owned(),
                            args: Vec::new(),
                            cwd: None,
                            env: Default::default(),
                        },
                        capabilities: McpCapabilityMatrix {
                            supports_resources: true,
                            ..Default::default()
                        },
                        startup_timeout_secs: Some(1),
                        request_timeout_secs: Some(1),
                        metadata: Default::default(),
                        oauth: None,
                        tool_policy: McpToolPolicy::default(),
                    },
                },
            ],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let result = execute_tool_call(
            &ToolCall {
                id: "unsupported-resource".to_owned(),
                name: "read_mcp_resource".to_owned(),
                input: json!({"server": "no-resources", "uri": "file:///README.md"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("read_mcp_resource should return result");

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(result.is_error);
        assert!(
            result
                .content
                .contains("does not advertise resources support"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn voice_input_returns_placeholder() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "voice_input".to_owned(),
                input: json!({"duration_secs": 3}),
            },
            &context,
            &broker,
        )
        .await
        .expect("voice_input should work");

        assert!(!result.is_error, "voice_input error: {}", result.content);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("should be valid JSON");
        assert_eq!(parsed["type"], "voice_input");
        assert_eq!(parsed["duration_secs"], 3);
    }

    #[tokio::test]
    async fn daemon_start_and_stop() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let context = ToolExecutionContext {
            original_cwd: tempdir.path().to_path_buf().clone(),
            cwd: tempdir.path().to_path_buf(),
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let broker = StaticPermissionBroker::new(true);

        let start_result = execute_tool_call(
            &ToolCall {
                id: "1".to_owned(),
                name: "daemon".to_owned(),
                input: json!({"action": "start", "command": "sleep 999"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("daemon start should work");

        assert!(
            !start_result.is_error,
            "daemon start error: {}",
            start_result.content
        );

        let status_result = execute_tool_call(
            &ToolCall {
                id: "2".to_owned(),
                name: "daemon".to_owned(),
                input: json!({"action": "status"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("daemon status should work");

        assert!(
            !status_result.is_error,
            "daemon status error: {}",
            status_result.content
        );

        let stop_result = execute_tool_call(
            &ToolCall {
                id: "3".to_owned(),
                name: "daemon".to_owned(),
                input: json!({"action": "stop"}),
            },
            &context,
            &broker,
        )
        .await
        .expect("daemon stop should work");

        assert!(
            !stop_result.is_error,
            "daemon stop error: {}",
            stop_result.content
        );
        assert!(
            stop_result.content.contains("Stopped 1 daemon"),
            "should stop 1 daemon"
        );
    }

    #[tokio::test]
    async fn all_phase4_tools_are_registered() {
        let specs = builtin_tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();

        assert!(
            names.contains(&"powershell"),
            "powershell should be registered"
        );
        assert!(names.contains(&"repl"), "repl should be registered");
        assert!(names.contains(&"monitor"), "monitor should be registered");
        assert!(
            names.contains(&"schedule_cron"),
            "schedule_cron should be registered"
        );
        assert!(
            names.contains(&"cron_delete"),
            "cron_delete should be registered"
        );
        assert!(
            names.contains(&"cron_list"),
            "cron_list should be registered"
        );
        assert!(
            names.contains(&"remote_trigger"),
            "remote_trigger should be registered"
        );
        assert!(names.contains(&"workflow"), "workflow should be registered");
        assert!(
            names.contains(&"suggest_pr"),
            "suggest_pr should be registered"
        );
        assert!(
            names.contains(&"enter_worktree"),
            "enter_worktree should be registered"
        );
        assert!(
            names.contains(&"exit_worktree"),
            "exit_worktree should be registered"
        );
        assert!(names.contains(&"brief"), "brief should be registered");
        assert!(
            names.contains(&"ctx_inspect"),
            "ctx_inspect should be registered"
        );
        assert!(
            names.contains(&"list_peers"),
            "list_peers should be registered"
        );
        assert!(names.contains(&"tungsten"), "tungsten should be registered");
        assert!(
            names.contains(&"overflow_test"),
            "overflow_test should be registered"
        );
        assert!(
            names.contains(&"synthetic_output"),
            "synthetic_output should be registered"
        );
        assert!(names.contains(&"mcp_auth"), "mcp_auth should be registered");
        assert!(
            !names.contains(&"list_mcp_resources"),
            "list_mcp_resources should be injected only for resource-capable MCP servers"
        );
        assert!(
            !names.contains(&"read_mcp_resource"),
            "read_mcp_resource should be injected only for resource-capable MCP servers"
        );
        assert!(
            names.contains(&"voice_input"),
            "voice_input should be registered"
        );
        assert!(names.contains(&"daemon"), "daemon should be registered");
    }

    #[test]
    fn tool_search_result_history_discovers_tools() {
        let discovered = extract_discovered_tool_names_from_conversation(&[
            claude_core::ConversationEntry::tool(
                "tool-1",
                "tool_search",
                r#"{"query":"web","results":[{"name":"web_fetch"},{"name":"web_search"}]}"#,
                false,
            ),
            claude_core::ConversationEntry::tool(
                "tool-2",
                "toolsearch",
                r#"{"query":"tasks","found_tools":[{"name":"task_create"}]}"#,
                false,
            ),
        ]);

        assert!(discovered.contains("web_fetch"));
        assert!(discovered.contains("web_search"));
        assert!(discovered.contains("task_create"));
    }

    #[test]
    fn carried_discovered_tools_are_merged_with_conversation_history() {
        let carried = std::collections::BTreeSet::from(["web_fetch".to_owned()]);
        let discovered = extract_discovered_tool_names(
            &[claude_core::ConversationEntry::tool(
                "tool-2",
                "toolsearch",
                r#"{"query":"tasks","found_tools":[{"name":"task_create"}]}"#,
                false,
            )],
            &carried,
        );

        assert!(discovered.contains("web_fetch"));
        assert!(discovered.contains("task_create"));
    }

    #[tokio::test]
    async fn tool_search_candidates_only_include_deferred_tools() {
        let specs = runtime_tool_search_candidate_specs().await;
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"web_fetch"));
        assert!(!names.contains(&"tool_search"));
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"bash_command"));
    }

    #[tokio::test]
    async fn visible_provider_pool_hides_deferred_tools_until_discovered() {
        let specs = runtime_visible_provider_tool_specs(&[]).await;
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"tool_search"));
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"web_fetch"));
        assert!(!names.contains(&"todo_write"));

        let discovered_specs =
            runtime_visible_provider_tool_specs(&[claude_core::ConversationEntry::tool(
                "tool-1",
                "tool_search",
                r#"{"query":"web","results":[{"name":"web_fetch"}]}"#,
                false,
            )])
            .await;
        let discovered_names = discovered_specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert!(discovered_names.contains(&"web_fetch"));
        assert!(!discovered_names.contains(&"todo_write"));
    }

    #[tokio::test]
    async fn task_local_runtime_policy_overlay_filters_provider_surface() {
        let outside = runtime_provider_tool_specs()
            .await
            .into_iter()
            .map(|spec| spec.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(outside.contains("read_file"));
        assert!(outside.contains("write_file"));

        let inside = with_tool_runtime_policy_overlay(
            ToolRuntimePolicyOverlay {
                allowed_tools: Some(vec!["read_file".to_owned()]),
                disallowed_tools: Vec::new(),
            },
            async {
                runtime_provider_tool_specs()
                    .await
                    .into_iter()
                    .map(|spec| spec.name)
                    .collect::<std::collections::BTreeSet<_>>()
            },
        )
        .await;
        assert!(inside.contains("read_file"));
        assert!(!inside.contains("write_file"));

        let after = runtime_provider_tool_specs()
            .await
            .into_iter()
            .map(|spec| spec.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(after.contains("write_file"));
    }

    #[test]
    fn runtime_policy_accepts_reference_style_tool_filter_names() {
        let policy = super::normalize_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: vec!["Bash(git:*)".to_owned(), "Read".to_owned()],
            disallowed_tools: vec!["WebFetch".to_owned(), "Bash(rm:*)".to_owned()],
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: Vec::new(),
            shell_policy: Default::default(),
        });

        assert!(policy.allowed_tools.contains(&"bash_command".to_owned()));
        assert!(policy.allowed_tools.contains(&"read_file".to_owned()));
        assert!(policy.disallowed_tools.contains(&"web_fetch".to_owned()));
        assert!(policy.disallowed_tools.contains(&"bash(rm:*)".to_owned()));
        assert!(super::tool_allowed_by_policy("bash_command", &policy));
        assert!(super::tool_allowed_by_policy("read_file", &policy));
        assert!(!super::tool_allowed_by_policy("write_file", &policy));
        assert!(!super::tool_allowed_by_policy("web_fetch", &policy));
    }

    #[tokio::test]
    async fn runtime_provider_tool_specs_prefers_task_local_mcp_observation_snapshot() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![fake_runtime_mcp_policy_entry("context7")],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;

        let outside = runtime_provider_tool_specs()
            .await
            .into_iter()
            .map(|spec| spec.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!outside.contains("mcp__context7__query_docs"));

        let observation = crate::mcp_runtime::RuntimeMcpObservation {
            servers: vec![crate::mcp_runtime::RuntimeMcpServerObservation {
                entry: crate::mcp_runtime::RuntimeMcpServerEntry {
                    origin_kind: "cwd",
                    origin_name: "workspace".to_owned(),
                    config_path: PathBuf::from(".mcp.json"),
                    server: fake_runtime_mcp_policy_entry("context7").server,
                },
                status: claude_ui_bridge::UiRuntimeMcpServerStatus::Connected,
                inspection: Some(McpServerInspection {
                    server_name: "context7".to_owned(),
                    protocol_version: "2025-03-26".to_owned(),
                    server_info: None,
                    capabilities: json!({}),
                    instructions: Some("Use the docs tools.".to_owned()),
                    tools: vec![McpToolDescriptor {
                        name: "query_docs".to_owned(),
                        title: None,
                        description: Some("Query docs".to_owned()),
                        input_schema: json!({"type": "object"}),
                        annotations: json!({}),
                    }],
                    prompts: Vec::new(),
                    resources: Vec::new(),
                }),
                error: None,
            }],
            warnings: Vec::new(),
        };

        let inside = crate::with_runtime_mcp_observation_provider(
            live_mcp_observation_provider(observation),
            async {
                runtime_provider_tool_specs()
                    .await
                    .into_iter()
                    .map(|spec| spec.name)
                    .collect::<std::collections::BTreeSet<_>>()
            },
        )
        .await;

        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(inside.contains("mcp__context7__query_docs"));
    }

    #[tokio::test]
    async fn runtime_mcp_catalog_exposes_prompt_commands_from_snapshot() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![fake_runtime_mcp_policy_entry("docs server")],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;

        let observation = crate::mcp_runtime::RuntimeMcpObservation {
            servers: vec![crate::mcp_runtime::RuntimeMcpServerObservation {
                entry: crate::mcp_runtime::RuntimeMcpServerEntry {
                    origin_kind: "cwd",
                    origin_name: "workspace".to_owned(),
                    config_path: PathBuf::from(".mcp.json"),
                    server: fake_runtime_mcp_policy_entry("docs server").server,
                },
                status: claude_ui_bridge::UiRuntimeMcpServerStatus::Connected,
                inspection: Some(McpServerInspection {
                    server_name: "docs server".to_owned(),
                    protocol_version: "2025-03-26".to_owned(),
                    server_info: None,
                    capabilities: json!({"prompts": {}}),
                    instructions: None,
                    tools: Vec::new(),
                    prompts: vec![McpPromptDescriptor {
                        name: "plan".to_owned(),
                        title: None,
                        description: Some("Plan with docs".to_owned()),
                        arguments: vec![McpPromptArgument {
                            name: "topic".to_owned(),
                            title: None,
                            description: None,
                            required: true,
                        }],
                    }],
                    resources: Vec::new(),
                }),
                error: None,
            }],
            warnings: Vec::new(),
        };

        let catalog = crate::with_runtime_mcp_observation_provider(
            live_mcp_observation_provider(observation),
            async { crate::mcp_catalog::runtime_mcp_catalog().await },
        )
        .await;

        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert_eq!(catalog.prompts.len(), 1);
        assert_eq!(catalog.prompts[0].command_name, "mcp__docs_server__plan");
        assert_eq!(catalog.prompts[0].arg_names, vec!["topic".to_owned()]);
    }

    #[tokio::test]
    async fn runtime_mcp_catalog_uses_failed_task_local_snapshot_without_retry() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![fake_runtime_mcp_policy_entry("context7")],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;

        let observation = crate::mcp_runtime::RuntimeMcpObservation {
            servers: vec![crate::mcp_runtime::RuntimeMcpServerObservation {
                entry: crate::mcp_runtime::RuntimeMcpServerEntry {
                    origin_kind: "cwd",
                    origin_name: "workspace".to_owned(),
                    config_path: PathBuf::from(".mcp.json"),
                    server: fake_runtime_mcp_policy_entry("context7").server,
                },
                status: claude_ui_bridge::UiRuntimeMcpServerStatus::Failed,
                inspection: None,
                error: Some("snapshot boom".to_owned()),
            }],
            warnings: Vec::new(),
        };

        let catalog = crate::with_runtime_mcp_observation_provider(
            live_mcp_observation_provider(observation),
            async { crate::mcp_catalog::runtime_mcp_catalog().await },
        )
        .await;

        crate::mcp_catalog::clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(catalog.tools.is_empty());
        assert!(
            catalog
                .warnings
                .iter()
                .any(|warning| warning.contains("snapshot boom"))
        );
    }

    #[tokio::test]
    async fn visible_provider_pool_keeps_full_tools_when_tool_search_is_unavailable() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX.lock().await;
        let original_policy = super::current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: vec!["tool_search".to_owned()],
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: Vec::new(),
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");

        let specs = runtime_visible_provider_tool_specs(&[]).await;
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"todo_write"));
        assert!(!names.contains(&"tool_search"));
    }

    #[tokio::test]
    async fn visible_provider_pool_keeps_carried_deferred_tools_after_compaction() {
        let carried = std::collections::BTreeSet::from(["web_fetch".to_owned()]);
        let specs = runtime_visible_provider_tool_specs_with_discovered_tools(&[], &carried).await;
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"tool_search"));
        assert!(names.contains(&"web_fetch"));
        assert!(!names.contains(&"todo_write"));
    }

    #[test]
    fn filesystem_precheck_includes_session_directory_overrides_from_broker() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let extra = tempdir.path().join("extra");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&extra).expect("extra");

        let fallback = StaticPermissionBroker::new(false);
        let layered = LayeredPermissionBroker::new(fallback, vec![]);
        layered
            .apply_permission_updates(&[PermissionUpdate::AddDirectories {
                destination: PermissionUpdateDestination::Session,
                directories: vec![extra.to_string_lossy().into_owned()],
            }])
            .expect("add session directory");

        let spec = ToolSpec {
            name: "read_file".to_owned(),
            protocol_name: "ReadFile".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: "read".to_owned(),
            requires_permission: false,
            input_schema: serde_json::json!({}),
        };
        let call = ToolCall {
            id: "tool-1".to_owned(),
            name: "read_file".to_owned(),
            input: serde_json::json!({"path": extra.join("allowed.txt")}),
        };
        let context = ToolExecutionContext {
            original_cwd: workspace.clone(),
            cwd: workspace,
            active_worktree_session: None,
            timeout_ms: 1_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let precheck = super::precheck_filesystem_permission(&spec, &call, &context, &layered);
        assert!(
            precheck.is_none(),
            "session directory should suppress prompt"
        );
    }
}
