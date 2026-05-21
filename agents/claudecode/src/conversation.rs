use std::collections::{BTreeSet, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use claude_config::{
    RuntimeConfig, SettingSource, import_legacy_profile, normalize_base_url,
    validate_provider_config,
};
use claude_core::{ConversationEntry, ConversationRole};
use claude_permissions::PermissionBroker;
use claude_protocol::{MessageRole, RuntimeEventDetail, UsagePayload};
use claude_provider::context::ContextWindowManager;
use claude_provider::query_source::ProviderRequestContext;
use claude_provider::{DiscoveredToolScope, ProviderCompatBackend, StreamingCallbacks};
use claude_session::resume_state::{PendingToolCall, ResumeState};
use claude_session::runtime_context::{
    persist_runtime_config_session_context, repair_interrupted_tool_batch,
    restore_runtime_config_session_context,
};
use claude_session::{SessionStore, conversation::ensure_conversation_initialized};
use claude_skills::SkillDocument;
use claude_tools::{
    ProgressCallback, ToolExecutionContext,
    agent::{DelegateProgressEvent, parse_delegate_progress_event},
    execute_tool_call,
    git::{apply_worktree_tool_result_to_runtime, sync_tool_context_from_runtime},
    mcp_runtime::discover_runtime_mcp_servers,
    plan_mode::normalize_exit_plan_mode_tool_calls,
    runtime_plan_mode::{
        build_runtime_plan_mode, inject_plan_mode_runtime_messages, install_plan_mode_runtime,
    },
    runtime_provider_tool_spec, runtime_tool_result_persistence_skip_names,
    tasks::load_persisted_ui_task_snapshots,
    tool_result_storage::{
        ContentReplacementRecord, ContentReplacementState,
        apply_tool_result_budget_to_conversation, process_tool_result_content,
        reconstruct_content_replacement_state,
    },
};
use claude_ui_bridge::UiTaskNode;

use crate::ResolvedPromptOverrides;
use crate::agents::build_remote_code_sub_agent_runtime;
use crate::cli::Cli;
use crate::conversation_backend::ConversationBackend;
use crate::hooks::{
    HookRunState, RuntimeHookDiscovery, apply_post_tool_hooks, apply_pre_tool_use_hooks,
    discover_runtime_hooks, ensure_session_start_hooks, wrap_permission_broker_with_hooks,
};
use crate::query_engine_compat::run_prompt_with_query_engine_compat;

pub(crate) const CONTENT_REPLACEMENT_EVENT_TYPE: &str = "content-replacement";

#[derive(Debug, Default)]
pub(crate) struct RuntimeExtensionDiscovery {
    pub(crate) skills: Vec<String>,
    pub(crate) plugins: Vec<String>,
    pub(crate) plugin_runtimes: Vec<String>,
    pub(crate) mcp_servers: Vec<String>,
    pub(crate) disabled_mcp_servers: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptRunOutcome {
    pub(crate) text: String,
    pub(crate) duration_ms: u64,
    pub(crate) duration_api_ms: u64,
    pub(crate) num_turns: u32,
    pub(crate) stop_reason: String,
    pub(crate) total_cost_usd: f64,
    pub(crate) usage: UsagePayload,
    pub(crate) model_usage: serde_json::Value,
    pub(crate) permission_denials: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct WizardSettingsDocument {
    provider: WizardSettingsProvider,
}

#[derive(Debug, Clone, serde::Serialize)]
struct WizardSettingsProvider {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    protocol: claude_core::ProviderProtocol,
}

#[derive(Debug, Clone)]
struct WizardProviderSelection {
    provider_name: String,
    protocol: claude_core::ProviderProtocol,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
}

pub(crate) type PromptEventSink = Arc<dyn Fn(PromptStreamEvent) + Send + Sync>;

pub(crate) fn load_content_replacement_records(
    store: &SessionStore,
    session_id: uuid::Uuid,
) -> Result<Vec<ContentReplacementRecord>> {
    let transcript = store.load_transcript(session_id)?;
    let mut records = Vec::new();
    for payload in transcript.named_event_payloads(CONTENT_REPLACEMENT_EVENT_TYPE) {
        if payload.get("sessionId").and_then(serde_json::Value::as_str)
            != Some(session_id.to_string().as_str())
        {
            continue;
        }
        if payload.get("agentId").is_some() {
            continue;
        }
        let Some(replacements) = payload.get("replacements").cloned() else {
            continue;
        };
        records.extend(serde_json::from_value::<Vec<ContentReplacementRecord>>(
            replacements,
        )?);
    }
    Ok(records)
}

pub(crate) fn provision_content_replacement_state(
    store: &SessionStore,
    session_id: uuid::Uuid,
    conversation: &[ConversationEntry],
) -> Result<ContentReplacementState> {
    let records = load_content_replacement_records(store, session_id)?;
    Ok(reconstruct_content_replacement_state(
        conversation,
        &records,
        None,
    ))
}

pub(crate) fn session_tool_results_dir(config: &RuntimeConfig) -> PathBuf {
    config
        .paths
        .sessions_dir
        .join(config.session_id.to_string())
        .join("tool-results")
}

pub(crate) struct ContentReplacementBackend {
    inner: Arc<dyn ConversationBackend>,
    store: Arc<SessionStore>,
    session_id: uuid::Uuid,
    tool_results_dir: PathBuf,
    state: tokio::sync::Mutex<ContentReplacementState>,
    skip_tool_names: HashSet<String>,
    persist_records: bool,
}

impl ContentReplacementBackend {
    pub(crate) fn new(
        inner: Arc<dyn ConversationBackend>,
        store: Arc<SessionStore>,
        session_id: uuid::Uuid,
        tool_results_dir: PathBuf,
        initial_state: ContentReplacementState,
        skip_tool_names: HashSet<String>,
    ) -> Arc<Self> {
        Self::new_with_options(
            inner,
            store,
            session_id,
            tool_results_dir,
            initial_state,
            skip_tool_names,
            true,
        )
    }

    pub(crate) fn new_with_options(
        inner: Arc<dyn ConversationBackend>,
        store: Arc<SessionStore>,
        session_id: uuid::Uuid,
        tool_results_dir: PathBuf,
        initial_state: ContentReplacementState,
        skip_tool_names: HashSet<String>,
        persist_records: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            store,
            session_id,
            tool_results_dir,
            state: tokio::sync::Mutex::new(initial_state),
            skip_tool_names,
            persist_records,
        })
    }

    pub(crate) async fn prepare_conversation(
        &self,
        conversation: &[ConversationEntry],
    ) -> Result<Vec<ConversationEntry>> {
        let mut provider_conversation = conversation.to_vec();
        let outcome = {
            let mut state = self.state.lock().await;
            apply_tool_result_budget_to_conversation(
                &mut provider_conversation,
                &mut state,
                &self.tool_results_dir,
                &self.skip_tool_names,
            )?
        };

        if self.persist_records && !outcome.newly_replaced.is_empty() {
            self.store.append_named_event(
                self.session_id,
                CONTENT_REPLACEMENT_EVENT_TYPE,
                serde_json::json!({
                    "sessionId": self.session_id,
                    "replacements": outcome.newly_replaced,
                }),
            )?;
        }

        Ok(provider_conversation)
    }
}

#[async_trait::async_trait]
impl ConversationBackend for ContentReplacementBackend {
    async fn complete(
        &self,
        conversation: &[ConversationEntry],
    ) -> Result<claude_core::ProviderResponse> {
        let provider_conversation = self.prepare_conversation(conversation).await?;
        self.inner.complete(&provider_conversation).await
    }

    async fn complete_with_context(
        &self,
        conversation: &[ConversationEntry],
        context: &ProviderRequestContext,
    ) -> Result<claude_core::ProviderResponse> {
        let provider_conversation = self.prepare_conversation(conversation).await?;
        self.inner
            .complete_with_context(&provider_conversation, context)
            .await
    }

    async fn complete_streaming(
        &self,
        conversation: &[ConversationEntry],
        callbacks: Option<StreamingCallbacks>,
    ) -> Result<claude_core::ProviderResponse> {
        let provider_conversation = self.prepare_conversation(conversation).await?;
        self.inner
            .complete_streaming(&provider_conversation, callbacks)
            .await
    }

    async fn complete_streaming_with_context(
        &self,
        conversation: &[ConversationEntry],
        callbacks: Option<StreamingCallbacks>,
        context: &ProviderRequestContext,
    ) -> Result<claude_core::ProviderResponse> {
        let provider_conversation = self.prepare_conversation(conversation).await?;
        self.inner
            .complete_streaming_with_context(&provider_conversation, callbacks, context)
            .await
    }

    fn sub_agent_completion(&self) -> Arc<dyn claude_core::SubAgentCompletion> {
        self.inner.sub_agent_completion()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PromptStreamEvent {
    MessageDelta {
        delta: String,
    },
    MessageCommitted {
        text: String,
    },
    MemorySaved {
        written_paths: Vec<String>,
        team_count: Option<usize>,
    },
    ToolStarted {
        tool_call_id: String,
        tool_name: String,
    },
    ToolProgress {
        tool_call_id: Option<String>,
        delta: Option<String>,
        elapsed_time_seconds: Option<u64>,
    },
    ToolFinished {
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
        summary: Option<String>,
    },
    SubtaskStarted {
        task_id: String,
        parent_task_id: Option<String>,
        description: String,
        depth: u32,
    },
    SubtaskProgress {
        task_id: String,
        turn: u32,
        max_turns: u32,
        summary: String,
    },
    SubtaskCompleted {
        task_id: String,
        success: bool,
        output_preview: String,
        turns_used: u32,
    },
    BatchProgress {
        total: usize,
        completed: usize,
        running: usize,
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
        entries_removed: usize,
        usage_ratio: f64,
    },
    TaskSnapshot {
        tasks: Vec<UiTaskNode>,
    },
}

impl PromptStreamEvent {
    #[must_use]
    pub(crate) fn runtime_event_detail(&self) -> Option<RuntimeEventDetail> {
        match self {
            Self::MessageDelta { delta } => Some(RuntimeEventDetail::MessageDelta {
                role: MessageRole::Assistant,
                delta: delta.clone(),
                message_id: None,
            }),
            Self::MessageCommitted { text } => Some(RuntimeEventDetail::MessageCommitted {
                role: MessageRole::Assistant,
                text: text.clone(),
                message_id: None,
            }),
            Self::ToolStarted {
                tool_call_id,
                tool_name,
            } => Some(RuntimeEventDetail::ToolStarted {
                tool_call_id: tool_call_id.clone().into(),
                tool_name: tool_name.clone().into(),
            }),
            Self::ToolProgress {
                tool_call_id,
                delta,
                elapsed_time_seconds,
            } => Some(RuntimeEventDetail::ToolProgress {
                tool_call_id: tool_call_id.clone().map(Into::into),
                tool_name: None,
                delta: delta.clone(),
                elapsed_time_seconds: *elapsed_time_seconds,
            }),
            Self::ToolFinished {
                tool_call_id,
                tool_name,
                is_error,
                summary,
            } => Some(RuntimeEventDetail::ToolFinished {
                tool_call_id: tool_call_id.clone().into(),
                tool_name: tool_name.clone().into(),
                is_error: *is_error,
                summary: summary.clone(),
            }),
            Self::SubtaskStarted { .. }
            | Self::MemorySaved { .. }
            | Self::SubtaskProgress { .. }
            | Self::SubtaskCompleted { .. }
            | Self::BatchProgress { .. }
            | Self::ContextUsage { .. }
            | Self::ContextOverflow { .. }
            | Self::ContextCompacted { .. }
            | Self::TaskSnapshot { .. } => None,
        }
    }
}

pub(crate) fn truncate_preview(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = collapsed.chars().take(max_chars).collect::<String>();
    if collapsed.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn session_task_dir(config: &RuntimeConfig) -> PathBuf {
    config
        .paths
        .artifacts_dir
        .join("tasks")
        .join(config.session_id.to_string())
}

pub(crate) fn restore_discovered_tool_scope(
    store: &SessionStore,
    session_id: uuid::Uuid,
    scope: &DiscoveredToolScope,
) -> Result<()> {
    if !store.session_transcript_path(session_id).exists() {
        scope.replace(std::collections::BTreeSet::new());
        return Ok(());
    }

    scope.replace(store.load_carried_discovered_tool_names(session_id)?);
    Ok(())
}

fn emit_task_snapshot_if_available(event_sink: &PromptEventSink, task_dir: &Path) {
    if let Ok(tasks) = load_persisted_ui_task_snapshots(task_dir) {
        event_sink(PromptStreamEvent::TaskSnapshot { tasks });
    }
}

fn is_permission_denied_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("permission denied")
        || (lowered.contains("permission") && lowered.contains("denied"))
}

pub(crate) fn build_prompt_progress_callback(
    config: &RuntimeConfig,
    event_sink: &PromptEventSink,
) -> Arc<ProgressCallback> {
    let event_sink = event_sink.clone();
    let task_dir = session_task_dir(config);
    Arc::new(move |message: &str| {
        let Some(event) = parse_delegate_progress_event(message) else {
            return;
        };
        match event {
            DelegateProgressEvent::SubtaskStarted {
                task_id,
                parent_task_id,
                description,
                depth,
            } => {
                event_sink(PromptStreamEvent::SubtaskStarted {
                    task_id,
                    parent_task_id,
                    description,
                    depth,
                });
                emit_task_snapshot_if_available(&event_sink, &task_dir);
            }
            DelegateProgressEvent::SubtaskProgress {
                task_id,
                turn,
                max_turns,
                summary,
            } => {
                event_sink(PromptStreamEvent::SubtaskProgress {
                    task_id,
                    turn,
                    max_turns,
                    summary,
                });
                emit_task_snapshot_if_available(&event_sink, &task_dir);
            }
            DelegateProgressEvent::SubtaskCompleted {
                task_id,
                success,
                output_preview,
                turns_used,
            } => {
                event_sink(PromptStreamEvent::SubtaskCompleted {
                    task_id,
                    success,
                    output_preview,
                    turns_used,
                });
                emit_task_snapshot_if_available(&event_sink, &task_dir);
            }
            DelegateProgressEvent::BatchProgress {
                total,
                completed,
                running,
            } => {
                event_sink(PromptStreamEvent::BatchProgress {
                    total,
                    completed,
                    running,
                });
                emit_task_snapshot_if_available(&event_sink, &task_dir);
            }
        }
    })
}

fn build_streaming_callbacks(
    include_partial_messages: bool,
    event_sink: PromptEventSink,
) -> StreamingCallbacks {
    let text_sink = event_sink.clone();
    let start_sink = event_sink.clone();
    let progress_sink = event_sink;
    StreamingCallbacks {
        on_text_delta: include_partial_messages.then(|| {
            Box::new(move |delta: &str| {
                if delta.is_empty() {
                    return;
                }
                text_sink(PromptStreamEvent::MessageDelta {
                    delta: delta.to_owned(),
                });
            }) as Box<dyn Fn(&str) + Send + Sync>
        }),
        on_tool_call_start: Some(Box::new(move |tool_call_id: &str, tool_name: &str| {
            if tool_call_id.is_empty() || tool_name.is_empty() {
                return;
            }
            start_sink(PromptStreamEvent::ToolStarted {
                tool_call_id: tool_call_id.to_owned(),
                tool_name: tool_name.to_owned(),
            });
        })),
        on_tool_call_delta: Some(Box::new(move |tool_call_id: &str, delta: &str| {
            if tool_call_id.is_empty() || delta.is_empty() {
                return;
            }
            progress_sink(PromptStreamEvent::ToolProgress {
                tool_call_id: Some(tool_call_id.to_owned()),
                delta: Some(delta.to_owned()),
                elapsed_time_seconds: None,
            });
        })),
        on_usage: None,
        on_thinking_delta: None,
        on_lifecycle_event: None,
    }
}

pub(crate) fn persist_session_context(store: &SessionStore, config: &RuntimeConfig) -> Result<()> {
    persist_runtime_config_session_context(store, config)
}

pub(crate) fn restore_session_context(
    store: &SessionStore,
    config: &mut RuntimeConfig,
) -> Result<()> {
    restore_runtime_config_session_context(store, config)
}

pub(crate) fn reapply_cli_overrides(
    cli: &Cli,
    prompt_overrides: &ResolvedPromptOverrides,
    config: &mut RuntimeConfig,
    permission_mode_explicit: bool,
) {
    if permission_mode_explicit {
        config.permission_mode = cli.permission_mode;
    }
    if let Some(cwd) = &cli.cwd {
        cwd.clone_into(&mut config.cwd);
    }
    if let Some(provider) = &cli.provider {
        provider.clone_into(&mut config.provider.name);
    }
    if let Some(model) = &cli.model {
        config.provider.model = Some(model.clone());
    }
    if let Some(effort) = cli
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.effort = Some(effort.to_owned());
    }
    if let Some(fallback_model) = cli
        .fallback_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.fallback_model = Some(fallback_model.to_owned());
        if config.provider.model.is_none() {
            config.provider.model = Some(fallback_model.to_owned());
        }
    }
    if let Some(output_style) = cli
        .output_style
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.output_style = Some(output_style.to_owned());
    }
    if let Some(language) = cli
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.language = Some(language.to_owned());
    }
    if cli.brief {
        config.brief_enabled = true;
    } else if cli.no_brief {
        config.brief_enabled = false;
    }
    if cli.proactive {
        config.proactive_active = true;
    } else if cli.no_proactive {
        config.proactive_active = false;
    }
    if cli.dangerously_skip_permissions {
        config.permission_mode = claude_core::PermissionMode::BypassPermissions;
    }
    if let Some(api_key) = &cli.api_key {
        config.provider.api_key = Some(api_key.clone());
        config.auth_source = Some("cli:api-key".to_owned());
    }
    if let Some(protocol) = cli.protocol {
        config.provider.protocol = protocol;
    }
    if let Some(base_url) = &cli.base_url {
        config.provider.base_url =
            normalize_base_url(Some(base_url.clone()), config.provider.protocol);
    } else if cli.protocol.is_some() {
        config.provider.base_url =
            normalize_base_url(config.provider.base_url.clone(), config.provider.protocol);
    }
    if prompt_overrides.system_prompt.is_some() || cli.system_prompt_file.is_some() {
        config.system_prompt = prompt_overrides.system_prompt.clone();
    }
    if prompt_overrides.append_system_prompt.is_some() || cli.append_system_prompt_file.is_some() {
        config.append_system_prompt = prompt_overrides.append_system_prompt.clone();
    }
}

