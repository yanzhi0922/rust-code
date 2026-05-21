use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use claude_agents::definition::AgentDefinition;
use claude_agents::loader::load_all_agents_with_context;
use claude_agents::prompt::{format_agent_line, visible_agents};
use claude_auth::load_persisted_oauth_state;
use claude_config::settings_layers::load_runtime_settings;
use claude_config::{RuntimeConfig, validate_provider_config};
use claude_context::runtime_identity::{
    agent_swarms_enabled, code_guide_enabled, default_entrypoint, embedded_search_tools_enabled,
    entrypoint_is_non_interactive, explore_plan_agents_enabled, fork_subagent_enabled,
    runtime_user_type_from_env, show_agent_concurrency_note, verification_agent_enabled,
};
use claude_context::{
    RuntimeFeatureGates, RuntimeIdentityContext, RuntimeSubscriptionContext, RuntimeUserType,
};
use claude_core::{
    Attachment, AttachmentMediaType, ConversationEntry, ConversationRole, Message, PermissionMode,
    ProviderProtocol, ProviderResponse, ToolCall, ToolResult,
};
use claude_mcp::normalization::{build_mcp_tool_name, mcp_info_from_string};
use claude_mcp::serialization::{McpCliState, SerializedClient, SerializedTool};
use claude_mcp::{McpClientInfo, McpListChangedSurface};
use claude_permissions::PermissionBroker;
use claude_protocol::UsagePayload;
use claude_provider::{
    ConversationBackend, DiscoveredToolScope, provider_runtime_tool_specs_for_request,
    query_source::ProviderRequestContext,
};
use claude_query_engine::{
    EffortLevel, ProcessUserInputContext, ProviderInvocationMode, QueryCheckpointKind, QueryEngine,
    QueryEngineConfig, QueryObserver, QueryObserverEvent, QuerySource, ToolRunResult, ToolRunner,
};
use claude_runtime_prompt::{
    PromptRuntimeOverrides, RuntimePromptSettings, clear_runtime_system_prompt_state,
    conversation_with_runtime_user_context_with_settings, effective_allowed_tool_names,
    runtime_agent_listing_delta_enabled, runtime_deferred_tools_delta_enabled, runtime_env_truthy,
    runtime_mcp_instructions_delta_enabled,
};
use claude_session::SessionStore;
use claude_session::resume_state::{PendingToolCall, ResumeState};
use claude_session::session_memory::session_memory_dir;
use claude_tools::{
    FileStateCache, RuntimeAgentPromptContext, ToolExecutionContext, ToolRuntimePolicyOverlay,
    ToolSpec, current_runtime_agent_prompt_context, current_tool_runtime_policy, execute_tool_call,
    git::{apply_worktree_tool_result_to_runtime, sync_tool_context_from_runtime},
    mcp_runtime::{
        RuntimeMcpObservation, RuntimeMcpServerObservation, discover_runtime_mcp_servers,
        observe_runtime_mcp_servers,
    },
    plan_mode::normalize_exit_plan_mode_tool_calls,
    runtime_plan_mode::{
        RuntimePlanModeReminder, RuntimePlanModeReminderKind, build_runtime_plan_mode_reminder,
        build_runtime_plan_mode_reminder_content, inject_plan_mode_runtime_messages,
    },
    runtime_provider_tool_spec, runtime_provider_tool_specs,
    tool_result_storage::process_tool_result_content,
    with_runtime_agent_prompt_context_provider, with_runtime_fork_snapshot_provider,
    with_runtime_mcp_observation_provider, with_runtime_mcp_state_provider,
    with_tool_runtime_policy_overlay,
};
use claude_ui_bridge::UiRuntimeMcpServerStatus;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agents::build_remote_code_sub_agent_runtime;
use crate::conversation::{
    ContentReplacementBackend, PromptEventSink, PromptRunOutcome, PromptStreamEvent,
    discover_runtime_extensions, provision_content_replacement_state, session_tool_results_dir,
    truncate_preview,
};
use crate::hooks::{
    HookExecutionOptions, HookRunState, RuntimeHookDiscovery, apply_post_tool_hooks_with_options,
    apply_pre_tool_use_hooks_with_options,
};
use crate::repl_hook_runtime::{
    ReplHookRuntimeResources, apply_runtime_hook_context, register_repl_runtime_hooks,
};
use crate::session_memory_runtime::try_session_memory_compaction;
struct CompatSharedState {
    config: Mutex<RuntimeConfig>,
    conversation: Mutex<Vec<ConversationEntry>>,
    discovered_tool_scope: DiscoveredToolScope,
    hook_state: Mutex<HookRunState>,
    streamed_tool_calls: Mutex<HashSet<String>>,
    latest_streaming_usage: Mutex<Option<UsagePayload>>,
    latest_request_id: Mutex<Option<String>>,
    read_file_state: FileStateCache,
}

fn fork_snapshot_from_conversation(
    conversation: &[ConversationEntry],
) -> claude_core::SubAgentForkSnapshot {
    claude_core::SubAgentForkSnapshot {
        fork_context_messages: conversation.iter().cloned().map(Message::from).collect(),
        system_prompt: conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .map(|entry| entry.text.clone())
            .filter(|text| !text.trim().is_empty()),
        user_context: BTreeMap::new(),
        system_context: BTreeMap::new(),
    }
}

pub(crate) type CompatRunOverrides = PromptRuntimeOverrides;

#[derive(Debug, Clone)]
pub(crate) struct ForkCacheSafeParams {
    pub(crate) fork_context_messages: Vec<claude_core::Message>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) user_context: std::collections::BTreeMap<String, String>,
    pub(crate) system_context: std::collections::BTreeMap<String, String>,
    pub(crate) read_file_state: Option<FileStateCache>,
}

impl ForkCacheSafeParams {
    pub(crate) fn from_repl_hook_context(
        context: &claude_query_engine::stop_hooks::ReplHookContext,
    ) -> Self {
        Self {
            fork_context_messages: context.messages.clone(),
            system_prompt: context.system_prompt.clone(),
            user_context: context.user_context.clone(),
            system_context: context.system_context.clone(),
            read_file_state: None,
        }
    }

    pub(crate) fn from_conversation(conversation: &[ConversationEntry]) -> Self {
        let snapshot = fork_snapshot_from_conversation(conversation);
        Self {
            fork_context_messages: snapshot.fork_context_messages,
            system_prompt: snapshot.system_prompt,
            user_context: snapshot.user_context,
            system_context: snapshot.system_context,
            read_file_state: None,
        }
    }