pub(crate) fn discover_runtime_extensions(config: &RuntimeConfig) -> RuntimeExtensionDiscovery {
    let mut skills = BTreeSet::new();
    let mut plugins = BTreeSet::new();
    let mut plugin_runtimes = BTreeSet::new();
    let mut warnings = Vec::new();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);

    if user_sources_enabled && config.paths.skills_dir.exists() {
        collect_skill_names(
            claude_skills::discover_skills(&config.paths.skills_dir),
            &mut skills,
            &mut warnings,
            "profile skills",
        );
    }

    if user_sources_enabled && config.paths.plugins_dir.exists() {
        match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
            Ok(discovered_plugins) => {
                for plugin in discovered_plugins {
                    plugins.insert(plugin.manifest.name.clone());
                    if plugin.runtime_config().is_some() {
                        plugin_runtimes.insert(plugin.manifest.name.clone());
                    }
                    collect_skill_names(
                        plugin.discover_bundled_skills(),
                        &mut skills,
                        &mut warnings,
                        &format!("plugin {}", plugin.manifest.name),
                    );
                }
            }
            Err(error) => warnings.push(format!("Failed to discover plugins: {error}")),
        }
    }

    let mcp_discovery = discover_runtime_mcp_servers(config, &[]);
    let mcp_servers = mcp_discovery.enabled_server_names();
    let disabled_mcp_servers = mcp_discovery.disabled_server_names();
    warnings.extend(mcp_discovery.warnings);

    RuntimeExtensionDiscovery {
        skills: skills.into_iter().collect(),
        plugins: plugins.into_iter().collect(),
        plugin_runtimes: plugin_runtimes.into_iter().collect(),
        mcp_servers,
        disabled_mcp_servers,
        warnings,
    }
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn collect_skill_names(
    result: std::result::Result<Vec<SkillDocument>, claude_skills::SkillError>,
    skills: &mut BTreeSet<String>,
    warnings: &mut Vec<String>,
    source: &str,
) {
    match result {
        Ok(discovered) => {
            skills.extend(
                discovered
                    .into_iter()
                    .map(|skill| skill.metadata.slug)
                    .collect::<Vec<_>>(),
            );
        }
        Err(error) => warnings.push(format!("Failed to discover {source}: {error}")),
    }
}

pub(crate) fn initialize_conversation(
    store: &SessionStore,
    config: &RuntimeConfig,
    title_hint: Option<&str>,
) -> Result<Vec<ConversationEntry>> {
    let title_hint = config
        .session_name
        .as_deref()
        .or(title_hint)
        .or(config.provider.model.as_deref());
    persist_session_context(store, config)?;
    ensure_conversation_initialized(
        store,
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        title_hint,
    )
    .and_then(|mut conversation| {
        repair_interrupted_tool_batch(store, config.session_id, &mut conversation)?;
        inject_plan_mode_runtime_messages(store, config.session_id, &mut conversation)?;
        Ok(conversation)
    })
}

pub(crate) async fn prepare_prompt_runtime_state(
    store: &SessionStore,
    config: &RuntimeConfig,
    discovered_tool_scope: &DiscoveredToolScope,
    discovery: &RuntimeHookDiscovery,
    title_hint: Option<&str>,
) -> Result<(Vec<ConversationEntry>, HookRunState)> {
    let mut conversation = initialize_conversation(store, config, title_hint)?;
    restore_discovered_tool_scope(store, config.session_id, discovered_tool_scope)?;
    let mut hook_state = HookRunState::load(store, config.session_id)?;
    ensure_session_start_hooks(discovery, config, store, &mut conversation, &mut hook_state)
        .await?;
    Ok((conversation, hook_state))
}