    pub(crate) fn with_read_file_state(mut self, read_file_state: FileStateCache) -> Self {
        self.read_file_state = Some(read_file_state);
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompatExecutionOptions {
    pub(crate) persist_session: bool,
    pub(crate) persist_transcript: bool,
    pub(crate) persist_runtime_context: bool,
    pub(crate) persist_tool_results_dir: Option<PathBuf>,
    pub(crate) hook_options: HookExecutionOptions,
    pub(crate) query_source: QuerySource,
    pub(crate) agent_id: Option<claude_core::AgentId>,
    pub(crate) fork_snapshot: Option<ForkCacheSafeParams>,
}

impl Default for CompatExecutionOptions {
    fn default() -> Self {
        Self {
            persist_session: true,
            persist_transcript: true,
            persist_runtime_context: true,
            persist_tool_results_dir: None,
            hook_options: HookExecutionOptions::persistent(),
            query_source: QuerySource::User,
            agent_id: None,
            fork_snapshot: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ForkedPromptRunOutcome {
    pub(crate) messages: Vec<ConversationEntry>,
    pub(crate) usage: UsagePayload,
    pub(crate) cache_read_input_tokens: u64,
    pub(crate) cache_creation_input_tokens: u64,
    pub(crate) num_turns: u32,
    pub(crate) duration_ms: u64,
}

fn apply_exact_system_prompt(
    conversation: &mut Vec<ConversationEntry>,
    system_prompt: Option<&str>,
) {
    let Some(system_prompt) = system_prompt else {
        return;
    };
    let Some(system_prompt) = (!system_prompt.trim().is_empty()).then_some(system_prompt) else {
        return;
    };
    if let Some(system_entry) = conversation
        .iter_mut()
        .find(|entry| entry.role == ConversationRole::System)
    {
        system_entry.text = system_prompt.to_owned();
        system_entry.history_text = None;
        system_entry.content_blocks.clear();
        return;
    }
    conversation.insert(0, ConversationEntry::system(system_prompt));
}

fn augment_conversation_with_explicit_user_context(
    conversation: &[ConversationEntry],
    user_context: &BTreeMap<String, String>,
) -> Vec<ConversationEntry> {
    if user_context.is_empty()
        || conversation.iter().any(|entry| {
            entry.role == ConversationRole::User
                && entry.text.contains(
                    "As you answer the user's questions, you can use the following context:",
                )
        })
    {
        return conversation.to_vec();
    }

    let body = user_context
        .iter()
        .map(|(key, value)| format!("# {key}\n{value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let reminder = ConversationEntry::user(format!(
        "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n{body}\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>\n"
    ));

    let mut augmented = Vec::with_capacity(conversation.len() + 1);
    if let Some((first, rest)) = conversation.split_first()
        && first.role == ConversationRole::System
    {
        augmented.push(first.clone());
        augmented.push(reminder);
        augmented.extend(rest.iter().cloned());
        return augmented;
    }
    augmented.push(reminder);
    augmented.extend(conversation.iter().cloned());
    augmented
}

struct CompatObserver {
    store: Arc<SessionStore>,
    shared: Arc<CompatSharedState>,
    event_sink: Option<PromptEventSink>,
    include_partial_messages: bool,
    execution: CompatExecutionOptions,
}

struct CriticalReminderBackend {
    inner: Arc<dyn ConversationBackend>,
    reminder: String,
}

const PLAN_MODE_MARKER: &str = "## Plan Mode Active";
const PLAN_MODE_REENTRY_MARKER: &str = "## Re-entering Plan Mode";
const PLAN_MODE_ACTIVE_REMINDER_PREFIX: &str = "Plan mode is active. The user indicated";
const PLAN_MODE_SPARSE_REMINDER_PREFIX: &str = "Plan mode still active";
const DEFERRED_TOOLS_DELTA_MARKER: &str = "__remote_code_meta__:deferred_tools_delta:";
const AGENT_LISTING_DELTA_MARKER: &str = "__remote_code_meta__:agent_listing_delta:";
const MCP_INSTRUCTIONS_DELTA_MARKER: &str = "__remote_code_meta__:mcp_instructions_delta:";

static RUNTIME_MCP_SESSION_OBSERVATIONS: OnceLock<
    StdMutex<HashMap<Uuid, Arc<StdMutex<RuntimeMcpObservation>>>>,
> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDeferredToolsDeltaMarker {
    #[serde(default, alias = "added_names")]
    added_names: Vec<String>,
    #[serde(default, alias = "added_lines")]
    added_lines: Vec<String>,
    #[serde(default, alias = "removed_names")]
    removed_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeAgentListingDeltaMarker {
    #[serde(default, alias = "added_types")]
    added_types: Vec<String>,
    #[serde(default, alias = "added_lines")]
    added_lines: Vec<String>,
    #[serde(default, alias = "removed_types")]
    removed_types: Vec<String>,
    #[serde(default, alias = "is_initial")]
    is_initial: bool,
    #[serde(default, alias = "show_concurrency_note")]
    show_concurrency_note: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeMcpInstructionsDeltaMarker {
    #[serde(default, alias = "added_names")]
    added_names: Vec<String>,
    #[serde(default, alias = "added_blocks")]
    added_blocks: Vec<String>,
    #[serde(default, alias = "removed_names")]
    removed_names: Vec<String>,
}

fn wrap_in_system_reminder(content: &str) -> String {
    format!("<system-reminder>\n{content}\n</system-reminder>")
}

fn augment_conversation_with_critical_reminder(
    conversation: &[ConversationEntry],
    reminder: &str,
) -> Vec<ConversationEntry> {
    let reminder = reminder.trim();
    if reminder.is_empty() {
        return conversation.to_vec();
    }

    let reminder_entry = ConversationEntry::user(wrap_in_system_reminder(reminder));
    let mut augmented = Vec::with_capacity(conversation.len() + 1);
    if let Some((first, rest)) = conversation.split_first()
        && first.role == ConversationRole::System
    {
        augmented.push(first.clone());
        augmented.push(reminder_entry);
        augmented.extend(rest.iter().cloned());
        return augmented;
    }
    augmented.push(reminder_entry);
    augmented.extend(conversation.iter().cloned());
    augmented
}

#[async_trait]
impl ConversationBackend for CriticalReminderBackend {
    async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
        let augmented = augment_conversation_with_critical_reminder(conversation, &self.reminder);
        self.inner.complete(&augmented).await
    }

    async fn complete_with_context(
        &self,
        conversation: &[ConversationEntry],
        context: &ProviderRequestContext,
    ) -> Result<ProviderResponse> {
        let augmented = augment_conversation_with_critical_reminder(conversation, &self.reminder);
        self.inner.complete_with_context(&augmented, context).await
    }

    async fn complete_streaming(
        &self,
        conversation: &[ConversationEntry],
        callbacks: Option<claude_provider::StreamingCallbacks>,
    ) -> Result<ProviderResponse> {
        let augmented = augment_conversation_with_critical_reminder(conversation, &self.reminder);
        self.inner.complete_streaming(&augmented, callbacks).await
    }

    async fn complete_streaming_with_context(
        &self,
        conversation: &[ConversationEntry],
        callbacks: Option<claude_provider::StreamingCallbacks>,
        context: &ProviderRequestContext,
    ) -> Result<ProviderResponse> {
        let augmented = augment_conversation_with_critical_reminder(conversation, &self.reminder);
        self.inner
            .complete_streaming_with_context(&augmented, callbacks, context)
            .await
    }

    fn sub_agent_completion(&self) -> Arc<dyn claude_core::SubAgentCompletion> {
        self.inner.sub_agent_completion()
    }
}

fn runtime_mcp_session_observations()
-> &'static StdMutex<HashMap<Uuid, Arc<StdMutex<RuntimeMcpObservation>>>> {
    RUNTIME_MCP_SESSION_OBSERVATIONS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn runtime_mcp_observation_from_discovery(config: &RuntimeConfig) -> RuntimeMcpObservation {
    let discovery = discover_runtime_mcp_servers(config, &[]);
    RuntimeMcpObservation {
        servers: discovery
            .servers
            .into_iter()
            .map(|entry| RuntimeMcpServerObservation {
                status: if entry.server.enabled {
                    UiRuntimeMcpServerStatus::Pending
                } else {
                    UiRuntimeMcpServerStatus::Disabled
                },
                entry,
                inspection: None,
                error: None,
            })
            .collect(),
        warnings: discovery.warnings,
    }
}

fn runtime_mcp_state_from_observation(observation: &RuntimeMcpObservation) -> McpCliState {
    let clients = observation
        .servers
        .iter()
        .map(|server| SerializedClient {
            name: server.entry.server.name.clone(),
            connection_type: server.status.as_str().to_owned(),
            capabilities: None,
        })
        .collect::<Vec<_>>();
    let tools = observation
        .servers
        .iter()
        .flat_map(|server| {
            server.inspection.iter().flat_map(|inspection| {
                inspection.tools.iter().map(|tool| SerializedTool {
                    name: build_mcp_tool_name(&server.entry.server.name, &tool.name),
                    description: tool.description.clone().unwrap_or_default(),
                    input_json_schema: Some(tool.input_schema.clone()),
                    is_mcp: Some(true),
                    original_tool_name: Some(tool.name.clone()),
                })
            })
        })
        .collect::<Vec<_>>();

    McpCliState {
        clients,
        tools,
        ..McpCliState::default()
    }
}

fn runtime_mcp_observation_key_matches(
    left: &claude_tools::mcp_runtime::RuntimeMcpServerEntry,
    right: &claude_tools::mcp_runtime::RuntimeMcpServerEntry,
) -> bool {
    left.origin_kind == right.origin_kind
        && left.origin_name == right.origin_name
        && left.config_path == right.config_path
        && left.server == right.server
}

fn merge_runtime_mcp_observation_with_discovery(
    current: &RuntimeMcpObservation,
    discovery: RuntimeMcpObservation,
) -> RuntimeMcpObservation {
    let mut servers = Vec::with_capacity(discovery.servers.len());
    for discovered in discovery.servers {
        let preserved = current
            .servers
            .iter()
            .find(|existing| {
                runtime_mcp_observation_key_matches(&existing.entry, &discovered.entry)
            })
            .filter(|_| discovered.entry.server.enabled)
            .cloned();
        servers.push(preserved.unwrap_or(discovered));
    }

    RuntimeMcpObservation {
        servers,
        warnings: discovery.warnings,
    }
}

fn runtime_mcp_session_observation(config: &RuntimeConfig) -> Arc<StdMutex<RuntimeMcpObservation>> {
    let discovery = runtime_mcp_observation_from_discovery(config);
    let session_id = config.session_id;
    let sessions = runtime_mcp_session_observations();
    let mut sessions = sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = sessions.get(&session_id) {
        if let Ok(mut snapshot) = existing.lock() {
            *snapshot = merge_runtime_mcp_observation_with_discovery(&snapshot, discovery);
        }
        return Arc::clone(existing);
    }

    let snapshot = Arc::new(StdMutex::new(discovery));
    sessions.insert(session_id, Arc::clone(&snapshot));
    snapshot
}

fn refresh_runtime_mcp_session_observation(
    config: RuntimeConfig,
    observation: Arc<StdMutex<RuntimeMcpObservation>>,
) {
    tokio::spawn(async move {
        let refreshed =
            observe_runtime_mcp_servers(&config, &[], true, &McpClientInfo::default()).await;
        let changed = observation.lock().ok().map(|snapshot| {
            refreshed
                .servers
                .iter()
                .filter(|server| server.inspection.is_some())
                .filter_map(|server| {
                    let previous = snapshot.servers.iter().find(|existing| {
                        runtime_mcp_observation_key_matches(&existing.entry, &server.entry)
                    });
                    let changed = previous.and_then(|previous| previous.inspection.as_ref())
                        != server.inspection.as_ref();
                    changed.then(|| server.entry.server.name.clone())
                })
                .collect::<Vec<_>>()
        });
        if let Some(changed) = changed {
            for server_name in changed {
                handle_runtime_mcp_session_list_changed(
                    &config,
                    &observation,
                    &server_name,
                    McpListChangedSurface::Tools,
                )
                .await;
            }
        }
        if let Ok(mut snapshot) = observation.lock() {
            *snapshot = refreshed;
        }
    });
}

async fn refresh_runtime_mcp_session_observation_for_server(
    config: &RuntimeConfig,
    observation: &Arc<StdMutex<RuntimeMcpObservation>>,
    server_name: &str,
    connect: bool,
) {
    let refreshed =
        observe_runtime_mcp_servers(config, &[], connect, &McpClientInfo::default()).await;
    if let Ok(mut snapshot) = observation.lock() {
        let mut merged = snapshot.clone();
        merged.warnings = refreshed.warnings;
        for refreshed_server in refreshed.servers {
            if refreshed_server.entry.server.name != server_name {
                continue;
            }
            if let Some(existing) = merged.servers.iter_mut().find(|server| {
                runtime_mcp_observation_key_matches(&server.entry, &refreshed_server.entry)
            }) {
                *existing = refreshed_server;
            } else {
                merged.servers.push(refreshed_server);
            }
        }
        *snapshot = merged;
    }
}

async fn handle_runtime_mcp_session_list_changed(
    config: &RuntimeConfig,
    observation: &Arc<StdMutex<RuntimeMcpObservation>>,
    server_name: &str,
    surface: McpListChangedSurface,
) {
    claude_tools::mcp_catalog::handle_runtime_mcp_list_changed(server_name, surface).await;
    if matches!(surface, McpListChangedSurface::Resources) {
        return;
    }
    refresh_runtime_mcp_session_observation_for_server(config, observation, server_name, true)
        .await;
}

fn spawn_runtime_mcp_providers(
    config: &RuntimeConfig,
) -> (
    Arc<claude_tools::RuntimeMcpStateProvider>,
    Arc<claude_tools::RuntimeMcpObservationProvider>,
) {
    let observation = runtime_mcp_session_observation(config);
    refresh_runtime_mcp_session_observation(config.clone(), Arc::clone(&observation));

    let observation_for_state = Arc::clone(&observation);
    let state_provider = Arc::new(move || {
        observation_for_state
            .lock()
            .map(|snapshot| runtime_mcp_state_from_observation(&snapshot))
            .unwrap_or_default()
    });

    let observation_provider = Arc::new(move || {
        observation
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    });

    (state_provider, observation_provider)
}

#[cfg(test)]
fn clear_runtime_mcp_session_observation(session_id: Uuid) {
    if let Some(sessions) = RUNTIME_MCP_SESSION_OBSERVATIONS.get()
        && let Ok(mut sessions) = sessions.lock()
    {
        sessions.remove(&session_id);
    }
}

fn spawn_runtime_agent_prompt_context_provider(
    config: &RuntimeConfig,
    broker: &dyn PermissionBroker,
    tool_results_dir: Option<PathBuf>,
) -> Arc<claude_tools::RuntimeAgentPromptContextProvider> {
    let runtime_identity = build_runtime_identity_context(config);
    let inherited_context = current_runtime_agent_prompt_context();
    let prompt_settings = claude_runtime_prompt::RuntimePromptSettings::from_config(config);
    let mut agent_memory_dirs = inherited_context
        .as_ref()
        .map(|context| context.agent_memory_dirs.clone())
        .unwrap_or_default();
    agent_memory_dirs.extend(claude_runtime_prompt::agent_memory_dirs(config));
    agent_memory_dirs.sort();
    agent_memory_dirs.dedup();
    let context = RuntimeAgentPromptContext {
        user_agents_dir: inherited_context
            .as_ref()
            .and_then(|context| context.user_agents_dir.clone())
            .or_else(user_agents_dir),
        project_agents_dir: inherited_context
            .as_ref()
            .and_then(|context| context.project_agents_dir.clone())
            .or_else(|| Some(project_agents_dir(config))),
        additional_working_directories: inherited_context
            .as_ref()
            .map(|context| context.additional_working_directories.clone())
            .unwrap_or_default(),
        allowed_agent_types: None,
        denied_agent_types: extract_denied_agent_types(&broker.layered_rules()),
        is_coordinator: claude_agents::coordinator::is_coordinator_mode(),
        is_non_interactive: config.print_mode,
        list_via_attachment: runtime_identity.features.agent_listing_delta_enabled,
        runtime_identity,
        scratchpad_dir: prompt_settings.scratchpad_dir.map(PathBuf::from),
        session_memory_dir: Some(session_memory_dir(config)),
        tasks_dir: Some(claude_swarm::team_helpers::claude_config_home_dir().join("tasks")),
        tool_results_dir: Some(tool_results_dir.unwrap_or_else(|| {
            config
                .paths
                .sessions_dir
                .join(config.session_id.to_string())
                .join("tool-results")
        })),
        auto_memory_dir: prompt_settings
            .auto_memory_permission_dir
            .map(PathBuf::from),
        auto_memory_read_dir: prompt_settings.auto_memory_read_dir.map(PathBuf::from),
        team_memory_read_dir: prompt_settings.team_memory_read_dir.map(PathBuf::from),
        project_temp_dir: prompt_settings.project_temp_dir.map(PathBuf::from),
        preview_launch_config_path: Some(config.original_cwd.join(".claude").join("launch.json")),
        teams_dir: Some(claude_swarm::team_helpers::teams_base_dir()),
        agent_memory_dirs,
    };
    Arc::new(move || context.clone())
}

fn runtime_delta_entry<T>(
    marker_prefix: &str,
    payload: &T,
    text: String,
) -> Result<ConversationEntry>
where
    T: Serialize,
{
    let mut entry = ConversationEntry::user(String::new());
    entry.history_text = Some(format!(
        "{marker_prefix}{}",
        serde_json::to_string(payload)?
    ));
    entry.content_blocks = vec![serde_json::json!({
        "type": "text",
        "text": wrap_in_system_reminder(&text),
    })];
    Ok(entry)
}

fn parse_runtime_marker<T>(entry: &ConversationEntry, marker_prefix: &str) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let marker = entry
        .history_text
        .as_deref()
        .or_else(|| (!entry.text.is_empty()).then_some(entry.text.as_str()))?;
    let payload = marker.strip_prefix(marker_prefix)?;
    serde_json::from_str(payload).ok()
}

fn announced_deferred_tool_names(
    conversation: &[ConversationEntry],
) -> std::collections::BTreeSet<String> {
    let mut announced = std::collections::BTreeSet::new();
    for entry in conversation {
        let Some(delta) = parse_runtime_marker::<RuntimeDeferredToolsDeltaMarker>(
            entry,
            DEFERRED_TOOLS_DELTA_MARKER,
        ) else {
            continue;
        };
        for name in delta.added_names {
            announced.insert(name);
        }
        for name in delta.removed_names {
            announced.remove(name.as_str());
        }
    }
    announced
}

fn announced_mcp_instruction_names(
    conversation: &[ConversationEntry],
) -> std::collections::BTreeSet<String> {
    let mut announced = std::collections::BTreeSet::new();
    for entry in conversation {
        let Some(delta) = parse_runtime_marker::<RuntimeMcpInstructionsDeltaMarker>(
            entry,
            MCP_INSTRUCTIONS_DELTA_MARKER,
        ) else {
            continue;
        };
        for name in delta.added_names {
            announced.insert(name);
        }
        for name in delta.removed_names {
            announced.remove(name.as_str());
        }
    }
    announced
}

fn announced_agent_types(conversation: &[ConversationEntry]) -> std::collections::BTreeSet<String> {
    let mut announced = std::collections::BTreeSet::new();
    for entry in conversation {
        let Some(delta) = parse_runtime_marker::<RuntimeAgentListingDeltaMarker>(
            entry,
            AGENT_LISTING_DELTA_MARKER,
        ) else {
            continue;
        };
        for agent_type in delta.added_types {
            announced.insert(agent_type);
        }
        for agent_type in delta.removed_types {
            announced.remove(agent_type.as_str());
        }
    }
    announced
}

fn runtime_agent_listing_delta_active() -> bool {
    current_runtime_agent_prompt_context()
        .map(|context| context.list_via_attachment)
        .unwrap_or_else(runtime_agent_listing_delta_enabled)
}

fn extract_denied_agent_types(
    rules: &[claude_permissions::SourceAwarePermissionRule],
) -> Vec<String> {
    let mut denied = std::collections::BTreeSet::new();
    for rule in rules {
        if rule.action != claude_permissions::RuleAction::Deny {
            continue;
        }
        let pattern = rule.tool_pattern.trim();
        let Some(open) = pattern.find('(') else {
            continue;
        };
        let Some(close) = pattern.rfind(')') else {
            continue;
        };
        let tool_name = pattern[..open].trim();
        if !tool_name.eq_ignore_ascii_case("Agent") {
            continue;
        }
        let content = pattern[open + 1..close].trim();
        if !content.is_empty() {
            denied.insert(content.to_owned());
        }
    }
    denied.into_iter().collect()
}

fn project_agents_dir(config: &RuntimeConfig) -> std::path::PathBuf {
    config.cwd.join(".claude").join("agents")
}

fn user_agents_dir() -> Option<std::path::PathBuf> {
    BaseDirs::new().map(|base| base.home_dir().join(".claude").join("agents"))
}

fn resolved_runtime_entrypoint(
    _config: &RuntimeConfig,
    is_non_interactive: bool,
    env_entrypoint: Option<String>,
) -> String {
    env_entrypoint
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let args = std::env::args().skip(1).collect::<Vec<_>>();
            if matches!(args.as_slice(), [cmd, subcmd, ..] if cmd == "mcp" && subcmd == "serve") {
                return Some("mcp".to_owned());
            }
            Some(
                default_entrypoint(is_non_interactive, runtime_env_truthy("CLAUDE_CODE_ACTION"))
                    .to_owned(),
            )
        })
        .expect("runtime entrypoint fallback should always resolve")
}

fn build_runtime_identity_context(config: &RuntimeConfig) -> RuntimeIdentityContext {
    build_runtime_identity_context_with_entrypoint(
        config,
        std::env::var("CLAUDE_CODE_ENTRYPOINT").ok(),
    )
}

fn build_runtime_identity_context_with_entrypoint(
    config: &RuntimeConfig,
    env_entrypoint: Option<String>,
) -> RuntimeIdentityContext {
    let resolved_settings = load_runtime_settings(&config.settings_files).unwrap_or_default();
    let is_non_interactive = config.print_mode
        || !matches!(config.output_format, claude_core::OutputFormat::Text)
        || entrypoint_is_non_interactive(env_entrypoint.as_deref());
    let entrypoint = Some(resolved_runtime_entrypoint(
        config,
        is_non_interactive,
        env_entrypoint,
    ));
    let user_type = runtime_user_type_from_env(std::env::var("USER_TYPE").ok().as_deref());
    let explicit_agent_team_opt_in = runtime_env_truthy("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")
        || std::env::args().any(|arg| arg == "--agent-teams");
    let subscription_type = std::env::var("CLAUDE_CODE_SUBSCRIPTION_TYPE")
        .ok()
        .or_else(|| match user_type {
            RuntimeUserType::Ant => Some("max".to_owned()),
            _ => None,
        });
    let coordinator_mode = claude_agents::coordinator::is_coordinator_mode();
    let embedded_search_tools = embedded_search_tools_enabled(
        entrypoint.as_deref(),
        runtime_env_truthy("EMBEDDED_SEARCH_TOOLS"),
    );
    let code_guide = code_guide_enabled(entrypoint.as_deref());
    let persisted_oauth = if config.auth_source.is_none()
        && config
            .provider
            .base_url
            .as_deref()
            .map(claude_model::is_first_party_base_url)
            .unwrap_or(matches!(
                config.provider.protocol,
                ProviderProtocol::Anthropic
            )) {
        load_persisted_oauth_state(&config.paths.profile_dir).ok()
    } else {
        None
    };
    let persisted_identity = persisted_oauth
        .as_ref()
        .map(|state| state.runtime_identity_fragment())
        .unwrap_or_default();
    let effective_subscription_type = subscription_type
        .clone()
        .or_else(|| persisted_identity.subscription.subscription_type.clone());
    let show_concurrency_note = show_agent_concurrency_note(effective_subscription_type.as_deref());

    RuntimeIdentityContext {
        user_type: if matches!(user_type, RuntimeUserType::Unknown)
            && persisted_oauth
                .as_ref()
                .is_some_and(|state| state.has_tokens())
        {
            RuntimeUserType::External
        } else {
            user_type
        },
        entrypoint,
        provider_name: Some(config.provider.name.clone()),
        auth_source: config.auth_source.clone().or_else(|| {
            persisted_oauth
                .as_ref()
                .is_some_and(|state| state.has_tokens())
                .then_some("claude.ai".to_owned())
        }),
        is_first_party: config
            .provider
            .base_url
            .as_deref()
            .map(claude_model::is_first_party_base_url)
            .unwrap_or(matches!(
                config.provider.protocol,
                ProviderProtocol::Anthropic
            )),
        is_non_interactive,
        kairos_active: runtime_env_truthy("KAIROS_ACTIVE"),
        fast_mode_flag_opt_in: resolved_settings.fast_mode == Some(true),
        fast_mode_per_session_opt_in: resolved_settings.fast_mode_per_session_opt_in == Some(true),
        fast_mode_user_setting: resolved_settings.fast_mode,
        organization_uuid: persisted_identity.organization_uuid,
        account_uuid: persisted_identity.account_uuid,
        email: persisted_identity.email,
        subscription: RuntimeSubscriptionContext {
            subscription_type: effective_subscription_type,
            rate_limit_tier: std::env::var("CLAUDE_CODE_RATE_LIMIT_TIER")
                .ok()
                .or_else(|| persisted_identity.subscription.rate_limit_tier.clone()),
            billing_type: persisted_identity.subscription.billing_type,
            has_extra_usage_enabled: persisted_identity.subscription.has_extra_usage_enabled,
            display_name: persisted_identity.subscription.display_name,
            account_created_at: persisted_identity.subscription.account_created_at,
            subscription_created_at: persisted_identity.subscription.subscription_created_at,
        },
        features: RuntimeFeatureGates {
            embedded_search_tools,
            explore_plan_agents_enabled: explore_plan_agents_enabled(true, true),
            verification_agent_enabled: verification_agent_enabled(true, false),
            code_guide_enabled: code_guide,
            agent_swarms_enabled: agent_swarms_enabled(user_type, explicit_agent_team_opt_in, true),
            show_agent_concurrency_note: show_concurrency_note,
            mcp_instructions_delta_enabled: runtime_mcp_instructions_delta_enabled(),
            deferred_tools_delta_enabled: runtime_deferred_tools_delta_enabled(),
            agent_listing_delta_enabled: runtime_agent_listing_delta_enabled(),
            include_token_budget_prompt: runtime_env_truthy("REMOTE_CODE_TOKEN_BUDGET_PROMPT"),
            sdk_disable_builtin_agents: runtime_env_truthy(
                "CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS",
            ),
            is_fork_subagent_enabled: fork_subagent_enabled(
                true,
                coordinator_mode,
                is_non_interactive,
            ),
        },
        ..RuntimeIdentityContext::default()
    }
}

fn visible_mcp_server_names_from_specs(specs: &[ToolSpec]) -> Vec<String> {
    let mut servers = Vec::new();
    for spec in specs {
        let Some(info) = mcp_info_from_string(&spec.name) else {
            continue;
        };
        if servers.iter().any(|server| server == &info.server_name) {
            continue;
        }
        servers.push(info.server_name);
    }
    servers
}

fn agent_tool_allowed_by_runtime_policy() -> bool {
    let policy = current_tool_runtime_policy();
    if !policy.allowed_tools.is_empty()
        && !policy
            .allowed_tools
            .iter()
            .any(|tool| tool.eq_ignore_ascii_case("agent"))
    {
        return false;
    }
    !policy
        .disallowed_tools
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case("agent"))
}

fn build_agent_listing_delta_for_visible_agents(
    active_agents: &[AgentDefinition],
    allowed_agent_types: Option<&[String]>,
    available_mcp_servers: &[String],
    denied_agent_types: &[String],
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    let mut filtered = visible_agents(
        active_agents,
        allowed_agent_types,
        Some(available_mcp_servers),
        Some(denied_agent_types),
    );

    let announced = announced_agent_types(conversation);
    let is_initial = announced.is_empty();
    let current_types = filtered
        .iter()
        .map(|agent| agent.agent_type.clone())
        .collect::<std::collections::BTreeSet<_>>();
    filtered.sort_by(|left, right| left.agent_type.cmp(&right.agent_type));

    let added = filtered
        .into_iter()
        .filter(|agent| !announced.contains(agent.agent_type.as_str()))
        .collect::<Vec<_>>();
    let removed_types = announced
        .into_iter()
        .filter(|agent_type| !current_types.contains(agent_type))
        .collect::<Vec<_>>();

    if added.is_empty() && removed_types.is_empty() {
        return Ok(None);
    }

    let added_lines = added
        .iter()
        .map(|agent| format_agent_line(agent))
        .collect::<Vec<_>>();
    let show_concurrency_note = current_runtime_agent_prompt_context()
        .map(|context| {
            context
                .runtime_identity
                .features
                .show_agent_concurrency_note
        })
        .unwrap_or(true);
    let mut parts = Vec::new();
    if !added_lines.is_empty() {
        let header = if is_initial {
            "Available agent types for the Agent tool:"
        } else {
            "New agent types are now available for the Agent tool:"
        };
        parts.push(format!("{header}\n{}", added_lines.join("\n")));
    }
    if !removed_types.is_empty() {
        parts.push(format!(
            "The following agent types are no longer available:\n{}",
            removed_types
                .iter()
                .map(|agent_type| format!("- {agent_type}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if is_initial && show_concurrency_note {
        parts.push(
            "Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses."
                .to_owned(),
        );
    }

    Ok(Some(runtime_delta_entry(
        AGENT_LISTING_DELTA_MARKER,
        &RuntimeAgentListingDeltaMarker {
            added_types: added.iter().map(|agent| agent.agent_type.clone()).collect(),
            added_lines,
            removed_types,
            is_initial,
            show_concurrency_note,
        },
        parts.join("\n\n"),
    )?))
}

async fn build_agent_listing_delta_entry(
    config: &RuntimeConfig,
    broker: &dyn PermissionBroker,
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    let specs = runtime_provider_tool_specs().await;
    if !specs.iter().any(|spec| spec.name == "agent") {
        return Ok(None);
    }

    let runtime_context = current_runtime_agent_prompt_context();
    let user_dir = runtime_context
        .as_ref()
        .and_then(|context| context.user_agents_dir.clone())
        .or_else(user_agents_dir);
    let project_dir = runtime_context
        .as_ref()
        .and_then(|context| context.project_agents_dir.clone())
        .unwrap_or_else(|| project_agents_dir(config));
    let runtime_identity = runtime_context
        .as_ref()
        .map(|context| context.runtime_identity.clone())
        .unwrap_or_else(RuntimeIdentityContext::from_legacy_env);
    let definitions = load_all_agents_with_context(
        user_dir.as_deref(),
        Some(project_dir.as_path()),
        &runtime_identity,
    );
    let available_mcp_servers = visible_mcp_server_names_from_specs(&specs);
    let allowed_agent_types = runtime_context
        .as_ref()
        .and_then(|context| context.allowed_agent_types.clone());
    let denied_agent_types = runtime_context
        .as_ref()
        .map(|context| context.denied_agent_types.clone())
        .unwrap_or_else(|| extract_denied_agent_types(&broker.layered_rules()));
    build_agent_listing_delta_for_visible_agents(
        &definitions.active_agents,
        allowed_agent_types.as_deref(),
        &available_mcp_servers,
        &denied_agent_types,
        conversation,
    )
}

fn build_deferred_tools_delta_from_specs(
    specs: &[ToolSpec],
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    if !runtime_deferred_tools_delta_enabled() {
        return Ok(None);
    }

    let has_tool_search = specs.iter().any(ToolSpec::is_tool_search);
    if !has_tool_search {
        return Ok(None);
    }

    let mut deferred_specs = specs
        .iter()
        .filter(|spec| spec.is_deferred())
        .cloned()
        .collect::<Vec<_>>();
    deferred_specs.sort_by(|left, right| left.provider_wire_name().cmp(right.provider_wire_name()));

    let announced = announced_deferred_tool_names(conversation);
    let deferred_names = deferred_specs
        .iter()
        .map(|spec| spec.provider_wire_name().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let pool_names = specs
        .iter()
        .map(|spec| spec.provider_wire_name().to_owned())
        .collect::<std::collections::BTreeSet<_>>();

    let added_names = deferred_specs
        .iter()
        .filter(|spec| !announced.contains(spec.provider_wire_name()))
        .map(|spec| spec.provider_wire_name().to_owned())
        .collect::<Vec<_>>();
    let removed_names = announced
        .into_iter()
        .filter(|name| !deferred_names.contains(name) && !pool_names.contains(name))
        .collect::<Vec<_>>();

    if added_names.is_empty() && removed_names.is_empty() {
        return Ok(None);
    }
    let added_lines = added_names.clone();

    let mut parts = Vec::new();
    if !added_lines.is_empty() {
        parts.push(format!(
            "The following deferred tools are now available via ToolSearch:\n{}",
            added_lines.join("\n")
        ));
    }
    if !removed_names.is_empty() {
        parts.push(format!(
            "The following deferred tools are no longer available (their MCP server disconnected). Do not search for them — ToolSearch will return no match:\n{}",
            removed_names.join("\n")
        ));
    }

    Ok(Some(runtime_delta_entry(
        DEFERRED_TOOLS_DELTA_MARKER,
        &RuntimeDeferredToolsDeltaMarker {
            added_names,
            added_lines,
            removed_names,
        },
        parts.join("\n\n"),
    )?))
}

async fn build_deferred_tools_delta_entry(
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    let specs = runtime_provider_tool_specs().await;
    build_deferred_tools_delta_from_specs(&specs, conversation)
}

fn build_mcp_instruction_blocks(
    catalog: claude_tools::mcp_catalog::RuntimeMcpCatalog,
) -> Vec<(String, String)> {
    let mut blocks = catalog
        .clients
        .into_iter()
        .filter_map(|client| {
            let instructions = client.instructions?;
            let trimmed = instructions.trim();
            (!trimmed.is_empty()).then(|| {
                let server_name = client.server_name;
                let block = format!("## {}\n{}", server_name, trimmed);
                (server_name, block)
            })
        })
        .collect::<Vec<_>>();
    blocks.sort_by(|left, right| left.0.cmp(&right.0));
    blocks
}

fn build_mcp_instructions_delta_from_blocks(
    blocks: Vec<(String, String)>,
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    if !runtime_mcp_instructions_delta_enabled() {
        return Ok(None);
    }

    let announced = announced_mcp_instruction_names(conversation);
    let current_names = blocks
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let added = blocks
        .iter()
        .filter(|(name, _)| !announced.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let removed_names = announced
        .into_iter()
        .filter(|name| !current_names.contains(name))
        .collect::<Vec<_>>();

    if added.is_empty() && removed_names.is_empty() {
        return Ok(None);
    }

    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!(
            "# MCP Server Instructions\n\nThe following MCP servers have provided instructions for how to use their tools and resources:\n\n{}",
            added
                .iter()
                .map(|(_, block)| block.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        ));
    }
    if !removed_names.is_empty() {
        parts.push(format!(
            "The following MCP servers have disconnected. Their instructions above no longer apply:\n{}",
            removed_names.join("\n")
        ));
    }

    Ok(Some(runtime_delta_entry(
        MCP_INSTRUCTIONS_DELTA_MARKER,
        &RuntimeMcpInstructionsDeltaMarker {
            added_names: added.iter().map(|(name, _)| name.clone()).collect(),
            added_blocks: added.iter().map(|(_, block)| block.clone()).collect(),
            removed_names,
        },
        parts.join("\n\n"),
    )?))
}

async fn build_mcp_instructions_delta_entry(
    conversation: &[ConversationEntry],
) -> Result<Option<ConversationEntry>> {
    let catalog = claude_tools::mcp_catalog::runtime_mcp_catalog().await;
    build_mcp_instructions_delta_from_blocks(build_mcp_instruction_blocks(catalog), conversation)
}

async fn inject_runtime_delta_messages(
    config: &RuntimeConfig,
    store: &SessionStore,
    broker: &dyn PermissionBroker,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<()> {
    inject_deferred_tools_delta_message(config.session_id, store, conversation).await?;
    inject_agent_listing_delta_message(config, store, broker, conversation).await?;
    inject_mcp_instructions_delta_message(config.session_id, store, conversation).await?;
    Ok(())
}

async fn inject_agent_listing_delta_message(
    config: &RuntimeConfig,
    store: &SessionStore,
    broker: &dyn PermissionBroker,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<()> {
    if !runtime_agent_listing_delta_active() {
        return Ok(());
    }
    let Some(entry) = build_agent_listing_delta_entry(config, broker, conversation).await? else {
        return Ok(());
    };
    store.append_conversation_entry(config.session_id, &entry)?;
    conversation.push(entry);
    Ok(())
}

async fn inject_deferred_tools_delta_message(
    session_id: uuid::Uuid,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<()> {
    let Some(entry) = build_deferred_tools_delta_entry(conversation).await? else {
        return Ok(());
    };
    store.append_conversation_entry(session_id, &entry)?;
    conversation.push(entry);
    Ok(())
}

async fn inject_mcp_instructions_delta_message(
    session_id: uuid::Uuid,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<()> {
    let Some(entry) = build_mcp_instructions_delta_entry(conversation).await? else {
        return Ok(());
    };
    store.append_conversation_entry(session_id, &entry)?;
    conversation.push(entry);
    Ok(())
}

async fn augment_post_compact_conversation_for_runtime(
    config: &RuntimeConfig,
    broker: &dyn PermissionBroker,
    store: &SessionStore,
    session_id: uuid::Uuid,
    mut conversation: Vec<ConversationEntry>,
) -> Vec<ConversationEntry> {
    append_post_compact_plan_attachment(store, session_id, &mut conversation);
    append_post_compact_plan_mode_reminder(store, session_id, &mut conversation);
    append_post_compact_deferred_tools_delta(&mut conversation).await;
    append_post_compact_agent_listing_delta(config, broker, &mut conversation).await;
    append_post_compact_mcp_instructions_delta(&mut conversation).await;
    conversation
}

async fn append_post_compact_deferred_tools_delta(conversation: &mut Vec<ConversationEntry>) {
    let Ok(entry) = build_deferred_tools_delta_entry(conversation).await else {
        return;
    };
    if let Some(entry) = entry {
        conversation.push(entry);
    }
}

async fn append_post_compact_agent_listing_delta(
    config: &RuntimeConfig,
    broker: &dyn PermissionBroker,
    conversation: &mut Vec<ConversationEntry>,
) {
    if !runtime_agent_listing_delta_active() || !agent_tool_allowed_by_runtime_policy() {
        return;
    }

    let Ok(entry) = build_agent_listing_delta_entry(config, broker, conversation).await else {
        return;
    };
    if let Some(entry) = entry {
        conversation.push(entry);
    }
}

async fn append_post_compact_mcp_instructions_delta(conversation: &mut Vec<ConversationEntry>) {
    let Ok(entry) = build_mcp_instructions_delta_entry(conversation).await else {
        return;
    };
    if let Some(entry) = entry {
        conversation.push(entry);
    }
}

fn append_post_compact_plan_attachment(
    store: &SessionStore,
    session_id: uuid::Uuid,
    conversation: &mut Vec<ConversationEntry>,
) {
    let Some(state) = store.load_plan_mode_state(session_id).ok().flatten() else {
        return;
    };
    let Some(plan_file_path) = state.plan_file_path else {
        return;
    };
    let plan_file_name = plan_file_path.display().to_string();
    let already_attached = conversation.iter().any(|entry| {
        entry.role == ConversationRole::User
            && entry
                .attachments
                .iter()
                .any(|attachment| attachment.filename.as_deref() == Some(plan_file_name.as_str()))
    });
    if already_attached {
        return;
    }

    let Ok(plan_content) = fs::read_to_string(&plan_file_path) else {
        return;
    };
    if plan_content.trim().is_empty() {
        return;
    }

    conversation.push(ConversationEntry::user_with_attachments(
        format!("Plan file reference: {plan_file_name}"),
        vec![Attachment::from_bytes(
            AttachmentMediaType::ApplicationPdf,
            plan_content.as_bytes(),
            Some(plan_file_name),
        )],
    ));
}

fn append_post_compact_plan_mode_reminder(
    store: &SessionStore,
    session_id: uuid::Uuid,
    conversation: &mut Vec<ConversationEntry>,
) {
    let Some(state) = store.load_plan_mode_state(session_id).ok().flatten() else {
        return;
    };

    let plan_file_path = state
        .plan_file_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(missing)".to_owned());

    let existing_plan_mode_marker = conversation.iter().any(|entry| {
        entry.role == ConversationRole::User
            && (entry.text.contains(PLAN_MODE_MARKER)
                || entry.text.contains(PLAN_MODE_REENTRY_MARKER)
                || entry.text.contains(PLAN_MODE_ACTIVE_REMINDER_PREFIX)
                || entry.text.contains(PLAN_MODE_SPARSE_REMINDER_PREFIX))
    });
    if state.current_permission_mode == PermissionMode::Plan {
        if existing_plan_mode_marker {
            return;
        }

        let plan_file_exists = state
            .plan_file_path
            .as_ref()
            .is_some_and(|path| path.exists());
        let reminder = if state.has_exited_plan_mode && plan_file_exists {
            build_runtime_plan_mode_reminder(RuntimePlanModeReminder {
                kind: RuntimePlanModeReminderKind::Reentry,
                plan_file_path,
                plan_exists: true,
                is_sub_agent: false,
            })
        } else {
            build_runtime_plan_mode_reminder(RuntimePlanModeReminder {
                kind: RuntimePlanModeReminderKind::Full,
                plan_file_path,
                plan_exists: plan_file_exists,
                is_sub_agent: false,
            })
        };
        conversation.push(ConversationEntry::user(reminder));
    } else if state.needs_plan_mode_exit_attachment
        && !conversation.iter().any(|entry| {
            entry.role == ConversationRole::User
                && entry
                    .text
                    .contains(&build_runtime_plan_mode_reminder_content(
                        RuntimePlanModeReminder {
                            kind: RuntimePlanModeReminderKind::Exit,
                            plan_file_path: plan_file_path.clone(),
                            plan_exists: state
                                .plan_file_path
                                .as_ref()
                                .is_some_and(|path| path.exists()),
                            is_sub_agent: false,
                        },
                    ))
        })
    {
        conversation.push(ConversationEntry::user(build_runtime_plan_mode_reminder(
            RuntimePlanModeReminder {
                kind: RuntimePlanModeReminderKind::Exit,
                plan_file_path,
                plan_exists: state
                    .plan_file_path
                    .as_ref()
                    .is_some_and(|path| path.exists()),
                is_sub_agent: false,
            },
        )));
    }
}

fn load_query_runtime_prompt_settings(config: &RuntimeConfig) -> Result<RuntimePromptSettings> {
    let prompt_settings = load_runtime_settings(&config.settings_files)?;
    let mut settings = RuntimePromptSettings::from_config(config);
    if let Some(runtime_context) = current_runtime_agent_prompt_context() {
        settings.additional_working_directories = runtime_context.additional_working_directories;
    }
    let runtime_identity = build_runtime_identity_context(config);
    settings.language = config.language.clone().or(prompt_settings.language.clone());
    settings.output_style = config
        .output_style
        .clone()
        .or(prompt_settings.output_style.clone());
    settings.proactive_active = config.proactive_active
        || runtime_env_truthy("REMOTE_CODE_PROACTIVE")
        || runtime_env_truthy("CLAUDE_CODE_PROACTIVE");
    settings.brief_enabled = config.brief_enabled
        || runtime_env_truthy("REMOTE_CODE_BRIEF")
        || runtime_env_truthy("CLAUDE_CODE_BRIEF");
    settings.mcp_instructions_delta_enabled =
        runtime_identity.features.mcp_instructions_delta_enabled;
    settings.is_non_interactive =
        config.print_mode || !matches!(config.output_format, claude_core::OutputFormat::Text);
    settings.include_token_budget_prompt = runtime_identity.features.include_token_budget_prompt;
    settings.runtime_identity = runtime_identity;
    Ok(settings)
}

async fn refresh_runtime_system_prompt(
    config: &RuntimeConfig,
    conversation: &mut Vec<ConversationEntry>,
    overrides: &CompatRunOverrides,
    discovered_tool_scope: &DiscoveredToolScope,
) -> Result<()> {
    let settings = load_query_runtime_prompt_settings(config)?;
    claude_runtime_prompt::refresh_runtime_system_prompt(
        config,
        conversation,
        overrides,
        &settings,
        discovered_tool_scope,
    )
    .await
}

fn config_prompt_runtime_overrides(config: &RuntimeConfig) -> CompatRunOverrides {
    CompatRunOverrides {
        system_prompt: config.system_prompt.clone(),
        append_system_prompt: config.append_system_prompt.clone(),
        ..CompatRunOverrides::default()
    }
}

fn merge_prompt_runtime_overrides(
    config: &RuntimeConfig,
    mut overrides: CompatRunOverrides,
) -> CompatRunOverrides {
    if overrides.system_prompt.is_none() {
        overrides.system_prompt = config.system_prompt.clone();
    }
    if overrides.append_system_prompt.is_none() {
        overrides.append_system_prompt = config.append_system_prompt.clone();
    }
    overrides
}

impl CompatObserver {
    async fn mark_tool_started_if_new(&self, tool_call_id: &str) -> bool {
        if tool_call_id.is_empty() {
            return false;
        }
        self.shared
            .streamed_tool_calls
            .lock()
            .await
            .insert(tool_call_id.to_owned())
    }

    fn should_persist(&self) -> bool {
        self.execution.persist_transcript
    }
}

#[async_trait]
impl QueryObserver for CompatObserver {
    async fn on_event(&self, event: QueryObserverEvent) -> Result<()> {
        let session_id = { self.shared.config.lock().await.session_id };
        match event {
            QueryObserverEvent::ContextBudgetEvaluated { turn, context, .. } => {
                if let Some(event_sink) = self.event_sink.as_ref() {
                    event_sink(PromptStreamEvent::ContextUsage {
                        estimated_tokens: context.estimated_tokens,
                        max_input_tokens: context.max_input_tokens,
                        threshold_tokens: context.threshold_tokens,
                        ratio: context.usage_ratio,
                    });
                    if context.needs_compaction {
                        event_sink(PromptStreamEvent::ContextOverflow {
                            estimated_tokens: context.estimated_tokens,
                            max_input_tokens: context.max_input_tokens,
                            threshold_tokens: context.threshold_tokens,
                            ratio: context.usage_ratio,
                        });
                    }
                }
                if context.needs_compaction && self.should_persist() {
                    self.store.append_named_event(
                        session_id,
                        "context_overflow",
                        serde_json::json!({
                            "turn": turn,
                            "estimated_tokens": context.estimated_tokens,
                            "max_input_tokens": context.max_input_tokens,
                            "threshold_tokens": context.threshold_tokens,
                            "usage_ratio": context.usage_ratio,
                        }),
                    )?;
                }
            }
            QueryObserverEvent::ContextCompactionApplied {
                turn,
                before_messages,
                after_messages,
                compacted_conversation,
                max_input_tokens,
                threshold_tokens,
                usage_ratio_before,
                usage_ratio_after,
                estimated_tokens_before,
                estimated_tokens_after,
            } => {
                let entries_removed = before_messages.saturating_sub(after_messages);
                let mut discovered_before_compaction = self.shared.discovered_tool_scope.snapshot();
                {
                    let conversation = self.shared.conversation.lock().await;
                    discovered_before_compaction.extend(
                        claude_tools::extract_discovered_tool_names(
                            &conversation,
                            &std::collections::BTreeSet::new(),
                        ),
                    );
                }

                if self.should_persist() {
                    self.store.append_named_event(
                        session_id,
                        "context_compacted",
                        serde_json::json!({
                            "turn": turn,
                            "entries_removed": entries_removed,
                            "usage_ratio_before": usage_ratio_before,
                            "usage_ratio_after": usage_ratio_after,
                            "estimated_tokens_before": estimated_tokens_before,
                            "estimated_tokens_after": estimated_tokens_after,
                            "max_input_tokens": max_input_tokens,
                            "threshold_tokens": threshold_tokens,
                        }),
                    )?;
                    let mut boundary = claude_transcript::CompactBoundary::new(
                        claude_transcript::CompactTrigger::Auto,
                        estimated_tokens_before,
                    );
                    boundary.messages_summarized = Some(entries_removed);
                    boundary.user_context = Some("query_engine_auto_compact".to_owned());
                    if !discovered_before_compaction.is_empty() {
                        boundary.pre_compact_discovered_tools =
                            discovered_before_compaction.iter().cloned().collect();
                    }
                    self.store.append_transcript_entry(
                        &claude_transcript::TranscriptEntry::compact_boundary_now(
                            session_id, boundary,
                        ),
                    )?;
                    clear_runtime_system_prompt_state(session_id);
                    for entry in &compacted_conversation {
                        self.store.append_conversation_entry(session_id, entry)?;
                    }
                }
                self.shared
                    .discovered_tool_scope
                    .replace(discovered_before_compaction);
                {
                    let mut conversation = self.shared.conversation.lock().await;
                    *conversation = compacted_conversation.clone();
                }
                if let Some(event_sink) = self.event_sink.as_ref() {
                    event_sink(PromptStreamEvent::ContextCompacted {
                        entries_removed,
                        usage_ratio: usage_ratio_after,
                    });
                    event_sink(PromptStreamEvent::ContextUsage {
                        estimated_tokens: estimated_tokens_after,
                        max_input_tokens,
                        threshold_tokens,
                        ratio: usage_ratio_after,
                    });
                }
            }
            QueryObserverEvent::StreamingTextDelta { delta, .. } => {
                if self.include_partial_messages
                    && !delta.is_empty()
                    && let Some(event_sink) = self.event_sink.as_ref()
                {
                    event_sink(PromptStreamEvent::MessageDelta { delta });
                }
            }
            QueryObserverEvent::StreamingToolCallStarted {
                tool_call_id,
                tool_name,
                ..
            } => {
                if !tool_name.is_empty()
                    && self.mark_tool_started_if_new(&tool_call_id).await
                    && let Some(event_sink) = self.event_sink.as_ref()
                {
                    event_sink(PromptStreamEvent::ToolStarted {
                        tool_call_id,
                        tool_name,
                    });
                }
            }
            QueryObserverEvent::StreamingToolCallDelta {
                tool_call_id,
                delta,
                ..
            } => {
                if !tool_call_id.is_empty()
                    && !delta.is_empty()
                    && let Some(event_sink) = self.event_sink.as_ref()
                {
                    event_sink(PromptStreamEvent::ToolProgress {
                        tool_call_id: Some(tool_call_id),
                        delta: Some(delta),
                        elapsed_time_seconds: None,
                    });
                }
            }
            QueryObserverEvent::StreamingUsageUpdated { turn, usage } => {
                let usage = UsagePayload {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_input_tokens: usage.cache_read_input_tokens,
                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                };
                {
                    let mut latest_usage = self.shared.latest_streaming_usage.lock().await;
                    *latest_usage = Some(usage.clone());
                }
                if self.should_persist() {
                    self.store.append_named_event(
                        session_id,
                        "streaming_usage",
                        serde_json::json!({
                            "turn": turn,
                            "usage": {
                                "input_tokens": usage.input_tokens,
                                "output_tokens": usage.output_tokens,
                            },
                        }),
                    )?;
                }
            }
            QueryObserverEvent::AssistantMessageCommitted {
                message,
                stop_reason,
                turn,
                usage,
                request_id,
            } => {
                {
                    let mut latest_request_id = self.shared.latest_request_id.lock().await;
                    *latest_request_id = request_id.clone();
                }
                let assistant_entry = assistant_entry_from_message(&message)?;
                if self.should_persist() {
                    self.store
                        .append_conversation_entry(session_id, &assistant_entry)?;
                    self.store.append_named_event(
                        session_id,
                        "assistant_turn",
                        serde_json::json!({
                            "turn": turn,
                            "stop_reason": stop_reason,
                            "usage": {
                                "input_tokens": usage.input_tokens,
                                "output_tokens": usage.output_tokens,
                            },
                            "request_id": request_id,
                            "tool_calls": assistant_entry.tool_calls.len(),
                            "text_preview": truncate_preview(&assistant_entry.text, 160),
                        }),
                    )?;
                }
                {
                    let mut conversation = self.shared.conversation.lock().await;
                    conversation.push(assistant_entry.clone());
                }
                if let Some(event_sink) = self.event_sink.as_ref()
                    && !assistant_entry.text.trim().is_empty()
                {
                    event_sink(PromptStreamEvent::MessageCommitted {
                        text: assistant_entry.text.clone(),
                    });
                }
                if self.should_persist() {
                    if assistant_entry.tool_calls.is_empty() {
                        self.store.clear_resume_state(session_id)?;
                    } else {
                        let pending_tool_calls = assistant_entry
                            .tool_calls
                            .iter()
                            .map(|tool_call| PendingToolCall {
                                id: tool_call.id.clone(),
                                name: tool_call.name.clone(),
                                input: tool_call.input.clone(),
                            })
                            .collect::<Vec<_>>();
                        self.store.save_resume_state(
                            session_id,
                            &ResumeState::from_pending_calls(pending_tool_calls),
                        )?;
                    }
                }
            }
            QueryObserverEvent::ToolCallStarted { tool_call, .. } => {
                if !tool_call.name.is_empty()
                    && self.mark_tool_started_if_new(&tool_call.id).await
                    && let Some(event_sink) = self.event_sink.as_ref()
                {
                    event_sink(PromptStreamEvent::ToolStarted {
                        tool_call_id: tool_call.id,
                        tool_name: tool_call.name,
                    });
                }
            }
            QueryObserverEvent::ToolResultCommitted {
                tool_call, result, ..
            } => {
                if let Some(event_sink) = self.event_sink.as_ref() {
                    event_sink(PromptStreamEvent::ToolFinished {
                        tool_call_id: tool_call.id,
                        tool_name: tool_call.name,
                        is_error: result.is_error,
                        summary: Some(truncate_preview(&result.content, 160)),
                    });
                }
            }
            QueryObserverEvent::CheckpointCleared { checkpoint }
                if checkpoint.kind == QueryCheckpointKind::ToolBatch =>
            {
                if self.should_persist() {
                    self.store.clear_resume_state(session_id)?;
                }
            }
            QueryObserverEvent::BudgetEvaluated { .. }
            | QueryObserverEvent::BudgetExceeded { .. }
            | QueryObserverEvent::CheckpointCreated { .. }
            | QueryObserverEvent::MessagesAppended { .. }
            | QueryObserverEvent::QueryFailed { .. }
            | QueryObserverEvent::QueryStarted { .. }
            | QueryObserverEvent::StreamingThinkingDelta { .. }
            | QueryObserverEvent::QueryResult { .. }
            | QueryObserverEvent::TokenBudgetContinuation { .. }
            | QueryObserverEvent::ReactiveCompactApplied { .. }
            | QueryObserverEvent::ToolUseSummary { .. }
            | QueryObserverEvent::Progress { .. }
            | QueryObserverEvent::Attachment { .. }
            | QueryObserverEvent::ApiRetry { .. }
            | QueryObserverEvent::StopHookBlocking { .. }
            | QueryObserverEvent::StopHookPrevented { .. }
            | QueryObserverEvent::MaxTokensEscalate { .. }
            | QueryObserverEvent::MaxTokensRecovery { .. }
            | QueryObserverEvent::ModelFallbackTriggered { .. }
            | QueryObserverEvent::CollapseDrainRetry { .. }
            | QueryObserverEvent::ReactiveCompactRetry { .. }
            | QueryObserverEvent::MaxTokensRecoveryExhausted { .. }
            | QueryObserverEvent::ImageErrorRecovery { .. }
            | QueryObserverEvent::MediaSizeErrorRecovery { .. }
            | QueryObserverEvent::ContextCollapseRecovery { .. } => {}
            QueryObserverEvent::QueryFinished { .. } => {}
            QueryObserverEvent::CheckpointCleared { .. } => {}
        }
        Ok(())
    }
}

struct CompatToolRunner {
    store: Arc<SessionStore>,
    discovery: RuntimeHookDiscovery,
    shared: Arc<CompatSharedState>,
    broker: Arc<dyn PermissionBroker>,
    allowed_tools: Option<HashSet<String>>,
    sub_agent_completion: Arc<dyn claude_core::SubAgentCompletion>,
    execution: CompatExecutionOptions,
}

#[async_trait]
impl ToolRunner for CompatToolRunner {
    async fn run_tool(
        &self,
        tool_call: &ToolCall,
        _context: &ProcessUserInputContext,
    ) -> Result<ToolRunResult> {
        let current_config = self.shared.config.lock().await.clone();
        let original_tool_spec = runtime_provider_tool_spec(&tool_call.name)
            .await
            .ok_or_else(|| anyhow!("unknown tool {}", tool_call.name))?;

        let (prepared, pre_messages) = {
            let mut conversation = self.shared.conversation.lock().await;
            let mut hook_state = self.shared.hook_state.lock().await;
            let before_messages = conversation.len();
            let prepared = apply_pre_tool_use_hooks_with_options(
                &self.discovery,
                &current_config,
                self.store.as_ref(),
                &mut conversation,
                &mut hook_state,
                tool_call,
                self.execution.hook_options,
            )
            .await?;
            (
                prepared,
                conversation[before_messages..]
                    .iter()
                    .cloned()
                    .map(Message::from)
                    .collect::<Vec<_>>(),
            )
        };

        let effective_tool_call = prepared.call;
        let effective_tool_spec = if effective_tool_call.name == original_tool_spec.name {
            original_tool_spec
        } else {
            runtime_provider_tool_spec(&effective_tool_call.name)
                .await
                .ok_or_else(|| anyhow!("unknown tool {}", effective_tool_call.name))?
        };
        let audit_count_before = self.broker.audit_records().len();
        let raw_result = if let Some(blocked_reason) = &prepared.blocked_reason {
            ToolResult {
                content: blocked_reason.clone(),
                is_error: true,
                content_blocks: Vec::new(),
                follow_up_user_blocks: Vec::new(),
            }
        } else if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(effective_tool_call.name.as_str()))
        {
            ToolResult {
                content: format!(
                    "Tool `{}` is not allowed for this agent run.",
                    effective_tool_call.name
                ),
                is_error: true,
                content_blocks: Vec::new(),
                follow_up_user_blocks: Vec::new(),
            }
        } else {
            let fork_snapshot = {
                let conversation = self.shared.conversation.lock().await;
                fork_snapshot_from_conversation(&conversation)
            };
            let fork_snapshot_provider: Arc<claude_tools::RuntimeForkSnapshotProvider> =
                Arc::new(move || fork_snapshot.clone());
            let tool_context = ToolExecutionContext {
                sub_agent: Some(build_remote_code_sub_agent_runtime(
                    &current_config,
                    self.sub_agent_completion.clone(),
                    self.shared.read_file_state.clone(),
                )),
                read_file_state: self.shared.read_file_state.clone(),
                ..ToolExecutionContext::from_runtime_config(&current_config)
            };
            match with_runtime_fork_snapshot_provider(fork_snapshot_provider, async {
                execute_tool_call(&effective_tool_call, &tool_context, self.broker.as_ref()).await
            })
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    tracing::warn!(
                        "tool execution error for {}: {error}",
                        effective_tool_call.name
                    );
                    if self.execution.persist_transcript {
                        self.store.append_named_event(
                            current_config.session_id,
                            "tool_error",
                            serde_json::json!({
                                "tool_name": effective_tool_call.name,
                                "tool_use_id": effective_tool_call.id,
                                "error": format!("{error:#}"),
                            }),
                        )?;
                    }
                    ToolResult {
                        content: format!("Tool execution error: {error}"),
                        is_error: true,
                        content_blocks: Vec::new(),
                        follow_up_user_blocks: Vec::new(),
                    }
                }
            }
        };

        for audit in self
            .broker
            .audit_records()
            .into_iter()
            .skip(audit_count_before)
        {
            if self.execution.persist_transcript {
                self.store.append_named_event(
                    current_config.session_id,
                    "permission_decision",
                    serde_json::to_value(&audit)?,
                )?;
            }
        }

        let is_permission_denied =
            raw_result.is_error && is_permission_denied_message(&raw_result.content);
        let permission_denial =
            (is_permission_denied || prepared.blocked_reason.is_some()).then(|| {
                serde_json::json!({
                    "tool_name": effective_tool_call.name,
                    "tool_use_id": effective_tool_call.id,
                    "tool_input": effective_tool_call.input.clone(),
                    "message": raw_result.content.clone(),
                })
            });

        let tool_results_dir = Some(
            self.execution
                .persist_tool_results_dir
                .clone()
                .unwrap_or_else(|| {
                    current_config
                        .paths
                        .sessions_dir
                        .join(current_config.session_id.to_string())
                        .join("tool-results")
                }),
        );
        let processed_result = process_tool_result_content(
            &raw_result.content,
            &raw_result.content_blocks,
            &effective_tool_call.id,
            &effective_tool_call.name,
            tool_results_dir.as_deref(),
            effective_tool_spec.tool_result_size_policy(),
        )?;
        let tool_preview = truncate_preview(&processed_result.content, 160);
        let model_name = current_config
            .provider
            .model
            .as_deref()
            .unwrap_or("unknown");
        let truncated_content =
            claude_provider::context::ContextWindowManager::for_model(model_name)
                .truncate_tool_output_default(&processed_result.content);
        let result = ToolResult {
            content: truncated_content.clone(),
            is_error: raw_result.is_error,
            content_blocks: processed_result.content_blocks.clone(),
            follow_up_user_blocks: raw_result.follow_up_user_blocks.clone(),
        };

        {
            let mut config = self.shared.config.lock().await;
            let mut tool_context = ToolExecutionContext {
                sub_agent: Some(build_remote_code_sub_agent_runtime(
                    &config,
                    self.sub_agent_completion.clone(),
                    self.shared.read_file_state.clone(),
                )),
                read_file_state: self.shared.read_file_state.clone(),
                ..ToolExecutionContext::from_runtime_config(&config)
            };
            if apply_worktree_tool_result_to_runtime(
                &effective_tool_call.name,
                &effective_tool_call.input,
                &result,
                &mut config,
                &mut tool_context,
            )? {
                if self.execution.persist_runtime_context {
                    crate::conversation::persist_session_context(self.store.as_ref(), &config)?;
                }
                sync_tool_context_from_runtime(&config, &mut tool_context);
            }
        }

        let mut post_messages = Vec::new();
        {
            let mut conversation = self.shared.conversation.lock().await;
            let mut tool_entry = ConversationEntry::tool(
                effective_tool_call.id.clone(),
                effective_tool_call.name.clone(),
                truncated_content,
                raw_result.is_error,
            );
            tool_entry.content_blocks = processed_result.content_blocks.clone();
            if self.execution.persist_transcript {
                self.store
                    .append_conversation_entry(current_config.session_id, &tool_entry)?;
                self.store.append_named_event(
                    current_config.session_id,
                    "tool_result",
                    serde_json::json!({
                        "tool_name": effective_tool_call.name,
                        "tool_use_id": effective_tool_call.id,
                        "is_error": tool_entry.is_error,
                        "content_preview": tool_preview,
                    }),
                )?;
            }
            conversation.push(tool_entry);
            if !raw_result.follow_up_user_blocks.is_empty() {
                let follow_up_entry = ConversationEntry::user_with_content_blocks(
                    raw_result.follow_up_user_blocks.clone(),
                );
                if self.execution.persist_transcript {
                    self.store
                        .append_conversation_entry(current_config.session_id, &follow_up_entry)?;
                }
                post_messages.push(Message::from(follow_up_entry.clone()));
                conversation.push(follow_up_entry);
            }
        }

        {
            let mut conversation = self.shared.conversation.lock().await;
            let mut hook_state = self.shared.hook_state.lock().await;
            let before_messages = conversation.len();
            apply_post_tool_hooks_with_options(
                &self.discovery,
                &current_config,
                self.store.as_ref(),
                &mut conversation,
                &mut hook_state,
                &effective_tool_call,
                &raw_result,
                self.execution.hook_options,
            )
            .await?;
            post_messages.extend(
                conversation[before_messages..]
                    .iter()
                    .cloned()
                    .map(Message::from),
            );
        }

        Ok(ToolRunResult {
            result,
            pre_messages,
            post_messages,
            permission_denial,
            output_tokens_consumed: None,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prompt_with_query_engine_compat(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    broker: Arc<dyn PermissionBroker>,
    event_sink: Option<PromptEventSink>,
    discovery: &RuntimeHookDiscovery,
    hook_state: &mut HookRunState,
    conversation: &mut Vec<ConversationEntry>,
    prompt: &str,
) -> Result<PromptRunOutcome> {
    let query_source = if matches!(config.input_format, claude_core::InputFormat::StreamJson)
        || matches!(config.output_format, claude_core::OutputFormat::StreamJson)
    {
        QuerySource::Sdk
    } else {
        QuerySource::ReplMainThread
    };
    run_prompt_with_query_engine_compat_overrides(
        config,
        store,
        backend,
        discovered_tool_scope,
        broker,
        event_sink,
        discovery,
        hook_state,
        conversation,
        prompt,
        config_prompt_runtime_overrides(config),
        CompatExecutionOptions {
            query_source,
            ..CompatExecutionOptions::default()
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prompt_with_query_engine_compat_overrides(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    broker: Arc<dyn PermissionBroker>,
    event_sink: Option<PromptEventSink>,
    discovery: &RuntimeHookDiscovery,
    hook_state: &mut HookRunState,
    conversation: &mut Vec<ConversationEntry>,
    prompt: &str,
    overrides: CompatRunOverrides,
    execution: CompatExecutionOptions,
) -> Result<PromptRunOutcome> {
    let readiness = validate_provider_config(&config.provider);
    if !readiness.ok {
        return Err(anyhow!(readiness.issues.join(" ")));
    }
    let overrides = merge_prompt_runtime_overrides(config, overrides);
    let tool_results_dir = execution
        .persist_tool_results_dir
        .clone()
        .unwrap_or_else(|| session_tool_results_dir(config));

    let started = Instant::now();
    if execution.persist_transcript {
        inject_plan_mode_runtime_messages(store, config.session_id, conversation)?;
        inject_runtime_delta_messages(config, store, broker.as_ref(), conversation).await?;
    }
    refresh_runtime_system_prompt(config, conversation, &overrides, &discovered_tool_scope).await?;
    let provider_conversation = if execution.persist_transcript {
        let prompt_settings = load_query_runtime_prompt_settings(config)?;
        conversation_with_runtime_user_context_with_settings(
            config,
            conversation,
            &overrides,
            &prompt_settings,
        )
        .await
    } else {
        let content_backend = ContentReplacementBackend::new_with_options(
            backend.clone(),
            Arc::new(SessionStore::open(config.paths.clone())?),
            config.session_id,
            tool_results_dir.clone(),
            provision_content_replacement_state(store, config.session_id, conversation)?,
            claude_tools::runtime_tool_result_persistence_skip_names(),
            false,
        );
        let mut prepared = content_backend.prepare_conversation(conversation).await?;
        if let Some(snapshot) = execution.fork_snapshot.as_ref() {
            apply_exact_system_prompt(&mut prepared, snapshot.system_prompt.as_deref());
            augment_conversation_with_explicit_user_context(&prepared, &snapshot.user_context)
        } else {
            prepared
        }
    };
    let existing_messages = provider_conversation
        .iter()
        .cloned()
        .map(Message::from)
        .collect::<Vec<_>>();
    if execution.persist_session {
        store.ensure_session(
            config.session_id,
            &config.cwd,
            &config.provider.name,
            config.provider.model.as_deref(),
            config.session_name.as_deref().or(Some(prompt)),
        )?;
        store.append_named_event(
            config.session_id,
            "prompt_started",
            serde_json::json!({
                "prompt": prompt,
                "provider": config.provider.name.clone(),
                "model": config.provider.model.clone(),
                "protocol": config.provider.protocol.as_str(),
            }),
        )?;
    }
    let reuse_pending_prompt =
        crate::conversation::has_unanswered_user_prompt(conversation, prompt);
    if !reuse_pending_prompt {
        let user_entry = ConversationEntry::user(prompt);
        if execution.persist_transcript {
            store.append_conversation_entry(config.session_id, &user_entry)?;
        }
        conversation.push(user_entry);
    }

    let compat_store = Arc::new(SessionStore::open(config.paths.clone())?);
    let read_file_state = execution
        .fork_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.read_file_state.clone())
        .unwrap_or_default();
    let shared = Arc::new(CompatSharedState {
        config: Mutex::new(config.clone()),
        conversation: Mutex::new(conversation.clone()),
        discovered_tool_scope: discovered_tool_scope.clone(),
        hook_state: Mutex::new(std::mem::take(hook_state)),
        streamed_tool_calls: Mutex::new(HashSet::new()),
        latest_streaming_usage: Mutex::new(None),
        latest_request_id: Mutex::new(None),
        read_file_state,
    });
    let observer = Arc::new(CompatObserver {
        store: compat_store.clone(),
        shared: shared.clone(),
        event_sink: event_sink.clone(),
        include_partial_messages: config.include_partial_messages,
        execution: execution.clone(),
    });
    let visible_tool_specs = provider_runtime_tool_specs_for_request(
        &config.provider,
        conversation,
        &discovered_tool_scope.snapshot(),
    )
    .await;
    let expanded_allowed_tools = effective_allowed_tool_names(&overrides, &visible_tool_specs);
    let tool_runner = Arc::new(CompatToolRunner {
        store: compat_store.clone(),
        discovery: discovery.clone(),
        shared: shared.clone(),
        broker: broker.clone(),
        allowed_tools: expanded_allowed_tools.clone(),
        sub_agent_completion: backend.sub_agent_completion(),
        execution: execution.clone(),
    });

    let model_name = config
        .provider
        .model
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    let backend: Arc<dyn ConversationBackend> = match overrides
        .critical_system_reminder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(reminder) => Arc::new(CriticalReminderBackend {
            inner: backend,
            reminder: reminder.to_owned(),
        }),
        None => backend,
    };
    let runtime_extensions = discover_runtime_extensions(config);
    let prompt_settings = load_query_runtime_prompt_settings(config)?;
    let mut process_context = ProcessUserInputContext::new(
        config.session_id.into(),
        config.permission_mode,
        &model_name,
    );
    process_context.effort = parse_effort(config.effort.as_deref());
    process_context.requested_effort = config.effort.clone();
    process_context.fast_mode = prompt_settings
        .runtime_identity
        .fast_mode_user_setting
        .unwrap_or(false)
        || prompt_settings
            .runtime_identity
            .fast_mode_per_session_opt_in;
    process_context.query_source = execution.query_source;
    process_context.agent_id = execution.agent_id.clone();
    process_context.discovered_skills = runtime_extensions
        .skills
        .into_iter()
        .collect::<HashSet<_>>();
    apply_runtime_hook_context(
        &mut process_context,
        config,
        &provider_conversation,
        execution.fork_snapshot.as_ref(),
    );

    let mut query_config = QueryEngineConfig::new(
        config.session_id.into(),
        &model_name,
        backend.clone(),
        tool_runner,
        rc_engine_events::EventStream::new(64),
    )
    .with_observer(observer);
    if let Some(schema) = config.structured_output_schema.clone() {
        query_config = query_config.with_structured_output_schema(schema);
    }
    if let Some(fallback_model) = config.fallback_model.clone() {
        query_config = query_config.with_fallback_model(fallback_model);
    }
    let post_compact_store = compat_store.clone();
    let post_compact_config = config.clone();
    let post_compact_broker = broker.clone();
    let post_compact_session_id = config.session_id;
    query_config = query_config.with_post_compact_transform(Arc::new(move |conversation| {
        let post_compact_config = post_compact_config.clone();
        let post_compact_broker = post_compact_broker.clone();
        let post_compact_store = post_compact_store.clone();
        Box::pin(async move {
            augment_post_compact_conversation_for_runtime(
                &post_compact_config,
                post_compact_broker.as_ref(),
                post_compact_store.as_ref(),
                post_compact_session_id,
                conversation,
            )
            .await
        })
    }));
    let compact_runtime_config = config.clone();
    query_config =
        query_config.with_compact_conversation_handler(Arc::new(move |conversation, manager| {
            let compact_runtime_config = compact_runtime_config.clone();
            Box::pin(async move {
                try_session_memory_compaction(&compact_runtime_config, &conversation, &manager)
                    .await
                    .map(|compacted| (compacted, "session_memory".to_owned()))
            })
        }));
    if event_sink.is_some() || config.print_mode {
        query_config =
            query_config.with_provider_invocation_mode(ProviderInvocationMode::Streaming);
    }
    query_config = register_repl_runtime_hooks(
        query_config,
        ReplHookRuntimeResources {
            config: config.clone(),
            store: compat_store.clone(),
            backend: backend.clone(),
            discovered_tool_scope: discovered_tool_scope.clone(),
            event_sink: event_sink.clone(),
        },
    );
    query_config.max_turns = u32::try_from(config.max_turns).unwrap_or(u32::MAX);

    let mut engine = QueryEngine::new(query_config, existing_messages);
    let submitted_messages = if reuse_pending_prompt {
        Vec::new()
    } else {
        vec![Message::from(ConversationEntry::user(prompt))]
    };
    let (runtime_mcp_state_provider, runtime_mcp_observation_provider) =
        spawn_runtime_mcp_providers(config);
    let runtime_agent_prompt_context_provider = spawn_runtime_agent_prompt_context_provider(
        config,
        broker.as_ref(),
        Some(tool_results_dir),
    );
    let result =
        with_runtime_agent_prompt_context_provider(runtime_agent_prompt_context_provider, async {
            with_runtime_mcp_observation_provider(runtime_mcp_observation_provider, async {
                with_runtime_mcp_state_provider(runtime_mcp_state_provider, async {
                    if let Some(allowed_tools) = expanded_allowed_tools.as_ref() {
                        with_tool_runtime_policy_overlay(
                            ToolRuntimePolicyOverlay {
                                allowed_tools: Some(allowed_tools.iter().cloned().collect()),
                                disallowed_tools: Vec::new(),
                            },
                            async {
                                engine
                                    .submit_message(submitted_messages, process_context)
                                    .await
                            },
                        )
                        .await
                    } else {
                        engine
                            .submit_message(submitted_messages, process_context)
                            .await
                    }
                })
                .await
            })
            .await
        })
        .await;

    *conversation = legacy_conversation_for_result(&engine, result.as_ref().err());
    {
        let mut shared_hook_state = shared.hook_state.lock().await;
        *hook_state = std::mem::take(&mut *shared_hook_state);
    }

    let latest_request_id = shared.latest_request_id.lock().await.clone();
    let usage = effective_usage(
        UsagePayload {
            input_tokens: engine.state().usage.input_tokens,
            output_tokens: engine.state().usage.output_tokens,
            cache_read_input_tokens: engine.state().usage.cache_read_input_tokens,
            cache_creation_input_tokens: engine.state().usage.cache_creation_input_tokens,
        },
        shared.latest_streaming_usage.lock().await.clone(),
    );
    let total_tool_calls = conversation
        .iter()
        .map(|entry| entry.tool_calls.len())
        .sum::<usize>();
    let permission_denials = match &result {
        Ok(query_result) => query_result.permission_denials.clone(),
        Err(_) => engine.state().permission_denials.clone(),
    };
    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = started.elapsed().as_millis() as u64;
    let model_usage = serde_json::json!({
        "provider": config.provider.name.clone(),
        "model": config.provider.model.clone(),
        "protocol": config.provider.protocol.as_str(),
        "turns": engine.state().turn,
        "tool_calls": total_tool_calls,
        "cache_read_input_tokens": engine.state().usage.cache_read_input_tokens,
        "cache_creation_input_tokens": engine.state().usage.cache_creation_input_tokens,
        "request_id": latest_request_id,
    });

    match result {
        Ok(query_result) => {
            let outcome = PromptRunOutcome {
                text: query_result.final_text.unwrap_or_default(),
                duration_ms,
                duration_api_ms: duration_ms,
                num_turns: query_result.turns,
                stop_reason: query_result.stop_reason.clone(),
                total_cost_usd: 0.0,
                usage,
                model_usage,
                permission_denials,
            };
            if execution.persist_transcript {
                store.append_named_event(
                    config.session_id,
                    "result",
                    serde_json::json!({
                        "is_error": false,
                        "stop_reason": outcome.stop_reason.clone(),
                        "usage": {
                            "input_tokens": outcome.usage.input_tokens,
                            "output_tokens": outcome.usage.output_tokens,
                        },
                        "duration_ms": duration_ms,
                        "num_turns": outcome.num_turns,
                        "total_cost_usd": outcome.total_cost_usd,
                        "model_usage": outcome.model_usage.clone(),
                        "permission_denials": outcome.permission_denials.clone(),
                        "request_id": outcome.model_usage.get("request_id").cloned().unwrap_or(serde_json::Value::Null),
                    }),
                )?;
            }
            Ok(outcome)
        }
        Err(claude_query_engine::EngineError::Stopped(reason))
            if reason == format!("turn budget exceeded ({})", config.max_turns) =>
        {
            let error = anyhow!(
                "Maximum turn budget reached ({}) without a final assistant reply.",
                config.max_turns
            );
            if execution.persist_transcript {
                store.append_named_event(
                    config.session_id,
                    "result",
                    serde_json::json!({
                        "is_error": true,
                        "stop_reason": "max_turns",
                        "usage": {
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        },
                        "duration_ms": duration_ms,
                        "num_turns": engine.state().turn,
                        "total_cost_usd": 0.0,
                        "model_usage": model_usage.clone(),
                        "permission_denials": permission_denials.clone(),
                        "request_id": model_usage.get("request_id").cloned().unwrap_or(serde_json::Value::Null),
                        "error": error.to_string(),
                    }),
                )?;
            }
            Err(error)
        }
        Err(error) => {
            if execution.persist_transcript {
                store.append_named_event(
                    config.session_id,
                    "result",
                    serde_json::json!({
                        "is_error": true,
                        "stop_reason": "error",
                        "usage": {
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        },
                        "duration_ms": duration_ms,
                        "num_turns": engine.state().turn,
                        "total_cost_usd": 0.0,
                        "model_usage": model_usage,
                        "permission_denials": permission_denials,
                        "request_id": latest_request_id,
                        "error": error.to_string(),
                    }),
                )?;
            }
            Err(error.into())
        }
    }
}

fn parse_effort(effort: Option<&str>) -> EffortLevel {
    match effort.unwrap_or_default().to_ascii_lowercase().as_str() {
        "low" => EffortLevel::Low,
        "high" => EffortLevel::High,
        _ => EffortLevel::Medium,
    }
}

fn effective_usage(
    usage: UsagePayload,
    latest_streaming_usage: Option<UsagePayload>,
) -> UsagePayload {
    if usage.input_tokens == 0 && usage.output_tokens == 0 {
        latest_streaming_usage.unwrap_or(usage)
    } else {
        usage
    }
}

fn is_permission_denied_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("permission denied")
        || (lowered.contains("permission") && lowered.contains("denied"))
}

fn assistant_entry_from_message(message: &Message) -> Result<ConversationEntry> {
    let Message::Assistant(_) = message else {
        return Err(anyhow!(
            "expected assistant message, got {}",
            message_kind(message)
        ));
    };
    let mut entry = message
        .as_conversation_entry()
        .ok_or_else(|| anyhow!("assistant message could not be converted to conversation entry"))?;
    normalize_exit_plan_mode_tool_calls(&mut entry.tool_calls);
    Ok(entry)
}

fn legacy_conversation_for_result(
    engine: &QueryEngine,
    error: Option<&claude_query_engine::EngineError>,
) -> Vec<ConversationEntry> {
    let mut conversation = engine.state().legacy_conversation();
    if let Some(claude_query_engine::EngineError::Stopped(reason)) = error
        && conversation
            .last()
            .is_some_and(|entry| entry.role == ConversationRole::System && entry.text == *reason)
    {
        conversation.pop();
    }
    conversation
}

fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::Progress(_) => "progress",
        Message::System(_) => "system",
        Message::Attachment(_) => "attachment",
        Message::HookResult(_) => "hook_result",
        Message::ToolUseSummary(_) => "tool_use_summary",
        Message::Tombstone(_) => "tombstone",
        Message::GroupedToolUse(_) => "grouped_tool_use",
        Message::CollapsedReadSearch(_) => "collapsed_read_search",
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_no_persist_forked_query(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    broker: Arc<dyn PermissionBroker>,
    discovery: &RuntimeHookDiscovery,
    hook_state: &mut HookRunState,
    fork_snapshot: ForkCacheSafeParams,
    prompt: &str,
    mut overrides: CompatRunOverrides,
    query_source: QuerySource,
    max_turns: Option<u32>,
    tool_results_dir: PathBuf,
) -> Result<ForkedPromptRunOutcome> {
    let mut child_config = config.clone();
    if let Some(max_turns) = max_turns {
        child_config.max_turns = usize::try_from(max_turns).unwrap_or(usize::MAX);
    }

    let mut conversation = fork_snapshot
        .fork_context_messages
        .iter()
        .filter_map(Message::as_conversation_entry)
        .collect::<Vec<_>>();

    if fork_snapshot.system_prompt.is_some() {
        overrides.system_prompt = None;
        overrides.append_system_prompt = None;
        overrides.agent_system_prompt = None;
        overrides.override_system_prompt = fork_snapshot.system_prompt.clone();
    }

    let outcome = run_prompt_with_query_engine_compat_overrides(
        &child_config,
        store,
        backend,
        discovered_tool_scope,
        broker,
        None,
        discovery,
        hook_state,
        &mut conversation,
        prompt,
        overrides,
        CompatExecutionOptions {
            persist_session: false,
            persist_transcript: false,
            persist_runtime_context: false,
            persist_tool_results_dir: Some(tool_results_dir),
            hook_options: HookExecutionOptions::ephemeral(),
            query_source,
            agent_id: None,
            fork_snapshot: Some(fork_snapshot),
        },
    )
    .await?;

    Ok(ForkedPromptRunOutcome {
        messages: conversation,
        usage: outcome.usage,
        cache_read_input_tokens: outcome
            .model_usage
            .get("cache_read_input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        cache_creation_input_tokens: outcome
            .model_usage
            .get("cache_creation_input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        num_turns: outcome.num_turns,
        duration_ms: outcome.duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use anyhow::Result;
    use base64::Engine;
    use claude_config::{ProviderOverrides, RuntimeConfig, RuntimeOverrides, load_runtime_config};
    use claude_context::{RuntimeIdentityContext, RuntimeUserType};
    use claude_core::{
        ConversationEntry, ConversationRole, InputFormat, Message, OutputFormat, PermissionMode,
        ProviderProtocol, ProviderResponse, SubAgentCompletion, ToolCall, UsageSummary,
    };
    use claude_mcp::{
        McpCapabilityMatrix, McpServerConfig, McpServerInspection, McpTransportConfig,
    };
    use claude_permissions::{
        LayeredPermissionBroker, PermissionBroker, PermissionDecision, PermissionRequest,
        StaticPermissionBroker,
    };
    use claude_provider::{
        ConversationBackend, DiscoveredToolScope, ProviderCompatBackend, StreamingCallbacks,
    };
    use claude_query_engine::{QueryObserver, QueryObserverEvent, QuerySource};
    use claude_session::{SessionStore, plan_state::PlanModeState};
    use claude_tools::mcp_catalog::clear_runtime_mcp_catalog_cache;
    use claude_tools::{
        RuntimeAgentPromptContext, RuntimeMcpServerPolicyEntry, ToolRuntimePolicy,
        configure_tool_runtime_policy, current_tool_runtime_policy,
        with_runtime_agent_prompt_context_provider, with_runtime_mcp_observation_provider,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::process::Command as ProcessCommand;
    use std::sync::OnceLock;
    use tempfile::{TempDir, tempdir};
    use tokio::sync::Mutex as AsyncMutex;

    use super::{
        AGENT_LISTING_DELTA_MARKER, CompatExecutionOptions, CompatObserver, CompatRunOverrides,
        CompatSharedState, DEFERRED_TOOLS_DELTA_MARKER, ForkCacheSafeParams,
        MCP_INSTRUCTIONS_DELTA_MARKER, RuntimeAgentListingDeltaMarker,
        RuntimeDeferredToolsDeltaMarker, RuntimeMcpInstructionsDeltaMarker, announced_agent_types,
        announced_deferred_tool_names, announced_mcp_instruction_names,
        augment_post_compact_conversation_for_runtime, build_agent_listing_delta_entry,
        build_mcp_instructions_delta_entry, build_runtime_identity_context,
        build_runtime_identity_context_with_entrypoint, refresh_runtime_system_prompt,
        run_no_persist_forked_query, run_prompt_with_query_engine_compat,
        run_prompt_with_query_engine_compat_overrides, runtime_delta_entry,
    };
    use crate::conversation::{PromptEventSink, PromptStreamEvent, initialize_conversation};
    use crate::hooks::{HookRunState, RuntimeHookDiscovery};
    use claude_system_prompt::cache::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;

    static RUNTIME_POLICY_TEST_MUTEX: OnceLock<AsyncMutex<()>> = OnceLock::new();

    struct CoordinatorModeTestGuard {
        _guard: tokio::sync::MutexGuard<'static, ()>,
    }

    impl CoordinatorModeTestGuard {
        async fn enter(mode: claude_agents::coordinator::CoordinatorMode) -> Self {
            let guard = RUNTIME_POLICY_TEST_MUTEX
                .get_or_init(|| AsyncMutex::new(()))
                .lock()
                .await;
            claude_agents::coordinator::reset_coordinator_override();
            let _ = claude_agents::coordinator::match_session_mode(Some(mode));
            Self { _guard: guard }
        }
    }

    impl Drop for CoordinatorModeTestGuard {
        fn drop(&mut self) {
            claude_agents::coordinator::reset_coordinator_override();
        }
    }

    fn mock_config_and_store() -> (TempDir, RuntimeConfig, SessionStore) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                base_url: Some("mock://provider".to_owned()),
                api_key: Some("mock".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config");
        let store = SessionStore::open(config.paths.clone()).expect("store");
        (tempdir, config, store)
    }

    fn fake_runtime_mcp_policy_entry(
        name: &str,
        config_path: std::path::PathBuf,
    ) -> RuntimeMcpServerPolicyEntry {
        RuntimeMcpServerPolicyEntry {
            origin_kind: "cwd".to_owned(),
            origin_name: "workspace".to_owned(),
            config_path,
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
                tool_policy: Default::default(),
            },
        }
    }

    fn live_mcp_observation_provider(
        observation: claude_tools::mcp_runtime::RuntimeMcpObservation,
    ) -> Arc<claude_tools::RuntimeMcpObservationProvider> {
        Arc::new(move || observation.clone())
    }

    fn mock_broker(config: &RuntimeConfig) -> Arc<dyn PermissionBroker> {
        Arc::new(LayeredPermissionBroker::new(
            StaticPermissionBroker::from_mode(config.permission_mode),
            Vec::new(),
        ))
    }

    fn allow_all_broker() -> Arc<dyn PermissionBroker> {
        Arc::new(StaticPermissionBroker::new(true))
    }

    fn run_large_stack_tokio_test<Fut>(test_body: impl FnOnce() -> Fut + Send + 'static)
    where
        Fut: Future<Output = ()> + 'static,
    {
        std::thread::Builder::new()
            .name("query-engine-compat-test".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(test_body());
            })
            .expect("spawn query engine compat test")
            .join()
            .expect("query engine compat test panicked");
    }

    #[tokio::test]
    async fn post_compact_runtime_augmentation_restores_plan_attachment_and_marker() {
        let (_tempdir, config, store) = mock_config_and_store();
        let broker = mock_broker(&config);
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("plan"),
            )
            .expect("session");
        let plan_dir = config.paths.profile_dir.join("plans");
        fs::create_dir_all(&plan_dir).expect("plan dir");
        let plan_path = plan_dir.join("plan-test.md");
        fs::write(&plan_path, "# Plan\n\n- keep this").expect("plan file");
        store
            .save_plan_mode_state(
                config.session_id,
                &PlanModeState {
                    current_permission_mode: PermissionMode::Plan,
                    plan_file_path: Some(plan_path.clone()),
                    ..PlanModeState::default()
                },
            )
            .expect("plan mode state");

        let augmented = augment_post_compact_conversation_for_runtime(
            &config,
            broker.as_ref(),
            &store,
            config.session_id,
            vec![
                ConversationEntry::system("sys"),
                ConversationEntry::user("tail"),
            ],
        )
        .await;
        let augmented = augment_post_compact_conversation_for_runtime(
            &config,
            broker.as_ref(),
            &store,
            config.session_id,
            augmented,
        )
        .await;

        let plan_filename = plan_path.display().to_string();
        let plan_attachments = augmented
            .iter()
            .flat_map(|entry| entry.attachments.iter())
            .filter(|attachment| attachment.filename.as_deref() == Some(plan_filename.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(plan_attachments.len(), 1);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&plan_attachments[0].data)
            .expect("base64 plan attachment");
        assert_eq!(
            String::from_utf8(decoded).expect("utf8 plan attachment"),
            "# Plan\n\n- keep this"
        );
        assert!(augmented.iter().any(|entry| {
            entry.role == ConversationRole::User
                && entry.text.contains("## Plan Workflow")
                && entry.text.contains("### Phase 5: Call ExitPlanMode")
        }));
    }

    #[test]
    fn deferred_tool_markers_reconstruct_announced_pool() {
        let add = runtime_delta_entry(
            DEFERRED_TOOLS_DELTA_MARKER,
            &RuntimeDeferredToolsDeltaMarker {
                added_names: vec!["alpha".to_owned(), "beta".to_owned()],
                added_lines: vec!["alpha".to_owned(), "beta".to_owned()],
                removed_names: Vec::new(),
            },
            "added".to_owned(),
        )
        .expect("delta entry");
        let remove = runtime_delta_entry(
            DEFERRED_TOOLS_DELTA_MARKER,
            &RuntimeDeferredToolsDeltaMarker {
                added_names: Vec::new(),
                added_lines: Vec::new(),
                removed_names: vec!["alpha".to_owned()],
            },
            "removed".to_owned(),
        )
        .expect("delta entry");

        let announced = announced_deferred_tool_names(&[add, remove]);
        assert_eq!(
            announced.into_iter().collect::<Vec<_>>(),
            vec!["beta".to_owned()]
        );
    }

    #[test]
    fn mcp_instruction_markers_reconstruct_announced_pool() {
        let add = runtime_delta_entry(
            MCP_INSTRUCTIONS_DELTA_MARKER,
            &RuntimeMcpInstructionsDeltaMarker {
                added_names: vec!["context7".to_owned(), "memory".to_owned()],
                added_blocks: vec![
                    "## context7\nUse docs".to_owned(),
                    "## memory\nUse memory".to_owned(),
                ],
                removed_names: Vec::new(),
            },
            "added".to_owned(),
        )
        .expect("delta entry");
        let remove = runtime_delta_entry(
            MCP_INSTRUCTIONS_DELTA_MARKER,
            &RuntimeMcpInstructionsDeltaMarker {
                added_names: Vec::new(),
                added_blocks: Vec::new(),
                removed_names: vec!["memory".to_owned()],
            },
            "removed".to_owned(),
        )
        .expect("delta entry");

        let announced = announced_mcp_instruction_names(&[add, remove]);
        assert_eq!(
            announced.into_iter().collect::<Vec<_>>(),
            vec!["context7".to_owned()]
        );
    }

    #[test]
    fn agent_listing_markers_reconstruct_announced_pool() {
        let add = runtime_delta_entry(
            AGENT_LISTING_DELTA_MARKER,
            &RuntimeAgentListingDeltaMarker {
                added_types: vec!["alpha".to_owned(), "beta".to_owned()],
                added_lines: Vec::new(),
                removed_types: Vec::new(),
                is_initial: true,
                show_concurrency_note: true,
            },
            "added".to_owned(),
        )
        .expect("delta entry");
        let remove = runtime_delta_entry(
            AGENT_LISTING_DELTA_MARKER,
            &RuntimeAgentListingDeltaMarker {
                added_types: Vec::new(),
                added_lines: Vec::new(),
                removed_types: vec!["alpha".to_owned()],
                is_initial: false,
                show_concurrency_note: false,
            },
            "removed".to_owned(),
        )
        .expect("delta entry");

        let announced = announced_agent_types(&[add, remove]);
        assert_eq!(
            announced.into_iter().collect::<Vec<_>>(),
            vec!["beta".to_owned()]
        );
    }

    #[tokio::test]
    async fn build_agent_listing_delta_entry_announces_initial_visible_agents() {
        let (_tempdir, mut config, _store) = mock_config_and_store();
        fs::create_dir_all(config.cwd.join(".claude").join("agents")).expect("agents dir");
        fs::write(
            config.cwd.join(".claude").join("agents").join("docs-agent.md"),
            "---\nname: docs-agent\ndescription: Use docs\nrequiredMcpServers: [context7]\n---\nYou answer docs questions.\n",
        )
        .expect("write docs agent");
        config.disallowed_tools.clear();

        let broker = mock_broker(&config);
        let entry = build_agent_listing_delta_entry(&config, broker.as_ref(), &[])
            .await
            .expect("delta")
            .expect("should create initial agent listing");

        assert_eq!(entry.role, ConversationRole::User);
        let text = entry.content_blocks[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .expect("system reminder text");
        assert!(text.contains("Available agent types for the Agent tool:"));
        assert!(text.contains("- general-purpose:"));
        assert!(!text.contains("- docs-agent:"));

        let marker = entry.history_text.expect("marker history text");
        assert!(marker.contains("\"addedLines\""));
        assert!(marker.contains("\"isInitial\":true"));
        assert!(marker.contains("\"showConcurrencyNote\":true"));
    }

    #[test]
    fn build_runtime_identity_context_defaults_to_cli_and_enables_fork_for_interactive_runs() {
        let (_tempdir, config, _store) = mock_config_and_store();
        let identity = build_runtime_identity_context_with_entrypoint(&config, None);

        assert_eq!(identity.entrypoint.as_deref(), Some("cli"));
        assert!(!identity.is_non_interactive);
        assert!(identity.features.explore_plan_agents_enabled);
        assert!(!identity.features.verification_agent_enabled);
        assert!(identity.features.code_guide_enabled);
        assert!(identity.features.is_fork_subagent_enabled);
    }

    #[test]
    fn build_runtime_identity_context_defaults_to_sdk_cli_for_noninteractive_runs() {
        let (_tempdir, mut config, _store) = mock_config_and_store();
        config.print_mode = true;

        let identity = build_runtime_identity_context_with_entrypoint(&config, None);

        assert_eq!(identity.entrypoint.as_deref(), Some("sdk-cli"));
        assert!(identity.is_non_interactive);
        assert!(!identity.features.is_fork_subagent_enabled);
    }

    #[test]
    fn build_runtime_identity_context_hydrates_persisted_oauth_identity_when_auth_source_missing() {
        let (_tempdir, mut config, _store) = mock_config_and_store();
        config.auth_source = None;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        fs::write(
            config.paths.profile_dir.join(".credentials.json"),
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "oauth-token",
                    "refreshToken": "refresh-token",
                    "expiresAt": 1234,
                    "scopes": ["user:profile", "user:inference"],
                    "subscriptionType": "team",
                    "rateLimitTier": "high"
                }
            })
            .to_string(),
        )
        .expect("write credentials");
        fs::write(
            config.paths.profile_dir.join(".config.json"),
            serde_json::json!({
                "oauthAccount": {
                    "accountUuid": "acct-1",
                    "emailAddress": "dev@example.com",
                    "organizationUuid": "org-1",
                    "displayName": "Dev User",
                    "hasExtraUsageEnabled": false,
                    "billingType": "stripe_subscription_contracted",
                    "accountCreatedAt": "2025-01-01T00:00:00Z",
                    "subscriptionCreatedAt": "2025-02-01T00:00:00Z"
                }
            })
            .to_string(),
        )
        .expect("write config");

        let identity = build_runtime_identity_context(&config);

        assert_eq!(identity.auth_source.as_deref(), Some("claude.ai"));
        assert_eq!(identity.user_type, RuntimeUserType::External);
        assert_eq!(identity.account_uuid.as_deref(), Some("acct-1"));
        assert_eq!(identity.organization_uuid.as_deref(), Some("org-1"));
        assert_eq!(identity.email.as_deref(), Some("dev@example.com"));
        assert_eq!(
            identity.subscription.subscription_type.as_deref(),
            Some("team")
        );
        assert_eq!(
            identity.subscription.rate_limit_tier.as_deref(),
            Some("high")
        );
        assert_eq!(
            identity.subscription.billing_type.as_deref(),
            Some("stripe_subscription_contracted")
        );
        assert_eq!(identity.subscription.has_extra_usage_enabled, Some(false));
        assert_eq!(
            identity.subscription.display_name.as_deref(),
            Some("Dev User")
        );
    }

    #[test]
    fn build_runtime_identity_context_prefers_explicit_auth_source_over_persisted_oauth_identity() {
        let (_tempdir, mut config, _store) = mock_config_and_store();
        config.auth_source = Some("env:ANTHROPIC_API_KEY".to_owned());
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        fs::write(
            config.paths.profile_dir.join(".credentials.json"),
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "oauth-token",
                    "refreshToken": "refresh-token",
                    "expiresAt": 1234,
                    "subscriptionType": "team",
                    "rateLimitTier": "high"
                }
            })
            .to_string(),
        )
        .expect("write credentials");
        fs::write(
            config.paths.profile_dir.join(".config.json"),
            serde_json::json!({
                "oauthAccount": {
                    "accountUuid": "acct-1",
                    "emailAddress": "dev@example.com",
                    "organizationUuid": "org-1"
                }
            })
            .to_string(),
        )
        .expect("write config");

        let identity = build_runtime_identity_context(&config);

        assert_eq!(
            identity.auth_source.as_deref(),
            Some("env:ANTHROPIC_API_KEY")
        );
        assert_eq!(identity.account_uuid, None);
        assert_eq!(identity.organization_uuid, None);
        assert_eq!(identity.subscription.subscription_type, None);
    }

    #[tokio::test]
    async fn build_agent_listing_delta_entry_respects_allowed_agent_types_from_runtime_context() {
        let (_tempdir, mut config, _store) = mock_config_and_store();
        let agents_dir = config.cwd.join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).expect("agents dir");
        fs::write(
            agents_dir.join("alpha-agent.md"),
            "---\nname: alpha-agent\ndescription: Use alpha\n---\nYou answer alpha questions.\n",
        )
        .expect("write alpha agent");
        fs::write(
            agents_dir.join("beta-agent.md"),
            "---\nname: beta-agent\ndescription: Use beta\n---\nYou answer beta questions.\n",
        )
        .expect("write beta agent");
        config.disallowed_tools.clear();

        let broker = mock_broker(&config);
        let context = RuntimeAgentPromptContext {
            user_agents_dir: None,
            project_agents_dir: Some(agents_dir),
            additional_working_directories: Vec::new(),
            allowed_agent_types: Some(vec!["alpha-agent".to_owned()]),
            denied_agent_types: Vec::new(),
            is_coordinator: false,
            is_non_interactive: false,
            list_via_attachment: true,
            runtime_identity: RuntimeIdentityContext::from_legacy_env(),
            scratchpad_dir: None,
            session_memory_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            auto_memory_dir: None,
            auto_memory_read_dir: None,
            team_memory_read_dir: None,
            project_temp_dir: None,
            preview_launch_config_path: None,
            teams_dir: None,
            agent_memory_dirs: Vec::new(),
        };

        let entry =
            with_runtime_agent_prompt_context_provider(Arc::new(move || context.clone()), async {
                build_agent_listing_delta_entry(&config, broker.as_ref(), &[])
                    .await
                    .expect("delta")
                    .expect("should create filtered agent listing")
            })
            .await;

        let text = entry.content_blocks[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .expect("system reminder text");
        assert!(text.contains("- alpha-agent:"));
        assert!(!text.contains("- beta-agent:"));
        assert!(!text.contains("- general-purpose:"));
    }

    #[tokio::test]
    async fn post_compact_runtime_augmentation_reannounces_deferred_tools_delta() {
        let (_tempdir, config, store) = mock_config_and_store();
        let broker = mock_broker(&config);

        let augmented = augment_post_compact_conversation_for_runtime(
            &config,
            broker.as_ref(),
            &store,
            config.session_id,
            vec![
                ConversationEntry::system("sys"),
                ConversationEntry::user("tail"),
            ],
        )
        .await;

        let announced = announced_deferred_tool_names(&augmented);
        // Deferred tools delta uses provider wire names (e.g. "TodoWrite"),
        // not internal tool names (e.g. "todo_write").
        assert!(announced.contains("TodoWrite"));
        let marker = augmented
            .iter()
            .find_map(|entry| {
                entry.history_text.as_deref().and_then(|text| {
                    text.starts_with(DEFERRED_TOOLS_DELTA_MARKER)
                        .then_some(text)
                })
            })
            .expect("deferred tools delta marker");
        assert!(marker.contains("\"addedLines\""));
    }

    #[tokio::test]
    async fn build_mcp_instructions_delta_entry_announces_connected_server_instructions() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX
            .get_or_init(|| AsyncMutex::new(()))
            .lock()
            .await;
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping MCP instructions delta test because Python is unavailable.");
            return;
        };

        let (_tempdir, config, _store) = mock_config_and_store();
        let script = config.cwd.join("mock_mcp_round_trip.py");
        fs::write(&script, mock_mcp_round_trip_server_script()).expect("mock mcp script");
        prefix_args.push(script.display().to_string());

        let original_policy = current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "cwd".to_owned(),
                origin_name: "workspace".to_owned(),
                config_path: config.cwd.join(".mcp.json"),
                server: McpServerConfig {
                    name: "mock".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: python,
                        args: prefix_args,
                        cwd: Some(config.cwd.clone()),
                        env: BTreeMap::new(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: Some(3),
                    request_timeout_secs: Some(3),
                    metadata: BTreeMap::new(),
                    oauth: None,
                    tool_policy: Default::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        clear_runtime_mcp_catalog_cache().await;

        let run_result = build_mcp_instructions_delta_entry(&[]).await;

        clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        let entry = run_result
            .expect("delta")
            .expect("should create MCP instructions delta");
        let text = entry.content_blocks[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .expect("system reminder text");
        assert!(text.contains("# MCP Server Instructions"));
        assert!(text.contains("## mock"));
        assert!(text.contains("Use mock MCP tools when they are available."));
        let marker = entry.history_text.as_deref().expect("history text");
        assert!(marker.contains("\"addedNames\":[\"mock\"]"));
        assert!(marker.contains("\"addedBlocks\""));
    }

    #[test]
    fn runtime_mcp_session_observation_reuses_existing_session_snapshot() {
        let (_tempdir, config, _store) = mock_config_and_store();
        let config_path = config.cwd.join(".mcp.json");
        fs::write(
            &config_path,
            r#"{"mcpServers":{"context7":{"command":"definitely-missing-mcp-command"}}}"#,
        )
        .expect("write mcp config");

        super::clear_runtime_mcp_session_observation(config.session_id);

        let first = super::runtime_mcp_session_observation(&config);
        {
            let mut snapshot = first.lock().expect("snapshot lock");
            let server = snapshot
                .servers
                .first_mut()
                .expect("discovered server should exist");
            server.status = claude_ui_bridge::UiRuntimeMcpServerStatus::Connected;
            server.inspection = Some(McpServerInspection {
                server_name: "context7".to_owned(),
                protocol_version: "2025-03-26".to_owned(),
                server_info: None,
                capabilities: json!({}),
                instructions: Some("Use Context7 for docs.".to_owned()),
                tools: Vec::new(),
                prompts: Vec::new(),
                resources: Vec::new(),
            });
        }

        let second = super::runtime_mcp_session_observation(&config);
        assert!(Arc::ptr_eq(&first, &second));
        let second_snapshot = second.lock().expect("snapshot lock").clone();
        assert_eq!(
            second_snapshot.servers[0].status,
            claude_ui_bridge::UiRuntimeMcpServerStatus::Connected
        );
        assert_eq!(
            second_snapshot.servers[0]
                .inspection
                .as_ref()
                .and_then(|inspection| inspection.instructions.as_deref()),
            Some("Use Context7 for docs.")
        );

        super::clear_runtime_mcp_session_observation(config.session_id);
    }

    #[tokio::test]
    async fn build_mcp_instructions_delta_entry_uses_task_local_mcp_snapshot() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX
            .get_or_init(|| AsyncMutex::new(()))
            .lock()
            .await;
        let (_tempdir, config, _store) = mock_config_and_store();
        let config_path = config.cwd.join(".mcp.json");
        let original_policy = current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![fake_runtime_mcp_policy_entry(
                "context7",
                config_path.clone(),
            )],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        clear_runtime_mcp_catalog_cache().await;

        let observation = claude_tools::mcp_runtime::RuntimeMcpObservation {
            servers: vec![claude_tools::mcp_runtime::RuntimeMcpServerObservation {
                entry: claude_tools::mcp_runtime::RuntimeMcpServerEntry {
                    origin_kind: "cwd",
                    origin_name: "workspace".to_owned(),
                    config_path,
                    server: fake_runtime_mcp_policy_entry("context7", config.cwd.join(".mcp.json"))
                        .server,
                },
                status: claude_ui_bridge::UiRuntimeMcpServerStatus::Connected,
                inspection: Some(McpServerInspection {
                    server_name: "context7".to_owned(),
                    protocol_version: "2025-03-26".to_owned(),
                    server_info: None,
                    capabilities: json!({}),
                    instructions: Some("Use Context7 for API and library docs.".to_owned()),
                    tools: Vec::new(),
                    prompts: Vec::new(),
                    resources: Vec::new(),
                }),
                error: None,
            }],
            warnings: Vec::new(),
        };

        let run_result = with_runtime_mcp_observation_provider(
            live_mcp_observation_provider(observation),
            async { build_mcp_instructions_delta_entry(&[]).await },
        )
        .await;

        clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        let entry = run_result
            .expect("delta")
            .expect("should create MCP instructions delta");
        let text = entry.content_blocks[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .expect("system reminder text");
        assert!(text.contains("## context7"));
        assert!(text.contains("Use Context7 for API and library docs."));
    }

    #[tokio::test]
    async fn runtime_mcp_list_changed_updates_session_snapshot_and_catalog_invalidation() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX
            .get_or_init(|| AsyncMutex::new(()))
            .lock()
            .await;
        let (_tempdir, config, _store) = mock_config_and_store();
        let config_path = config.cwd.join(".mcp.json");
        fs::write(
            &config_path,
            r#"{"mcpServers":{"context7":{"command":"definitely-missing-mcp-command","startup_timeout_secs":1,"request_timeout_secs":1}}}"#,
        )
        .expect("write mcp config");
        let original_policy = current_tool_runtime_policy();
        super::clear_runtime_mcp_session_observation(config.session_id);
        let observation = super::runtime_mcp_session_observation(&config);
        let discovered_entry = observation
            .lock()
            .expect("snapshot")
            .servers
            .first()
            .expect("server")
            .entry
            .clone();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: discovered_entry.origin_kind.to_owned(),
                origin_name: discovered_entry.origin_name.clone(),
                config_path: discovered_entry.config_path.clone(),
                server: discovered_entry.server.clone(),
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        clear_runtime_mcp_catalog_cache().await;

        {
            let mut snapshot = observation.lock().expect("snapshot");
            let server = snapshot.servers.first_mut().expect("server");
            server.status = claude_ui_bridge::UiRuntimeMcpServerStatus::Connected;
            server.inspection = Some(McpServerInspection {
                server_name: "context7".to_owned(),
                protocol_version: "2025-03-26".to_owned(),
                server_info: None,
                capabilities: json!({"tools": {"listChanged": true}}),
                instructions: Some("stale instructions".to_owned()),
                tools: Vec::new(),
                prompts: Vec::new(),
                resources: Vec::new(),
            });
        }

        let initial_observation = observation.lock().expect("snapshot").clone();
        let initial_entry = with_runtime_mcp_observation_provider(
            live_mcp_observation_provider(initial_observation),
            async { build_mcp_instructions_delta_entry(&[]).await },
        )
        .await
        .expect("initial delta")
        .expect("initial entry");
        assert!(
            initial_entry
                .content_blocks
                .iter()
                .any(|block| block.to_string().contains("stale instructions"))
        );

        super::handle_runtime_mcp_session_list_changed(
            &config,
            &observation,
            "context7",
            claude_mcp::McpListChangedSurface::Resources,
        )
        .await;
        {
            let snapshot = observation.lock().expect("snapshot");
            assert_eq!(
                snapshot.servers[0].status,
                claude_ui_bridge::UiRuntimeMcpServerStatus::Connected,
                "resources/list_changed should not invalidate the prompt/tool snapshot"
            );
            assert_eq!(
                snapshot.servers[0]
                    .inspection
                    .as_ref()
                    .and_then(|inspection| inspection.instructions.as_deref()),
                Some("stale instructions")
            );
        }

        super::handle_runtime_mcp_session_list_changed(
            &config,
            &observation,
            "context7",
            claude_mcp::McpListChangedSurface::Prompts,
        )
        .await;
        {
            let snapshot = observation.lock().expect("snapshot");
            assert_eq!(
                snapshot.servers[0].status,
                claude_ui_bridge::UiRuntimeMcpServerStatus::Failed
            );
            assert!(snapshot.servers[0].inspection.is_none());
            assert!(snapshot.servers[0].error.is_some());
        }

        clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");
        super::clear_runtime_mcp_session_observation(config.session_id);
    }

    #[tokio::test]
    async fn compat_observer_persists_compacted_suffix_after_boundary() {
        let (_tempdir, config, store) = mock_config_and_store();
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("compact"),
            )
            .expect("session");
        store
            .append_conversation_entry(config.session_id, &ConversationEntry::user("old"))
            .expect("old entry");

        let observer = CompatObserver {
            store: Arc::new(SessionStore::open(config.paths.clone()).expect("observer store")),
            shared: Arc::new(CompatSharedState {
                config: tokio::sync::Mutex::new(config.clone()),
                conversation: tokio::sync::Mutex::new(vec![ConversationEntry::tool(
                    "tool-1",
                    "tool_search",
                    r#"{"query":"web","results":[{"name":"web_fetch"}]}"#,
                    false,
                )]),
                discovered_tool_scope: DiscoveredToolScope::default(),
                hook_state: tokio::sync::Mutex::new(HookRunState::default()),
                streamed_tool_calls: tokio::sync::Mutex::new(std::collections::HashSet::new()),
                latest_streaming_usage: tokio::sync::Mutex::new(None),
                latest_request_id: tokio::sync::Mutex::new(None),
                read_file_state: claude_tools::FileStateCache::new(),
            }),
            event_sink: None,
            include_partial_messages: false,
            execution: CompatExecutionOptions::default(),
        };

        observer
            .on_event(QueryObserverEvent::ContextCompactionApplied {
                turn: 1,
                before_messages: 5,
                after_messages: 2,
                compacted_conversation: vec![
                    ConversationEntry::system("summary"),
                    ConversationEntry::user("tail"),
                ],
                max_input_tokens: 100,
                threshold_tokens: 80,
                usage_ratio_before: 0.9,
                usage_ratio_after: 0.2,
                estimated_tokens_before: 90,
                estimated_tokens_after: 20,
            })
            .await
            .expect("compaction event");

        let loaded = store
            .load_conversation(config.session_id)
            .expect("load compacted conversation");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, "summary");
        assert_eq!(loaded[1].text, "tail");
        let transcript = store
            .load_transcript_v2(config.session_id)
            .expect("typed transcript");
        let boundary = transcript
            .iter()
            .find_map(|entry| entry.as_compact_boundary())
            .expect("compact boundary");
        assert_eq!(
            boundary.pre_compact_discovered_tools,
            vec!["web_fetch".to_owned()]
        );
        let carried = store
            .load_carried_discovered_tool_names(config.session_id)
            .expect("carried discovered tools");
        assert!(carried.contains("web_fetch"));
    }

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

    fn mock_mcp_round_trip_server_script() -> &'static str {
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
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.1.0"},
                "instructions": "Use mock MCP tools when they are available."
            }
        }), flush=True)
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "tools": [{
                    "name": "resolve-library-id",
                    "description": "Resolve a library identifier",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "libraryName": {"type": "string"}
                        },
                        "required": ["libraryName"]
                    }
                }]
            }
        }), flush=True)
    elif method == "tools/call":
        library_name = message["params"]["arguments"]["libraryName"]
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "content": [{"type": "text", "text": f"resolved: {library_name}"}],
                "structuredContent": {"library": library_name},
                "isError": False
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

    #[derive(Default)]
    struct DenyCommandBroker;

    #[async_trait::async_trait]
    impl PermissionBroker for DenyCommandBroker {
        fn mode(&self) -> Option<PermissionMode> {
            Some(PermissionMode::Default)
        }

        async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
            if request.tool_name == "bash_command" {
                PermissionDecision::deny("Permission denied for bash_command.")
            } else {
                PermissionDecision::allow()
            }
        }
    }

    fn mock_provider_backend(config: &RuntimeConfig) -> Arc<dyn ConversationBackend> {
        Arc::new(ProviderCompatBackend::new(
            Arc::new(claude_provider::ProviderClient::new().expect("provider client")),
            &config.provider,
        ))
    }

    struct DummySubAgentCompletion;

    #[async_trait::async_trait]
    impl SubAgentCompletion for DummySubAgentCompletion {
        async fn complete(&self, _conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            Ok(ProviderResponse {
                text: "subagent".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }
    }

    #[derive(Default)]
    struct RecordingBackend {
        conversations: Arc<StdMutex<Vec<Vec<ConversationEntry>>>>,
    }

    #[async_trait::async_trait]
    impl ConversationBackend for RecordingBackend {
        async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            self.conversations
                .lock()
                .expect("recording lock")
                .push(conversation.to_vec());
            Ok(ProviderResponse {
                text: "recorded".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }

        async fn complete_streaming(
            &self,
            conversation: &[ConversationEntry],
            _callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete(conversation).await
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    #[derive(Default)]
    struct ToolRoundTripRecordingBackend {
        complete_calls: AtomicUsize,
        conversations: Arc<StdMutex<Vec<Vec<ConversationEntry>>>>,
    }

    #[async_trait::async_trait]
    impl ConversationBackend for ToolRoundTripRecordingBackend {
        async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            self.conversations
                .lock()
                .expect("recording lock")
                .push(conversation.to_vec());
            let call_index = self.complete_calls.fetch_add(1, Ordering::SeqCst);
            if call_index == 0 {
                return Ok(ProviderResponse {
                    text: String::new(),
                    history_text: None,
                    thinking: None,
                    content_blocks: Vec::new(),
                    tool_calls: vec![ToolCall {
                        id: "tool-1".to_owned(),
                        name: "glob".to_owned(),
                        input: json!({"pattern": "*.rs"}),
                    }],
                    request_id: None,
                    usage: UsageSummary::default(),
                    stop_reason: "tool_use".to_owned(),
                    research: None,
                });
            }

            Ok(ProviderResponse {
                text: "done after tool".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }

        async fn complete_streaming(
            &self,
            conversation: &[ConversationEntry],
            _callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete(conversation).await
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    #[derive(Default)]
    struct RecordingStreamingBackend {
        complete_calls: AtomicUsize,
        complete_streaming_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ConversationBackend for RecordingStreamingBackend {
        async fn complete(&self, _conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            self.complete_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse {
                text: "buffered".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }

        async fn complete_streaming(
            &self,
            _conversation: &[ConversationEntry],
            callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete_streaming_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(callbacks) = callbacks
                && let Some(on_text_delta) = callbacks.on_text_delta.as_ref()
            {
                on_text_delta("streaming-backend");
            }
            Ok(ProviderResponse {
                text: "streaming-backend".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    struct FailingUsageStreamingBackend;

    #[async_trait::async_trait]
    impl ConversationBackend for FailingUsageStreamingBackend {
        async fn complete(&self, _conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            Err(anyhow::anyhow!("streaming backend failed"))
        }

        async fn complete_streaming(
            &self,
            _conversation: &[ConversationEntry],
            callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            if let Some(callbacks) = callbacks
                && let Some(on_usage) = callbacks.on_usage.as_ref()
            {
                on_usage(claude_provider::streaming::StreamingUsageUpdate {
                    input_tokens: 7,
                    output_tokens: 4,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                });
            }
            Err(anyhow::anyhow!("streaming backend failed"))
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    struct PermissionDeniedCommandBackend;

    #[async_trait::async_trait]
    impl ConversationBackend for PermissionDeniedCommandBackend {
        async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            let has_tool_result_after_latest_user = conversation
                .iter()
                .rev()
                .take_while(|entry| entry.role != ConversationRole::User)
                .any(|entry| entry.role == ConversationRole::Tool);
            Ok(ProviderResponse {
                text: if has_tool_result_after_latest_user {
                    "mock provider observed the denial".to_owned()
                } else {
                    "attempting command".to_owned()
                },
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: if has_tool_result_after_latest_user {
                    Vec::new()
                } else {
                    vec![ToolCall {
                        id: "tool-denied-1".to_owned(),
                        name: "bash_command".to_owned(),
                        input: serde_json::json!({"command": "echo hi"}),
                    }]
                },
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            })
        }

        async fn complete_streaming(
            &self,
            conversation: &[ConversationEntry],
            _callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete(conversation).await
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    struct DynamicMcpRoundTripBackend;

    #[async_trait::async_trait]
    impl ConversationBackend for DynamicMcpRoundTripBackend {
        async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            let has_tool_result_after_latest_user = conversation
                .iter()
                .rev()
                .take_while(|entry| entry.role != ConversationRole::User)
                .find(|entry| entry.role == ConversationRole::Tool)
                .cloned();

            if let Some(tool_entry) = has_tool_result_after_latest_user {
                return Ok(ProviderResponse {
                    text: format!("dynamic MCP tool result: {}", tool_entry.text),
                    history_text: None,
                    thinking: None,
                    content_blocks: Vec::new(),
                    tool_calls: Vec::new(),
                    request_id: None,
                    usage: UsageSummary::default(),
                    stop_reason: "end_turn".to_owned(),
                    research: None,
                });
            }

            Ok(ProviderResponse {
                text: "calling dynamic MCP tool".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: "mcp-dynamic-1".to_owned(),
                    name: "mcp__mock__resolve-library-id".to_owned(),
                    input: json!({"libraryName": "tokio"}),
                }],
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "tool_use".to_owned(),
                research: None,
            })
        }

        async fn complete_streaming(
            &self,
            conversation: &[ConversationEntry],
            _callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete(conversation).await
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummySubAgentCompletion)
        }
    }

    #[tokio::test]
    async fn compat_run_persists_basic_mock_result() {
        let (_tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("hello compat")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");

        let outcome = run_prompt_with_query_engine_compat(
            &config,
            &store,
            mock_provider_backend(&config),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "hello compat",
        )
        .await
        .expect("compat run should succeed");

        assert!(outcome.text.contains("mock provider response"));
        let events = store.load_events(config.session_id).expect("events");
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "prompt_started")
        );
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "assistant_turn")
        );
        assert!(events.iter().any(|event| event.event_type == "result"));
        let assistant_turn = events
            .iter()
            .find(|event| event.event_type == "assistant_turn")
            .expect("assistant_turn event");
        assert_eq!(
            assistant_turn
                .payload
                .as_ref()
                .and_then(|payload| payload.get("request_id"))
                .and_then(serde_json::Value::as_str),
            Some("mock-request-id")
        );
        let result = events
            .iter()
            .find(|event| event.event_type == "result")
            .expect("result event");
        assert_eq!(
            result
                .payload
                .as_ref()
                .and_then(|payload| payload.get("request_id"))
                .and_then(serde_json::Value::as_str),
            Some("mock-request-id")
        );
        assert_eq!(
            outcome
                .model_usage
                .get("request_id")
                .and_then(serde_json::Value::as_str),
            Some("mock-request-id")
        );
        assert!(
            conversation
                .iter()
                .any(|entry| entry.role == ConversationRole::Assistant)
        );
    }

    #[tokio::test]
    async fn compat_run_reinjects_critical_system_reminder_before_agent_prompt() {
        let (_tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("verify this")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingBackend::default());

        run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "verify this",
            CompatRunOverrides {
                agent_system_prompt: Some("Verifier".to_owned()),
                critical_system_reminder: Some("CRITICAL CHECK".to_owned()),
                ..CompatRunOverrides::default()
            },
            CompatExecutionOptions::default(),
        )
        .await
        .expect("compat run");

        let calls = backend.conversations.lock().expect("recording lock");
        let first_call = calls.first().expect("provider call");
        let reminder_index = first_call
            .iter()
            .position(|entry| {
                entry.role == ConversationRole::User
                    && entry.text == "<system-reminder>\nCRITICAL CHECK\n</system-reminder>"
            })
            .expect("critical system reminder");
        let prompt_index = first_call
            .iter()
            .position(|entry| entry.role == ConversationRole::User && entry.text == "verify this")
            .expect("user prompt");
        assert!(reminder_index < prompt_index);
    }

    #[tokio::test]
    async fn compat_run_reinjects_critical_system_reminder_on_each_provider_turn() {
        let (_tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("use a tool")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(ToolRoundTripRecordingBackend::default());

        run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "use a tool",
            CompatRunOverrides {
                agent_system_prompt: Some("Verifier".to_owned()),
                critical_system_reminder: Some("CRITICAL CHECK".to_owned()),
                ..CompatRunOverrides::default()
            },
            CompatExecutionOptions::default(),
        )
        .await
        .expect("compat run");

        let calls = backend.conversations.lock().expect("recording lock");
        assert!(calls.len() >= 2, "expected a tool round-trip");
        for call in calls.iter().take(2) {
            let reminder_count = call
                .iter()
                .filter(|entry| {
                    entry.role == ConversationRole::User
                        && entry.text == "<system-reminder>\nCRITICAL CHECK\n</system-reminder>"
                })
                .count();
            assert_eq!(reminder_count, 1);
        }
    }

    #[tokio::test]
    async fn compat_run_omits_claude_md_and_git_status_for_child_overrides() {
        let (_tempdir, config, store) = mock_config_and_store();
        fs::write(config.cwd.join("CLAUDE.md"), "Follow project rules.").expect("claude md");
        fs::create_dir_all(config.cwd.join(".git")).expect("git marker");
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("explore this")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingBackend::default());

        run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "explore this",
            CompatRunOverrides {
                agent_system_prompt: Some("Explore child".to_owned()),
                omit_claude_md: true,
                omit_git_status: true,
                ..CompatRunOverrides::default()
            },
            CompatExecutionOptions::default(),
        )
        .await
        .expect("compat run");

        let calls = backend.conversations.lock().expect("recording lock");
        let first_call = calls.first().expect("provider call");
        let user_context = first_call
            .iter()
            .find(|entry| {
                entry.role == ConversationRole::User
                    && entry.text.contains(
                        "As you answer the user's questions, you can use the following context:",
                    )
            })
            .expect("runtime user context");
        assert!(user_context.text.contains("currentDate"));
        assert!(!user_context.text.contains("claudeMd"));
        let system_entry = first_call
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(!system_entry.text.contains("gitStatus:"));
    }

    #[tokio::test]
    async fn compat_run_includes_coordinator_worker_tools_context_in_user_reminder() {
        let _coordinator_mode = CoordinatorModeTestGuard::enter(
            claude_agents::coordinator::CoordinatorMode::Coordinator,
        )
        .await;

        let (_tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation = initialize_conversation(&store, &config, Some("coordinate this"))
            .expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingBackend::default());

        run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "coordinate this",
            CompatRunOverrides::default(),
            CompatExecutionOptions::default(),
        )
        .await
        .expect("compat run");

        let calls = backend.conversations.lock().expect("recording lock");
        let first_call = calls.first().expect("provider call");
        let user_context = first_call
            .iter()
            .find(|entry| {
                entry.role == ConversationRole::User
                    && entry.text.contains(
                        "As you answer the user's questions, you can use the following context:",
                    )
            })
            .expect("runtime user context");
        assert!(user_context.text.contains("workerToolsContext"));
        assert!(
            user_context
                .text
                .contains("Workers spawned via the Agent tool")
        );
    }

    #[test]
    fn compat_run_clears_resume_state_after_mock_list_files_prompt() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                rt.block_on(async {
                    let (_tempdir, config, store) = mock_config_and_store();
                    let discovery = RuntimeHookDiscovery::default();
                    let mut conversation =
                        initialize_conversation(&store, &config, Some("list files"))
                            .expect("conversation");
                    let mut hook_state =
                        HookRunState::load(&store, config.session_id).expect("hook state");

                    let outcome = run_prompt_with_query_engine_compat(
                        &config,
                        &store,
                        mock_provider_backend(&config),
                        DiscoveredToolScope::default(),
                        mock_broker(&config),
                        None,
                        &discovery,
                        &mut hook_state,
                        &mut conversation,
                        "list files",
                    )
                    .await
                    .expect("compat run should succeed");

                    assert!(!outcome.text.trim().is_empty());
                    assert!(
                        conversation
                            .iter()
                            .any(|entry| entry.role == ConversationRole::Assistant)
                    );
                    let resume_state = store
                        .load_resume_state(config.session_id)
                        .expect("resume state")
                        .expect("resume state row");
                    assert!(resume_state.pending_tool_calls.is_empty());
                    let transcript = store
                        .load_transcript(config.session_id)
                        .expect("load transcript");
                    assert!(
                        transcript
                            .conversation_entries()
                            .iter()
                            .any(|entry| entry.role == ConversationRole::Assistant),
                        "event types: {:?}",
                        transcript
                            .events()
                            .iter()
                            .map(|event| event.event_type.clone())
                            .collect::<Vec<_>>()
                    );
                });
            })
            .expect("thread spawn")
            .join()
            .expect("thread join");
    }

    #[tokio::test]
    async fn compat_run_ephemeral_execution_skips_session_and_transcript_persistence() {
        let (tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("ephemeral")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let before_events = store.load_events(config.session_id).expect("events before");
        let before_conversation = store
            .load_conversation(config.session_id)
            .expect("conversation before");

        let outcome = run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            Arc::new(RecordingBackend::default()),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "ephemeral",
            CompatRunOverrides::default(),
            CompatExecutionOptions {
                persist_session: false,
                persist_transcript: false,
                persist_runtime_context: false,
                persist_tool_results_dir: Some(tempdir.path().join("ephemeral-tool-results")),
                hook_options: crate::hooks::HookExecutionOptions::ephemeral(),
                query_source: QuerySource::User,
                agent_id: None,
                fork_snapshot: None,
            },
        )
        .await
        .expect("ephemeral compat run");

        assert_eq!(outcome.text, "recorded");
        assert!(
            conversation
                .iter()
                .any(|entry| entry.role == ConversationRole::Assistant)
        );

        let after_events = store.load_events(config.session_id).expect("events after");
        let after_conversation = store
            .load_conversation(config.session_id)
            .expect("conversation after");
        assert_eq!(after_events.len(), before_events.len());
        assert_eq!(after_conversation.len(), before_conversation.len());
    }

    #[tokio::test]
    async fn no_persist_forked_query_uses_snapshot_prompt_and_user_context() {
        let (tempdir, config, store) = mock_config_and_store();
        let discovery = RuntimeHookDiscovery::default();
        let _conversation =
            initialize_conversation(&store, &config, Some("fork parent")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingBackend::default());
        let fork_snapshot = ForkCacheSafeParams {
            fork_context_messages: vec![Message::from(ConversationEntry::user("parent context"))],
            system_prompt: Some("Fork system prompt".to_owned()),
            user_context: BTreeMap::from([("snapshotKey".to_owned(), "snapshotValue".to_owned())]),
            system_context: BTreeMap::from([("cwd".to_owned(), config.cwd.display().to_string())]),
            read_file_state: None,
        };

        let outcome = run_no_persist_forked_query(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            &discovery,
            &mut hook_state,
            fork_snapshot,
            "child task",
            CompatRunOverrides::default(),
            QuerySource::ExtractMemories,
            Some(2),
            tempdir.path().join("fork-tool-results"),
        )
        .await
        .expect("fork run");

        assert!(
            outcome
                .messages
                .iter()
                .any(|entry| entry.role == ConversationRole::Assistant && entry.text == "recorded")
        );
        let calls = backend.conversations.lock().expect("recording lock");
        let first_call = calls.first().expect("provider call");
        let system_entry = first_call
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(system_entry.text.contains("Fork system prompt"));
        let user_context = first_call
            .iter()
            .find(|entry| {
                entry.role == ConversationRole::User
                    && entry.text.contains(
                        "As you answer the user's questions, you can use the following context:",
                    )
            })
            .expect("runtime user context");
        assert!(user_context.text.contains("# snapshotKey\nsnapshotValue"));
        assert!(first_call.iter().any(|entry| {
            entry.role == ConversationRole::User && entry.text == "parent context"
        }));
        assert!(
            first_call.iter().any(|entry| {
                entry.role == ConversationRole::User && entry.text == "child task"
            })
        );
    }

    #[tokio::test]
    async fn refresh_runtime_system_prompt_preserves_structured_blocks() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        let mut conversation = initialize_conversation(&store, &config, Some("structured prompt"))
            .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                agent_system_prompt: Some("Follow child instructions".to_owned()),
                allowed_tools: None,
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(!system_entry.text.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
        assert!(system_entry.text.contains("Follow child instructions"));
        assert!(!system_entry.text.contains("You are an interactive agent"));
        assert_eq!(system_entry.content_blocks.len(), 1);
        assert!(
            system_entry.content_blocks[0]
                .get("cache_control")
                .is_some_and(|cache| cache.get("scope").is_none())
        );

        let dynamic_text = system_entry.content_blocks[0]["text"]
            .as_str()
            .expect("prompt block text");
        assert!(dynamic_text.contains("Follow child instructions"));
        assert!(!dynamic_text.contains("# Custom Agent Instructions"));
        assert!(!dynamic_text.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
    }

    #[tokio::test]
    async fn agent_system_prompt_keeps_runtime_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation =
            initialize_conversation(&store, &config, Some("agent prompt context"))
                .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                agent_system_prompt: Some("Specialized child agent".to_owned()),
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(system_entry.text.contains("Specialized child agent"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn custom_system_prompt_skips_runtime_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation = initialize_conversation(&store, &config, Some("custom prompt test"))
            .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                system_prompt: Some("Custom headless prompt".to_owned()),
                allowed_tools: None,
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(system_entry.text.contains("Custom headless prompt"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn append_system_prompt_keeps_runtime_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation = initialize_conversation(&store, &config, Some("append prompt test"))
            .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                append_system_prompt: Some("Append runtime prompt".to_owned()),
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(system_entry.text.contains("Append runtime prompt"));
        assert!(system_entry.text.contains("You are an interactive agent"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn override_system_prompt_replaces_runtime_prompt_and_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation =
            initialize_conversation(&store, &config, Some("override prompt test"))
                .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                system_prompt: Some("Custom prompt should lose".to_owned()),
                append_system_prompt: Some("Append prompt should lose".to_owned()),
                override_system_prompt: Some("Override runtime prompt".to_owned()),
                agent_system_prompt: Some("Agent prompt should lose".to_owned()),
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(system_entry.text.contains("Override runtime prompt"));
        assert!(!system_entry.text.contains("Custom prompt should lose"));
        assert!(!system_entry.text.contains("Append prompt should lose"));
        assert!(!system_entry.text.contains("Agent prompt should lose"));
        assert!(!system_entry.text.contains("You are an interactive agent"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn empty_custom_system_prompt_still_skips_default_prompt_and_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation =
            initialize_conversation(&store, &config, Some("empty custom prompt"))
                .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                system_prompt: Some(String::new()),
                allowed_tools: None,
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(
            system_entry.text.is_empty() || system_entry.text.trim().is_empty(),
            "system text: {:?}",
            system_entry.text
        );
        assert!(
            system_entry.content_blocks.is_empty()
                || system_entry.content_blocks.iter().all(|block| block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.trim().is_empty())),
            "content blocks: {:?}",
            system_entry.content_blocks
        );
        assert!(!system_entry.text.contains("You are an interactive agent"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn whitespace_custom_system_prompt_skips_default_prompt_and_system_context() {
        let _coordinator_mode =
            CoordinatorModeTestGuard::enter(claude_agents::coordinator::CoordinatorMode::Normal)
                .await;
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&config.cwd)
            .output()
            .expect("git init");
        let mut conversation =
            initialize_conversation(&store, &config, Some("blank custom prompt"))
                .expect("conversation");

        refresh_runtime_system_prompt(
            &config,
            &mut conversation,
            &CompatRunOverrides {
                system_prompt: Some("   ".to_owned()),
                allowed_tools: None,
                ..CompatRunOverrides::default()
            },
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("refresh runtime system prompt");

        let system_entry = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::System)
            .expect("system entry");
        assert!(
            system_entry.text.is_empty() || system_entry.text.trim().is_empty(),
            "system text: {:?}",
            system_entry.text
        );
        assert!(
            system_entry.content_blocks.is_empty()
                || system_entry.content_blocks.iter().all(|block| block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.trim().is_empty())),
            "content blocks: {:?}",
            system_entry.content_blocks
        );
        assert!(!system_entry.text.contains("You are an interactive agent"));
        assert!(
            !system_entry
                .text
                .contains("This is the git status at the start")
        );
    }

    #[tokio::test]
    async fn compat_run_accepts_dynamic_mcp_tools() {
        let _runtime_policy_guard = RUNTIME_POLICY_TEST_MUTEX
            .get_or_init(|| AsyncMutex::new(()))
            .lock()
            .await;
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping dynamic MCP compat test because Python is unavailable.");
            return;
        };

        let (_tempdir, config, store) = mock_config_and_store();
        let script = config.cwd.join("mock_mcp_round_trip.py");
        fs::write(&script, mock_mcp_round_trip_server_script()).expect("mock mcp script");
        prefix_args.push(script.display().to_string());

        let original_policy = current_tool_runtime_policy();
        configure_tool_runtime_policy(ToolRuntimePolicy {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_output_dir: None,
            tasks_dir: None,
            tool_results_dir: None,
            mcp_servers: vec![RuntimeMcpServerPolicyEntry {
                origin_kind: "cwd".to_owned(),
                origin_name: "workspace".to_owned(),
                config_path: config.cwd.join(".mcp.json"),
                server: McpServerConfig {
                    name: "mock".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: python,
                        args: prefix_args,
                        cwd: Some(config.cwd.clone()),
                        env: BTreeMap::new(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: Some(3),
                    request_timeout_secs: Some(3),
                    oauth: None,
                    metadata: BTreeMap::new(),
                    tool_policy: Default::default(),
                },
            }],
            shell_policy: Default::default(),
        })
        .expect("set runtime policy");
        clear_runtime_mcp_catalog_cache().await;

        let run_result = async {
            let discovery = RuntimeHookDiscovery::default();
            let mut conversation = initialize_conversation(&store, &config, Some("resolve tokio"))
                .expect("conversation");
            let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");

            run_prompt_with_query_engine_compat(
                &config,
                &store,
                Arc::new(DynamicMcpRoundTripBackend),
                DiscoveredToolScope::default(),
                allow_all_broker(),
                None,
                &discovery,
                &mut hook_state,
                &mut conversation,
                "resolve tokio",
            )
            .await
            .map(|outcome| (outcome, conversation))
        }
        .await;

        clear_runtime_mcp_catalog_cache().await;
        configure_tool_runtime_policy(original_policy).expect("restore runtime policy");

        let (outcome, conversation) = run_result.expect("compat run should succeed");
        assert!(
            outcome.text.contains("tokio"),
            "outcome text: {:?}; conversation: {:?}",
            outcome.text,
            conversation
        );
        assert!(
            outcome.text.contains("library"),
            "outcome text: {:?}; conversation: {:?}",
            outcome.text,
            conversation
        );
        assert!(conversation.iter().any(|entry| {
            entry.role == ConversationRole::Tool
                && entry.tool_call_id.as_deref() == Some("mcp-dynamic-1")
                && entry.name.as_deref() == Some("mcp__mock__resolve-library-id")
        }));
    }

    #[tokio::test]
    async fn compat_observer_translates_streaming_events_without_duplicate_tool_started() {
        let (_tempdir, config, _store) = mock_config_and_store();
        let captured = Arc::new(StdMutex::new(Vec::<PromptStreamEvent>::new()));
        let captured_sink = Arc::clone(&captured);
        let event_sink: PromptEventSink = Arc::new(move |event| {
            captured_sink
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });
        let observer = CompatObserver {
            store: Arc::new(SessionStore::open(config.paths.clone()).expect("store")),
            shared: Arc::new(CompatSharedState {
                config: tokio::sync::Mutex::new(config.clone()),
                conversation: tokio::sync::Mutex::new(Vec::new()),
                discovered_tool_scope: DiscoveredToolScope::default(),
                hook_state: tokio::sync::Mutex::new(HookRunState::default()),
                streamed_tool_calls: tokio::sync::Mutex::new(std::collections::HashSet::new()),
                latest_streaming_usage: tokio::sync::Mutex::new(None),
                latest_request_id: tokio::sync::Mutex::new(None),
                read_file_state: claude_tools::FileStateCache::new(),
            }),
            event_sink: Some(event_sink),
            include_partial_messages: true,
            execution: CompatExecutionOptions::default(),
        };

        observer
            .on_event(QueryObserverEvent::StreamingToolCallStarted {
                turn: 1,
                tool_call_id: "tool-1".to_owned(),
                tool_name: "bash_command".to_owned(),
            })
            .await
            .expect("streaming tool start");
        observer
            .on_event(QueryObserverEvent::ToolCallStarted {
                turn: 1,
                batch_size: 1,
                batch_index: 0,
                tool_call: ToolCall {
                    id: "tool-1".to_owned(),
                    name: "bash_command".to_owned(),
                    input: serde_json::json!({"command": "echo hi"}),
                },
            })
            .await
            .expect("buffered tool start");
        observer
            .on_event(QueryObserverEvent::StreamingToolCallDelta {
                turn: 1,
                tool_call_id: "tool-1".to_owned(),
                delta: "{\"command\":\"echo hi\"}".to_owned(),
            })
            .await
            .expect("streaming tool delta");
        observer
            .on_event(QueryObserverEvent::StreamingTextDelta {
                turn: 1,
                delta: "OK".to_owned(),
                accumulated_text: "OK".to_owned(),
            })
            .await
            .expect("streaming text delta");

        let events = captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    PromptStreamEvent::ToolStarted { tool_call_id, tool_name }
                        if tool_call_id == "tool-1" && tool_name == "bash_command"
                ))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            PromptStreamEvent::ToolProgress {
                tool_call_id: Some(tool_call_id),
                delta: Some(delta),
                elapsed_time_seconds: None,
            } if tool_call_id == "tool-1" && delta.contains("echo hi")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptStreamEvent::MessageDelta { delta } if delta == "OK"
        )));
    }

    #[tokio::test]
    async fn compat_run_reuses_caller_backend_for_streaming_event_sink_path() {
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.include_partial_messages = true;
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("streaming")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingStreamingBackend::default());
        let captured = Arc::new(StdMutex::new(Vec::<PromptStreamEvent>::new()));
        let captured_sink = Arc::clone(&captured);
        let event_sink: PromptEventSink = Arc::new(move |event| {
            captured_sink
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });

        let outcome = run_prompt_with_query_engine_compat(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            Some(event_sink),
            &discovery,
            &mut hook_state,
            &mut conversation,
            "streaming",
        )
        .await
        .expect("compat streaming run should succeed");

        assert_eq!(outcome.text, "streaming-backend");
        assert_eq!(backend.complete_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.complete_streaming_calls.load(Ordering::SeqCst), 1);
        let events = captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(events.iter().any(|event| matches!(
            event,
            PromptStreamEvent::MessageDelta { delta } if delta == "streaming-backend"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            PromptStreamEvent::MessageCommitted { text } if text == "streaming-backend"
        )));
    }

    #[tokio::test]
    async fn compat_run_uses_streaming_backend_for_print_mode_without_event_sink() {
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.print_mode = true;
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("print")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let backend = Arc::new(RecordingStreamingBackend::default());

        let outcome = run_prompt_with_query_engine_compat(
            &config,
            &store,
            backend.clone(),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            "print",
        )
        .await
        .expect("compat print-mode run should succeed");

        assert_eq!(outcome.text, "streaming-backend");
        assert_eq!(backend.complete_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.complete_streaming_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn compat_run_persists_latest_streaming_usage_on_error() {
        let (_tempdir, mut config, store) = mock_config_and_store();
        config.include_partial_messages = true;
        let discovery = RuntimeHookDiscovery::default();
        let mut conversation =
            initialize_conversation(&store, &config, Some("streaming")).expect("conversation");
        let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");
        let event_sink: PromptEventSink = Arc::new(|_| {});

        let error = run_prompt_with_query_engine_compat(
            &config,
            &store,
            Arc::new(FailingUsageStreamingBackend),
            DiscoveredToolScope::default(),
            mock_broker(&config),
            Some(event_sink),
            &discovery,
            &mut hook_state,
            &mut conversation,
            "streaming",
        )
        .await
        .expect_err("compat streaming run should fail");

        assert!(error.to_string().contains("streaming backend failed"));
        let transcript = store
            .load_transcript(config.session_id)
            .expect("load transcript");
        let result = transcript
            .latest_named_event_payload("result")
            .expect("result payload");
        assert_eq!(result["usage"]["input_tokens"], 7);
        assert_eq!(result["usage"]["output_tokens"], 4);
        let streaming_usage = transcript
            .latest_named_event_payload("streaming_usage")
            .expect("streaming usage payload");
        assert_eq!(streaming_usage["usage"]["input_tokens"], 7);
        assert_eq!(streaming_usage["usage"]["output_tokens"], 4);
    }

    #[test]
    fn compat_run_permission_denials_include_tool_input() {
        run_large_stack_tokio_test(|| async {
            let (_tempdir, config, store) = mock_config_and_store();
            let discovery = RuntimeHookDiscovery::default();
            let mut conversation = initialize_conversation(&store, &config, Some("run command"))
                .expect("conversation");
            let mut hook_state = HookRunState::load(&store, config.session_id).expect("hook state");

            let outcome = run_prompt_with_query_engine_compat(
                &config,
                &store,
                Arc::new(PermissionDeniedCommandBackend),
                DiscoveredToolScope::default(),
                Arc::new(DenyCommandBroker),
                None,
                &discovery,
                &mut hook_state,
                &mut conversation,
                "run command",
            )
            .await
            .expect("compat run should recover from denied command");

            assert_eq!(outcome.permission_denials.len(), 1);
            assert_eq!(outcome.permission_denials[0]["tool_name"], "bash_command");
            assert_eq!(
                outcome.permission_denials[0]["tool_input"]["command"],
                "echo hi"
            );
        });
    }
}