pub(crate) fn has_unanswered_user_prompt(conversation: &[ConversationEntry], prompt: &str) -> bool {
    let normalized = prompt.trim();
    if normalized.is_empty() {
        return false;
    }

    conversation
        .iter()
        .rev()
        .take_while(|entry| entry.role != ConversationRole::Assistant)
        .any(|entry| {
            entry.role == ConversationRole::User
                && entry.attachments.is_empty()
                && entry.history_text().trim() == normalized
        })
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prompt(
    config: &mut RuntimeConfig,
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
    let replacement_state =
        provision_content_replacement_state(store, config.session_id, conversation)?;
    let skip_tool_names = runtime_tool_result_persistence_skip_names();
    let backend = ContentReplacementBackend::new(
        backend,
        Arc::new(SessionStore::open(config.paths.clone())?),
        config.session_id,
        session_tool_results_dir(config),
        replacement_state,
        skip_tool_names,
    );
    let broker = wrap_permission_broker_with_hooks(broker, discovery, config);
    let outcome = if env::var_os("REMOTE_CODE_FORCE_LEGACY_PROMPT_LOOP").is_some() {
        run_prompt_legacy(
            config,
            store,
            backend.clone(),
            broker,
            event_sink,
            discovery,
            hook_state,
            conversation,
            prompt,
        )
        .await
    } else {
        run_prompt_with_query_engine_compat(
            config,
            store,
            backend.clone(),
            discovered_tool_scope,
            broker,
            event_sink,
            discovery,
            hook_state,
            conversation,
            prompt,
        )
        .await
    }?;

    Ok(outcome)
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
async fn run_prompt_legacy(
    config: &mut RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    broker: Arc<dyn PermissionBroker>,
    event_sink: Option<PromptEventSink>,
    discovery: &RuntimeHookDiscovery,
    hook_state: &mut HookRunState,
    conversation: &mut Vec<ConversationEntry>,
    prompt: &str,
) -> Result<PromptRunOutcome> {
    let readiness = validate_provider_config(&config.provider);
    if !readiness.ok {
        return Err(anyhow!(readiness.issues.join(" ")));
    }

    let started = Instant::now();
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
    if !has_unanswered_user_prompt(conversation, prompt) {
        let user_entry = ConversationEntry::user(prompt);
        store.append_conversation_entry(config.session_id, &user_entry)?;
        conversation.push(user_entry);
    }

    let progress_cb = event_sink
        .as_ref()
        .map(|event_sink| build_prompt_progress_callback(config, event_sink));

    let read_file_state = claude_tools::FileStateCache::new();
    let mut tool_context = ToolExecutionContext {
        cwd: config.cwd.clone(),
        original_cwd: config.original_cwd.clone(),
        active_worktree_session: config.active_worktree_session.clone(),
        timeout_ms: config.provider.timeout_ms,
        sub_agent: Some(build_remote_code_sub_agent_runtime(
            config,
            backend.sub_agent_completion(),
            read_file_state.clone(),
        )),
        progress_cb,
        task_stack: std::sync::Arc::new(parking_lot::Mutex::new(
            claude_core::task_stack::TaskStack::default(),
        )),
        read_file_state,
        sub_agent_output_tokens: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    let mut usage = UsagePayload::default();
    let mut num_turns = 0u32;
    let mut permission_denials = Vec::new();
    let mut total_tool_calls = 0usize;
    let model_name = config.provider.model.as_deref().unwrap_or("unknown");
    let context_manager = ContextWindowManager::for_model(model_name);
    for turn_index in 0..config.max_turns {
        num_turns += 1;

        let budget_snapshot = context_manager.budget_snapshot(conversation);
        if let Some(event_sink) = event_sink.as_ref() {
            event_sink(PromptStreamEvent::ContextUsage {
                estimated_tokens: budget_snapshot.estimated_tokens,
                max_input_tokens: budget_snapshot.max_input_tokens,
                threshold_tokens: budget_snapshot.threshold_tokens(),
                ratio: budget_snapshot.usage_ratio,
            });
        }

        // Compact conversation if context window is getting full.
        if budget_snapshot.exceeds_threshold() {
            if let Some(event_sink) = event_sink.as_ref() {
                event_sink(PromptStreamEvent::ContextOverflow {
                    estimated_tokens: budget_snapshot.estimated_tokens,
                    max_input_tokens: budget_snapshot.max_input_tokens,
                    threshold_tokens: budget_snapshot.threshold_tokens(),
                    ratio: budget_snapshot.usage_ratio,
                });
            }
            store.append_named_event(
                config.session_id,
                "context_overflow",
                serde_json::json!({
                    "turn": turn_index + 1,
                    "estimated_tokens": budget_snapshot.estimated_tokens,
                    "max_input_tokens": budget_snapshot.max_input_tokens,
                    "threshold_tokens": budget_snapshot.threshold_tokens(),
                    "usage_ratio": budget_snapshot.usage_ratio,
                }),
            )?;

            let compacted = context_manager.compact(conversation);
            let removed = conversation.len().saturating_sub(compacted.len());
            *conversation = compacted;
            let compacted_snapshot = context_manager.budget_snapshot(conversation);

            if removed > 0 {
                store.append_named_event(
                    config.session_id,
                    "context_compacted",
                    serde_json::json!({
                        "turn": turn_index + 1,
                        "entries_removed": removed,
                        "usage_ratio_before": budget_snapshot.usage_ratio,
                        "usage_ratio_after": compacted_snapshot.usage_ratio,
                        "estimated_tokens_before": budget_snapshot.estimated_tokens,
                        "estimated_tokens_after": compacted_snapshot.estimated_tokens,
                    }),
                )?;
                if let Some(event_sink) = event_sink.as_ref() {
                    event_sink(PromptStreamEvent::ContextCompacted {
                        entries_removed: removed,
                        usage_ratio: compacted_snapshot.usage_ratio,
                    });
                    event_sink(PromptStreamEvent::ContextUsage {
                        estimated_tokens: compacted_snapshot.estimated_tokens,
                        max_input_tokens: compacted_snapshot.max_input_tokens,
                        threshold_tokens: compacted_snapshot.threshold_tokens(),
                        ratio: compacted_snapshot.usage_ratio,
                    });
                }
            }
        }

        let mut response = if let Some(event_sink) = event_sink.clone() {
            backend
                .complete_streaming(
                    conversation,
                    Some(build_streaming_callbacks(
                        config.include_partial_messages,
                        event_sink,
                    )),
                )
                .await?
        } else {
            backend.complete(conversation).await?
        };
        normalize_exit_plan_mode_tool_calls(&mut response.tool_calls);
        usage.input_tokens += response.usage.input_tokens;
        usage.output_tokens += response.usage.output_tokens;
        usage.cache_read_input_tokens += response.usage.cache_read_input_tokens;
        usage.cache_creation_input_tokens += response.usage.cache_creation_input_tokens;
        // Handle max_tokens stop reason — log warning and annotate response.
        let lowered_stop = response.stop_reason.to_ascii_lowercase();
        if lowered_stop == "max_tokens"
            || lowered_stop == "max_tokens_reached"
            || lowered_stop == "length"
        {
            tracing::warn!(
                "Legacy prompt loop: response truncated (stop_reason={}), output may be incomplete",
                response.stop_reason
            );
            if !response.text.is_empty() {
                response
                    .text
                    .push_str("\n\n[Output truncated — max output token limit reached.]");
            }
        }
        total_tool_calls += response.tool_calls.len();
        let assistant_entry = ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::Assistant,
            text: response.text.clone(),
            history_text: response.history_text.clone(),
            content_blocks: response.content_blocks.clone(),
            tool_calls: response.tool_calls.clone(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        };
        store.append_conversation_entry(config.session_id, &assistant_entry)?;
        conversation.push(assistant_entry);
        store.append_named_event(
            config.session_id,
            "assistant_turn",
            serde_json::json!({
                "turn": turn_index + 1,
                "stop_reason": response.stop_reason,
                "usage": {
                    "input_tokens": response.usage.input_tokens,
                    "output_tokens": response.usage.output_tokens,
                },
                "tool_calls": response.tool_calls.len(),
                "text_preview": truncate_preview(&response.text, 160),
            }),
        )?;
        if let Some(event_sink) = event_sink.as_ref()
            && !response.text.trim().is_empty()
        {
            event_sink(PromptStreamEvent::MessageCommitted {
                text: response.text.clone(),
            });
        }

        if response.tool_calls.is_empty() {
            store.clear_resume_state(config.session_id)?;
            #[allow(clippy::cast_possible_truncation)]
            let duration_ms = started.elapsed().as_millis() as u64;
            let outcome = PromptRunOutcome {
                text: response.text,
                duration_ms,
                duration_api_ms: duration_ms,
                num_turns,
                stop_reason: response.stop_reason.clone(),
                total_cost_usd: 0.0,
                usage,
                model_usage: serde_json::json!({
                    "provider": config.provider.name.clone(),
                    "model": config.provider.model.clone(),
                    "protocol": config.provider.protocol.as_str(),
                    "turns": num_turns,
                    "tool_calls": total_tool_calls,
                }),
                permission_denials,
            };
            store.append_named_event(
                config.session_id,
                "result",
                serde_json::json!({
                    "is_error": false,
                    "stop_reason": response.stop_reason,
                    "usage": {
                        "input_tokens": outcome.usage.input_tokens,
                        "output_tokens": outcome.usage.output_tokens,
                    },
                    "duration_ms": duration_ms,
                    "num_turns": outcome.num_turns,
                }),
            )?;
            return Ok(outcome);
        }

        let pending_tool_calls = response
            .tool_calls
            .iter()
            .map(|tool_call| PendingToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                input: tool_call.input.clone(),
            })
            .collect::<Vec<_>>();
        store.save_resume_state(
            config.session_id,
            &ResumeState::from_pending_calls(pending_tool_calls),
        )?;

        for tool_call in &response.tool_calls {
            let original_tool_spec = runtime_provider_tool_spec(&tool_call.name)
                .await
                .ok_or_else(|| anyhow!("unknown tool {}", tool_call.name))?;

            let prepared = apply_pre_tool_use_hooks(
                discovery,
                config,
                store,
                conversation,
                hook_state,
                tool_call,
            )
            .await?;

            let effective_tool_call = prepared.call;
            let effective_tool_spec = if effective_tool_call.name == original_tool_spec.name {
                original_tool_spec
            } else {
                runtime_provider_tool_spec(&effective_tool_call.name)
                    .await
                    .ok_or_else(|| anyhow!("unknown tool {}", effective_tool_call.name))?
            };
            let audit_count_before = broker.audit_records().len();
            let tool_result = if let Some(blocked_reason) = &prepared.blocked_reason {
                claude_core::ToolResult {
                    content: blocked_reason.clone(),
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: Vec::new(),
                }
            } else {
                let fork_snapshot = claude_core::SubAgentForkSnapshot {
                    fork_context_messages: conversation
                        .iter()
                        .cloned()
                        .map(claude_core::Message::from)
                        .collect(),
                    system_prompt: conversation
                        .iter()
                        .find(|entry| entry.role == ConversationRole::System)
                        .map(|entry| entry.text.clone())
                        .filter(|text| !text.trim().is_empty()),
                    user_context: std::collections::BTreeMap::new(),
                    system_context: std::collections::BTreeMap::new(),
                };
                let fork_snapshot_provider: Arc<claude_tools::RuntimeForkSnapshotProvider> =
                    Arc::new(move || fork_snapshot.clone());
                // Capture tool execution errors as error tool results instead of
                // propagating, to keep conversation state consistent for the next
                // provider call.  This matches the TUI error-recovery pattern.
                match claude_tools::with_runtime_fork_snapshot_provider(
                    fork_snapshot_provider,
                    async {
                        execute_tool_call(&effective_tool_call, &tool_context, broker.as_ref())
                            .await
                    },
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        let tool_name = effective_tool_call.name.clone();
                        let tool_id = effective_tool_call.id.clone();
                        tracing::warn!("tool execution error for {tool_name}: {error}");
                        store.append_named_event(
                            config.session_id,
                            "tool_error",
                            serde_json::json!({
                                "tool_name": tool_name,
                                "tool_use_id": tool_id,
                                "error": format!("{error:#}"),
                            }),
                        )?;
                        claude_core::ToolResult {
                            content: format!("Tool execution error: {error}"),
                            is_error: true,
                            content_blocks: Vec::new(),
                            follow_up_user_blocks: Vec::new(),
                        }
                    }
                }
            };
            let new_audits = broker
                .audit_records()
                .into_iter()
                .skip(audit_count_before)
                .collect::<Vec<_>>();
            for audit in new_audits {
                store.append_named_event(
                    config.session_id,
                    "permission_decision",
                    serde_json::to_value(&audit)?,
                )?;
            }
            let is_permission_denied =
                tool_result.is_error && is_permission_denied_message(&tool_result.content);
            if is_permission_denied || prepared.blocked_reason.is_some() {
                permission_denials.push(serde_json::json!({
                    "tool_name": effective_tool_call.name,
                    "tool_use_id": effective_tool_call.id,
                    "tool_input": effective_tool_call.input.clone(),
                    "message": tool_result.content.clone(),
                }));
            }
            let tool_results_dir = config
                .paths
                .sessions_dir
                .join(config.session_id.to_string())
                .join("tool-results");
            let processed_result = process_tool_result_content(
                &tool_result.content,
                &tool_result.content_blocks,
                &effective_tool_call.id,
                &effective_tool_call.name,
                Some(&tool_results_dir),
                effective_tool_spec.tool_result_size_policy(),
            )?;
            let tool_preview = truncate_preview(&processed_result.content, 160);
            let processed_tool_result = claude_core::ToolResult {
                content: processed_result.content.clone(),
                is_error: tool_result.is_error,
                content_blocks: processed_result.content_blocks.clone(),
                follow_up_user_blocks: tool_result.follow_up_user_blocks.clone(),
            };
            if apply_worktree_tool_result_to_runtime(
                &effective_tool_call.name,
                &effective_tool_call.input,
                &processed_tool_result,
                config,
                &mut tool_context,
            )? {
                persist_session_context(store, config)?;
                sync_tool_context_from_runtime(config, &mut tool_context);
            }
            if let Some(event_sink) = event_sink.as_ref() {
                event_sink(PromptStreamEvent::ToolFinished {
                    tool_call_id: effective_tool_call.id.clone(),
                    tool_name: effective_tool_call.name.clone(),
                    is_error: tool_result.is_error,
                    summary: Some(tool_preview.clone()),
                });
            }
            let truncated_content =
                context_manager.truncate_tool_output_default(&processed_result.content);
            let mut tool_entry = ConversationEntry::tool(
                effective_tool_call.id.clone(),
                effective_tool_call.name.clone(),
                truncated_content,
                tool_result.is_error,
            );
            tool_entry.content_blocks = processed_result.content_blocks.clone();
            store.append_conversation_entry(config.session_id, &tool_entry)?;
            store.append_named_event(
                config.session_id,
                "tool_result",
                serde_json::json!({
                    "tool_name": effective_tool_call.name,
                    "tool_use_id": effective_tool_call.id,
                    "is_error": tool_entry.is_error,
                    "content_preview": tool_preview,
                }),
            )?;
            conversation.push(tool_entry);
            if !tool_result.follow_up_user_blocks.is_empty() {
                let follow_up_entry = ConversationEntry::user_with_content_blocks(
                    tool_result.follow_up_user_blocks.clone(),
                );
                store.append_conversation_entry(config.session_id, &follow_up_entry)?;
                conversation.push(follow_up_entry);
            }

            apply_post_tool_hooks(
                discovery,
                config,
                store,
                conversation,
                hook_state,
                &effective_tool_call,
                &tool_result,
            )
            .await?;
        }
        store.clear_resume_state(config.session_id)?;
    }
    let error = anyhow!(
        "Maximum turn budget reached ({}) without a final assistant reply.",
        config.max_turns
    );
    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = started.elapsed().as_millis() as u64;
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
            "num_turns": num_turns,
            "error": error.to_string(),
        }),
    )?;
    Err(error)
}

pub(crate) async fn run_oneshot_text(
    config: &mut RuntimeConfig,
    store: &SessionStore,
    prompt: String,
) -> Result<()> {
    let backend = ProviderCompatBackend::new(
        Arc::new(claude_provider::ProviderClient::new()?),
        &config.provider,
    );
    let discovered_tool_scope = backend.discovered_tool_scope();
    let (plan_mode_controller, broker) = build_runtime_plan_mode(config, store)?;
    let _plan_mode_runtime = install_plan_mode_runtime(plan_mode_controller)?;
    let discovery = discover_runtime_hooks(config, &[]);
    let (mut conversation, mut hook_state) = prepare_prompt_runtime_state(
        store,
        config,
        &discovered_tool_scope,
        &discovery,
        Some(&prompt),
    )
    .await?;
    let response = run_prompt(
        config,
        store,
        Arc::new(backend),
        discovered_tool_scope,
        broker,
        None,
        &discovery,
        &mut hook_state,
        &mut conversation,
        &prompt,
    )
    .await?;
    println!("{}", response.text);
    Ok(())
}

pub(crate) fn run_migrate(
    config: &RuntimeConfig,
    command: crate::cli::MigrateCommand,
) -> Result<()> {
    match command {
        crate::cli::MigrateCommand::Import { source } => {
            let summary = import_legacy_profile(source, &config.paths)?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
    }
}

/// Detect whether this is a first run and launch an interactive setup wizard.
///
/// A first run is detected when:
/// - No API key is configured (neither env var nor CLI override)
/// - No active runtime settings files were loaded
///
/// The wizard guides the user through:
/// 1. Provider selection (Anthropic / OpenAI / DeepSeek / GLM / Custom)
/// 2. API key entry
/// 3. Model selection (with sensible defaults per provider)
/// 4. Saves the configuration to the active settings target
pub(crate) fn run_first_run_wizard(config: &mut RuntimeConfig) -> Result<()> {
    if !should_run_first_run_wizard(config) {
        return Ok(());
    }

    // Only run the wizard when connected to a terminal (stdin is tty).
    // In headless / CI environments, skip silently.
    if !atty_check() {
        eprintln!(
            "⚠ No API key configured. Set --api-key or a compatible env var such as REMOTE_CODE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, or a provider-specific key."
        );
        return Ok(());
    }

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║          Welcome to Remote Code Rust!                   ║");
    println!("║                                                          ║");
    println!("║  Let's set up your provider configuration.              ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();

    // Step 1: Provider selection
    println!("Which LLM provider would you like to use?");
    println!("  1) Anthropic (Claude)");
    println!("  2) OpenAI (GPT / o-series)");
    println!("  3) DeepSeek");
    println!("  4) 智谱 AI (GLM)");
    println!("  5) MiniMax");
    println!("  6) Custom (OpenAI-compatible)");
    println!("  7) Custom (Anthropic-compatible)");
    println!();
    let provider_choice = read_line_prompt("Enter choice [1-7]: ")?;

    let (provider_name, protocol, default_base_url, default_model) = match provider_choice.trim() {
        "1" => (
            "anthropic",
            claude_core::ProviderProtocol::Anthropic,
            "https://api.anthropic.com",
            "claude-sonnet-4-20250514",
        ),
        "2" => (
            "openai",
            claude_core::ProviderProtocol::OpenAi,
            "https://api.openai.com",
            "gpt-4o",
        ),
        "3" => (
            "deepseek",
            claude_core::ProviderProtocol::OpenAi,
            "https://api.deepseek.com",
            "deepseek-chat",
        ),
        "4" => (
            "glm",
            claude_core::ProviderProtocol::OpenAi,
            "https://open.bigmodel.cn/api/paas",
            "glm-5.1",
        ),
        "5" => (
            "minimax",
            claude_core::ProviderProtocol::OpenAi,
            "https://api.minimax.chat",
            "MiniMax-M1",
        ),
        "6" => ("custom", claude_core::ProviderProtocol::OpenAi, "", ""),
        "7" => ("custom", claude_core::ProviderProtocol::Anthropic, "", ""),
        _ => {
            println!("  → Using default: OpenAI-compatible");
            ("custom", claude_core::ProviderProtocol::OpenAi, "", "")
        }
    };

    // Step 2: Base URL
    let base_url = if default_base_url.is_empty() {
        let input = read_line_prompt("Enter base URL: ")?;
        let trimmed = input.trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else {
        let input = read_line_prompt(&format!("Base URL [{default_base_url}]: "))?;
        let trimmed = input.trim().to_owned();
        if trimmed.is_empty() {
            Some(default_base_url.to_owned())
        } else {
            Some(trimmed)
        }
    };

    // Step 3: API Key
    let api_key = {
        let input = read_line_prompt("Enter your API key: ")?;
        let trimmed = input.trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    };

    if api_key.is_none() {
        println!();
        println!("  ⚠ No API key entered. You can set it later via:");
        println!(
            "    --api-key, REMOTE_CODE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, or a provider-specific env var"
        );
        println!();
    }

    // Step 4: Model
    let model = if default_model.is_empty() {
        let input = read_line_prompt("Enter model name: ")?;
        let trimmed = input.trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else {
        let input = read_line_prompt(&format!("Model [{default_model}]: "))?;
        let trimmed = input.trim().to_owned();
        if trimmed.is_empty() {
            Some(default_model.to_owned())
        } else {
            Some(trimmed)
        }
    };

    let selection = WizardProviderSelection {
        provider_name: provider_name.to_owned(),
        protocol,
        base_url,
        api_key,
        model,
    };

    // Step 5: Save to the currently-active settings target.
    let settings_path = resolve_first_run_settings_path(config)?;
    write_wizard_settings_file(&settings_path, &selection)?;
    println!();
    println!("  ✓ Configuration saved to {}", settings_path.display());

    // Step 6: Apply to current config
    apply_wizard_settings(config, &settings_path, &selection);

    println!("  ✓ Provider: {}", selection.provider_name);
    if let Some(m) = &selection.model {
        println!("  ✓ Model:    {m}");
    }
    println!();
    println!("  Setup complete! Run `remote-code doctor` to verify your configuration.");
    println!();

    Ok(())
}

fn should_run_first_run_wizard(config: &RuntimeConfig) -> bool {
    config.provider.api_key.is_none() && config.settings_files.is_empty()
}

fn resolve_first_run_settings_path(config: &RuntimeConfig) -> Result<PathBuf> {
    if let Some(path) = config.cli_settings_files.last() {
        return Ok(path.clone());
    }

    for source in [
        SettingSource::Local,
        SettingSource::Project,
        SettingSource::User,
    ] {
        if config.allowed_setting_sources.contains(&source) {
            return Ok(match source {
                SettingSource::User => config.paths.profile_dir.join("settings.json"),
                SettingSource::Project => config.cwd.join(".remote-code").join("settings.json"),
                SettingSource::Local => config.cwd.join(".remote-code").join("settings.local.json"),
            });
        }
    }

    Err(anyhow!(
        "No writable settings target is available; enable at least one of user/project/local or pass --settings"
    ))
}

fn wizard_settings_document(selection: &WizardProviderSelection) -> WizardSettingsDocument {
    WizardSettingsDocument {
        provider: WizardSettingsProvider {
            name: selection.provider_name.clone(),
            base_url: selection.base_url.clone(),
            api_key: selection.api_key.clone(),
            model: selection.model.clone(),
            protocol: selection.protocol,
        },
    }
}

fn write_wizard_settings_file(path: &Path, selection: &WizardProviderSelection) -> Result<()> {
    let document = wizard_settings_document(selection);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "toml" {
        std::fs::write(path, toml::to_string_pretty(&document)?)?;
    } else {
        let settings_file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(settings_file, &document)?;
    }
    Ok(())
}

fn apply_wizard_settings(
    config: &mut RuntimeConfig,
    settings_path: &Path,
    selection: &WizardProviderSelection,
) {
    config.provider.name = selection.provider_name.clone();
    config.provider.protocol = selection.protocol;
    config.provider.base_url = normalize_base_url(selection.base_url.clone(), selection.protocol);
    config.provider.api_key = selection.api_key.clone();
    config.provider.model = selection.model.clone();
    config.auth_source = selection
        .api_key
        .as_ref()
        .map(|_| format!("settings:{}", settings_path.display()));
    config.settings_files = claude_config::settings_layers::resolve_runtime_settings_files(
        &config.cwd,
        &config.paths.profile_dir,
        &config.paths.profiles_dir,
        &config.cli_settings_files,
        &config.allowed_setting_sources,
    );
    config.setting_sources = config
        .settings_files
        .iter()
        .map(|path| format!("settings:{}", path.display()))
        .collect();
}

/// Check if stdin is connected to a terminal (TTY).
fn atty_check() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Read a line from stdin with a prompt.
fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{prompt}");
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    use anyhow::Result;
    use async_trait::async_trait;
    use clap::Parser;
    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use claude_core::{
        ConversationEntry, ConversationRole, ProviderResponse, SubAgentCompletion, ToolCall,
    };
    use claude_protocol::{MessageRole, RuntimeEventDetail};
    use claude_provider::StreamingCallbacks;
    use claude_session::SessionStore;
    use claude_session::resume_state::{PendingToolCall, ResumeState};
    use tempfile::tempdir;

    use super::{
        ContentReplacementBackend, PromptStreamEvent, WizardProviderSelection,
        apply_wizard_settings, discover_runtime_extensions, has_unanswered_user_prompt,
        initialize_conversation, provision_content_replacement_state, reapply_cli_overrides,
        resolve_first_run_settings_path, session_tool_results_dir, should_run_first_run_wizard,
        write_wizard_settings_file,
    };
    use crate::ResolvedPromptOverrides;
    use crate::conversation_backend::ConversationBackend;

    struct DummyCompletion;

    #[async_trait]
    impl SubAgentCompletion for DummyCompletion {
        async fn complete(
            &self,
            _conversation: &[ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            Ok(ProviderResponse::default())
        }
    }

    #[derive(Default)]
    struct RecordingBackend {
        conversations: StdMutex<Vec<Vec<ConversationEntry>>>,
    }

    #[async_trait]
    impl ConversationBackend for RecordingBackend {
        async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            self.conversations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(conversation.to_vec());
            Ok(ProviderResponse {
                text: "ok".to_owned(),
                ..ProviderResponse::default()
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
            Arc::new(DummyCompletion)
        }
    }

    fn test_config() -> (tempfile::TempDir, claude_config::RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        (tempdir, config)
    }

    fn sample_wizard_selection() -> WizardProviderSelection {
        WizardProviderSelection {
            provider_name: "glm".to_owned(),
            protocol: claude_core::ProviderProtocol::OpenAi,
            base_url: Some("https://open.bigmodel.cn/api/paas".to_owned()),
            api_key: Some("secret".to_owned()),
            model: Some("glm-5.1".to_owned()),
        }
    }

    #[test]
    fn prompt_stream_event_maps_message_delta_to_shared_runtime_event() {
        let event = PromptStreamEvent::MessageDelta {
            delta: "hello".to_owned(),
        };

        assert_eq!(
            event.runtime_event_detail(),
            Some(RuntimeEventDetail::MessageDelta {
                role: MessageRole::Assistant,
                delta: "hello".to_owned(),
                message_id: None,
            })
        );
    }

    #[test]
    fn prompt_stream_event_keeps_non_runtime_only_events_local() {
        let event = PromptStreamEvent::ContextUsage {
            estimated_tokens: 10,
            max_input_tokens: 100,
            threshold_tokens: 80,
            ratio: 0.1,
        };

        assert_eq!(event.runtime_event_detail(), None);
    }

    #[test]
    fn wizard_round_trips_loader_compatible_settings_schema() {
        let (_tempdir, config) = test_config();
        let settings_path = config.paths.profile_dir.join("settings.json");
        let selection = sample_wizard_selection();

        write_wizard_settings_file(&settings_path, &selection).expect("wizard settings");
        let resolved = claude_config::settings_layers::load_runtime_settings(&[settings_path])
            .expect("settings should load");

        assert_eq!(resolved.provider_name.as_deref(), Some("glm"));
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://open.bigmodel.cn/api/paas")
        );
        assert_eq!(resolved.api_key.as_deref(), Some("secret"));
        assert_eq!(resolved.model.as_deref(), Some("glm-5.1"));
        assert_eq!(
            resolved.protocol,
            Some(claude_core::ProviderProtocol::OpenAi)
        );
    }

    #[test]
    fn wizard_prefers_explicit_settings_target_and_updates_runtime_metadata() {
        let (_tempdir, mut config) = test_config();
        let explicit = config.cwd.join("configs").join("wizard.toml");
        config.cli_settings_files = vec![
            config.cwd.join("configs").join("base.toml"),
            explicit.clone(),
        ];
        let selection = sample_wizard_selection();

        let target = resolve_first_run_settings_path(&config).expect("target path");
        assert_eq!(target, explicit);

        write_wizard_settings_file(&target, &selection).expect("write explicit settings");
        apply_wizard_settings(&mut config, &target, &selection);

        let rendered = fs::read_to_string(&target).expect("settings file");
        assert!(rendered.contains("[provider]"));
        assert!(rendered.contains("name = \"glm\""));
        assert_eq!(config.settings_files, config.cli_settings_files);
        assert_eq!(
            config.setting_sources,
            vec![
                format!("settings:{}", config.cli_settings_files[0].display()),
                format!("settings:{}", config.cli_settings_files[1].display())
            ]
        );
        assert_eq!(
            config.auth_source.as_deref(),
            Some(format!("settings:{}", target.display()).as_str())
        );
    }

    #[test]
    fn reapply_cli_overrides_restores_permission_mode() {
        let (_tempdir, mut config) = test_config();
        config.permission_mode = claude_core::PermissionMode::Default;

        let cli = crate::cli::Cli::parse_from([
            "remote-code",
            "--permission-mode",
            "accept-edits",
            "resume prompt",
        ]);
        reapply_cli_overrides(&cli, &ResolvedPromptOverrides::default(), &mut config, true);

        assert_eq!(
            config.permission_mode,
            claude_core::PermissionMode::AcceptEdits
        );
    }

    #[test]
    fn reapply_cli_overrides_restores_runtime_knobs() {
        let (_tempdir, mut config) = test_config();
        config.effort = None;
        config.fallback_model = None;
        config.output_style = None;
        config.language = None;
        config.brief_enabled = false;
        config.proactive_active = true;

        let cli = crate::cli::Cli::parse_from([
            "remote-code",
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
            "resume prompt",
        ]);
        reapply_cli_overrides(
            &cli,
            &ResolvedPromptOverrides::default(),
            &mut config,
            false,
        );

        assert_eq!(config.effort.as_deref(), Some("high"));
        assert_eq!(config.fallback_model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(config.output_style.as_deref(), Some("concise"));
        assert_eq!(config.language.as_deref(), Some("zh-CN"));
        assert!(config.brief_enabled);
        assert!(!config.proactive_active);
        assert_eq!(
            config.permission_mode,
            claude_core::PermissionMode::BypassPermissions
        );
    }

    #[tokio::test]
    async fn content_replacement_backend_rewrites_prompt_and_records_transcript() {
        let (_tempdir, config) = test_config();
        let store = Arc::new(SessionStore::open(config.paths.clone()).expect("store"));
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("content replacement"),
            )
            .expect("ensure session");
        let backend = Arc::new(RecordingBackend::default());
        let replacement_backend = ContentReplacementBackend::new(
            backend.clone(),
            store.clone(),
            config.session_id,
            session_tool_results_dir(&config),
            super::ContentReplacementState::new(),
            std::collections::HashSet::new(),
        );
        let conversation = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("call-large", "bash_command", "x".repeat(210_000), false),
        ];

        replacement_backend
            .complete(&conversation)
            .await
            .expect("complete");

        let captured = backend
            .conversations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0][1].text.starts_with("<persisted-output>"));
        assert!(
            session_tool_results_dir(&config)
                .join("call-large.txt")
                .exists()
        );

        let records = super::load_content_replacement_records(store.as_ref(), config.session_id)
            .expect("replacement records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool_use_id, "call-large");
        assert_eq!(records[0].replacement, captured[0][1].text);
    }

    #[tokio::test]
    async fn content_replacement_resume_reapplies_exact_transcript_replacement() {
        let (_tempdir, config) = test_config();
        let store = Arc::new(SessionStore::open(config.paths.clone()).expect("store"));
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("content replacement resume"),
            )
            .expect("ensure session");
        let original = vec![
            ConversationEntry::assistant(""),
            ConversationEntry::tool("call-large", "bash_command", "x".repeat(210_000), false),
        ];
        let first_backend = Arc::new(RecordingBackend::default());
        let first = ContentReplacementBackend::new(
            first_backend,
            store.clone(),
            config.session_id,
            session_tool_results_dir(&config),
            super::ContentReplacementState::new(),
            std::collections::HashSet::new(),
        );
        first.complete(&original).await.expect("first complete");

        let state =
            provision_content_replacement_state(store.as_ref(), config.session_id, &original)
                .expect("resume state");
        let second_backend = Arc::new(RecordingBackend::default());
        let second = ContentReplacementBackend::new(
            second_backend.clone(),
            store.clone(),
            config.session_id,
            session_tool_results_dir(&config),
            state,
            std::collections::HashSet::new(),
        );
        second.complete(&original).await.expect("second complete");

        let captured = second_backend
            .conversations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let records = super::load_content_replacement_records(store.as_ref(), config.session_id)
            .expect("replacement records");
        assert_eq!(records.len(), 1);
        assert_eq!(captured[0][1].text, records[0].replacement);
    }

    #[test]
    fn reapply_cli_overrides_preserves_restored_permission_mode_when_cli_is_default() {
        let (_tempdir, mut config) = test_config();
        config.permission_mode = claude_core::PermissionMode::Plan;

        let cli = crate::cli::Cli::parse_from(["remote-code", "resume prompt"]);
        reapply_cli_overrides(
            &cli,
            &ResolvedPromptOverrides::default(),
            &mut config,
            false,
        );

        assert_eq!(config.permission_mode, claude_core::PermissionMode::Plan);
    }

    #[test]
    fn reapply_cli_overrides_restores_prompt_overrides() {
        let (_tempdir, mut config) = test_config();
        config.system_prompt = Some("restored system".to_owned());
        config.append_system_prompt = Some("restored append".to_owned());

        let cli = crate::cli::Cli::parse_from([
            "remote-code",
            "--system-prompt",
            "inline system",
            "--append-system-prompt",
            "inline append",
            "resume prompt",
        ]);
        let prompt_overrides = ResolvedPromptOverrides {
            system_prompt: Some("inline system".to_owned()),
            append_system_prompt: Some("inline append".to_owned()),
        };
        reapply_cli_overrides(&cli, &prompt_overrides, &mut config, false);

        assert_eq!(config.system_prompt.as_deref(), Some("inline system"));
        assert_eq!(
            config.append_system_prompt.as_deref(),
            Some("inline append")
        );
    }

    #[test]
    fn initialize_conversation_repairs_interrupted_tool_batches() {
        let (_tempdir, config) = test_config();
        let store = SessionStore::open(config.paths.clone()).expect("store");

        let _ = initialize_conversation(&store, &config, Some("repair test"))
            .expect("initial conversation");

        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls.push(ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: serde_json::json!({"path": "src/lib.rs"}),
        });
        store
            .append_conversation_entry(config.session_id, &assistant)
            .expect("append assistant");
        store
            .save_resume_state(
                config.session_id,
                &ResumeState::from_pending_calls(vec![PendingToolCall {
                    id: "call-1".to_owned(),
                    name: "replace_in_file".to_owned(),
                    input: serde_json::json!({"path": "src/lib.rs"}),
                }]),
            )
            .expect("save resume state");

        let repaired = initialize_conversation(&store, &config, Some("repair test"))
            .expect("conversation should repair pending tools");
        let repaired_tool = repaired
            .iter()
            .find(|entry| entry.role == ConversationRole::Tool)
            .expect("synthetic tool result should exist");
        assert_eq!(repaired_tool.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(repaired_tool.name.as_deref(), Some("replace_in_file"));
        assert!(repaired_tool.is_error);
        assert!(repaired_tool.text.contains("interrupted"));

        let resume_state = store
            .load_resume_state(config.session_id)
            .expect("load resume state")
            .expect("resume state should exist");
        assert!(resume_state.pending_tool_calls.is_empty());
    }

    #[test]
    fn has_unanswered_user_prompt_detects_retry_after_interrupted_tool() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls.push(ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: serde_json::json!({"path": "src/lib.rs"}),
        });

        let conversation = vec![
            ConversationEntry::system("system"),
            assistant,
            ConversationEntry::user("retry prompt"),
            ConversationEntry::tool("call-1", "replace_in_file", "interrupted", true),
        ];

        assert!(has_unanswered_user_prompt(&conversation, "retry prompt"));
        assert!(!has_unanswered_user_prompt(
            &conversation,
            "different prompt"
        ));
    }

    #[test]
    fn wizard_target_uses_highest_allowed_scope_and_first_run_gate_is_strict() {
        let (_tempdir, mut config) = test_config();
        config.allowed_setting_sources = vec![SettingSource::Project, SettingSource::Local];

        assert!(should_run_first_run_wizard(&config));
        assert_eq!(
            resolve_first_run_settings_path(&config).expect("target"),
            config.cwd.join(".remote-code").join("settings.local.json")
        );

        config.settings_files = vec![PathBuf::from("visible-settings.json")];
        assert!(!should_run_first_run_wizard(&config));
        config.settings_files.clear();

        config.provider.api_key = Some("env-key".to_owned());
        assert!(!should_run_first_run_wizard(&config));
    }

    #[test]
    fn discover_runtime_extensions_respects_setting_sources() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plugin_root = profile.join("plugins").join("sample");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace dir");
        fs::create_dir_all(profile.join("skills").join("demo")).expect("profile skills");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
        fs::write(
            profile.join("skills").join("demo").join("SKILL.md"),
            "# Demo\n\nSummary.\n",
        )
        .expect("write skill");
        fs::write(
            profile.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            r#"[servers.profile]
command = "python"
args = ["profile.py"]

[servers.disabled]
command = "python"
args = ["disabled.py"]
enabled = false"#,
        )
        .expect("write profile mcp");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            r#"[servers.project]
command = "python"
args = ["project.py"]"#,
        )
        .expect("write project mcp");
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "sample",
                "version": "0.1.0",
                "skills": "./skills",
                "mcp": "./mcp.toml",
                "runtime": {
                    "command": "python",
                    "cwd": "."
                }
            }"#,
        )
        .expect("write plugin manifest");
        fs::create_dir_all(plugin_root.join("skills").join("bundled")).expect("plugin skills");
        fs::write(
            plugin_root.join("skills").join("bundled").join("SKILL.md"),
            "# Bundled\n\nSummary.\n",
        )
        .expect("write plugin skill");
        fs::write(
            plugin_root.join("mcp.toml"),
            r#"[servers.plugin]
command = "python"
args = ["plugin.py"]"#,
        )
        .expect("write plugin mcp");

        let project_only = load_runtime_config(
            Some(cwd.clone()),
            Some(profile.clone()),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides {
                allowed_setting_sources: Some(vec![SettingSource::Project]),
                ..RuntimeOverrides::default()
            },
        )
        .expect("project config");
        let discovery = discover_runtime_extensions(&project_only);
        assert!(discovery.skills.is_empty());
        assert!(discovery.plugins.is_empty());
        assert!(discovery.plugin_runtimes.is_empty());
        assert_eq!(discovery.mcp_servers, vec!["project".to_owned()]);
        assert!(discovery.disabled_mcp_servers.is_empty());

        let user_only = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides {
                allowed_setting_sources: Some(vec![SettingSource::User]),
                ..RuntimeOverrides::default()
            },
        )
        .expect("user config");
        let discovery = discover_runtime_extensions(&user_only);
        assert_eq!(
            discovery.skills,
            vec!["bundled".to_owned(), "demo".to_owned()]
        );
        assert_eq!(discovery.plugins, vec!["sample".to_owned()]);
        assert_eq!(discovery.plugin_runtimes, vec!["sample".to_owned()]);
        assert_eq!(
            discovery.mcp_servers,
            vec!["plugin".to_owned(), "profile".to_owned()]
        );
        assert_eq!(discovery.disabled_mcp_servers, vec!["disabled".to_owned()]);
    }
}
