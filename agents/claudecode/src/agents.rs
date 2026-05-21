use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use claude_agents::builtins::{
    explore_agent, general_purpose_agent, plan_agent, verification_agent,
};
use claude_agents::constants::FORK_SUBAGENT_TYPE;
use claude_agents::loader::load_all_agents;
use claude_agents::{
    AgentDefinition, AgentExecutionRequest, AgentExecutor, AgentIdentity, AgentRunConfig,
    AgentRunResult, AgentRunner, AgentScheduler, AgentSource, AgentTask,
};
use claude_config::{RuntimeConfig, restamp_runtime_session};
use claude_core::{
    ConversationEntry as CoreConversationEntry, ConversationRole, ProviderProtocol,
    ProviderResponse, SubAgentCompletion, SubAgentExecutionRequest, SubAgentExecutionResult,
};
use claude_model::model::{ResolveContext, parse_user_specified_model_with_ctx};
use claude_model::{
    ModelProvider, detect_provider, is_first_party_base_url, is_model_alias, provider_model_id,
};
use claude_provider::{ProviderClient, ProviderCompatBackend};
use claude_query_engine::QuerySource;
use claude_session::SessionStore;
use claude_tools::{
    ToolSpec,
    runtime_plan_mode::{
        build_runtime_plan_mode, copy_plan_mode_state_for_fork, install_plan_mode_runtime,
    },
    runtime_provider_tool_specs,
};
use tempfile::TempDir;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::cli::{AgentsCommand, AgentsPlanArgs};
use crate::conversation::{
    discover_runtime_extensions, initialize_conversation, restore_discovered_tool_scope,
};
use crate::hooks::{HookRunState, discover_runtime_hooks, ensure_session_start_hooks};
use crate::query_engine_compat::{
    CompatExecutionOptions, CompatRunOverrides, ForkCacheSafeParams, run_no_persist_forked_query,
    run_prompt_with_query_engine_compat_overrides,
};

pub(crate) fn parse_agent_spec(spec: &str) -> Result<AgentIdentity> {
    let mut segments = spec.splitn(4, ';').map(str::trim);
    let name = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("invalid --agent spec `{spec}`; expected name;role;paths;labels"))?;
    let role = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("invalid --agent spec `{spec}`; role is missing"))?;
    let mut agent = AgentIdentity::new(name, role);
    agent.ownership_paths = segments.next().map(parse_csv_list).unwrap_or_default();
    agent.labels = segments
        .next()
        .map(parse_key_value_pairs)
        .unwrap_or_default();
    Ok(agent)
}

pub(crate) fn parse_task_spec(spec: &str) -> Result<AgentTask> {
    let mut segments = spec.splitn(4, ';').map(str::trim);
    let title = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow!("invalid --task spec `{spec}`; expected title;paths;labels;description")
        })?;
    let mut task = AgentTask::new(title);
    task.ownership_paths = segments.next().map(parse_csv_list).unwrap_or_default();
    task.required_labels = segments
        .next()
        .map(parse_key_value_pairs)
        .unwrap_or_default();
    segments
        .next()
        .unwrap_or_default()
        .clone_into(&mut task.description);
    task.budget.read_calls = 32;
    task.budget.edit_calls = 12;
    task.budget.command_calls = 8;
    Ok(task)
}

fn default_agent_specs_for_workspace(workspace_scope: &str) -> Vec<AgentIdentity> {
    let workspace_spec = format!("workspace;implementer;{workspace_scope};phase=workspace");
    vec![
        parse_agent_spec("planner;planner;;phase=plan").unwrap_or_else(|_| {
            let mut agent = AgentIdentity::new("planner", "planner");
            agent.labels.insert("phase".to_owned(), "plan".to_owned());
            agent
        }),
        parse_agent_spec(&workspace_spec).unwrap_or_else(|_| {
            let mut agent = AgentIdentity::new("workspace", "implementer");
            agent.ownership_paths = vec![workspace_scope.to_owned()];
            agent
        }),
        parse_agent_spec(
            "runtime;implementer;apps/remote-code,crates/rc-session,crates/rc-tools;phase=local",
        )
        .unwrap_or_else(|_| AgentIdentity::new("runtime", "implementer")),
        parse_agent_spec(
            "remote;implementer;apps/remote-code-runner,apps/remote-code-control-plane,crates/rc-runner,crates/rc-control-plane;phase=remote",
        )
        .unwrap_or_else(|_| AgentIdentity::new("remote", "implementer")),
        parse_agent_spec("review;reviewer;.;phase=review")
            .unwrap_or_else(|_| AgentIdentity::new("review", "reviewer")),
    ]
}

pub(crate) fn default_agent_specs(config: &RuntimeConfig) -> Vec<AgentIdentity> {
    default_agent_specs_for_workspace(&config.cwd.display().to_string())
}

pub(crate) fn default_task_for_objective(objective: &str, config: &RuntimeConfig) -> AgentTask {
    let mut task = AgentTask::new(objective);
    task.description = format!(
        "Coordinate work for {} in {}",
        objective,
        config.cwd.display()
    );
    task.ownership_paths = vec![config.cwd.display().to_string()];
    task.budget.read_calls = 64;
    task.budget.edit_calls = 16;
    task.budget.command_calls = 12;
    task
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_key_value_pairs(value: &str) -> BTreeMap<String, String> {
    value
        .split(',')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                None
            } else {
                Some((key.to_owned(), value.to_owned()))
            }
        })
        .collect()
}

#[derive(Clone)]
struct RemoteCodeAgentExecutor {
    base_config: RuntimeConfig,
}

impl RemoteCodeAgentExecutor {
    fn new(config: &RuntimeConfig) -> Self {
        Self {
            base_config: config.clone(),
        }
    }
}

#[derive(Clone)]
struct RemoteCodeSubAgentRuntime {
    completion: Arc<dyn SubAgentCompletion>,
    executor: RemoteCodeAgentExecutor,
    read_file_state: claude_tools::FileStateCache,
}

impl RemoteCodeSubAgentRuntime {
    fn new(
        config: &RuntimeConfig,
        completion: Arc<dyn SubAgentCompletion>,
        read_file_state: claude_tools::FileStateCache,
    ) -> Self {
        Self {
            completion,
            executor: RemoteCodeAgentExecutor::new(config),
            read_file_state,
        }
    }
}

#[async_trait]
impl SubAgentCompletion for RemoteCodeSubAgentRuntime {
    async fn complete(&self, conversation: &[CoreConversationEntry]) -> Result<ProviderResponse> {
        self.completion.complete(conversation).await
    }

    fn supports_agent_execution(&self) -> bool {
        true
    }

    async fn execute_agent(
        &self,
        request: SubAgentExecutionRequest,
    ) -> Result<SubAgentExecutionResult> {
        if request.fork_snapshot.is_some() {
            return self
                .executor
                .execute_fork(request, self.read_file_state.clone_isolated())
                .await;
        }
        let provider_model =
            resolve_requested_agent_model(&self.executor.base_config, request.model.as_deref());
        let result = self
            .executor
            .execute(AgentExecutionRequest {
                agent_type: request.agent_type,
                agent_name: request.agent_name,
                team_name: request.team_name,
                task: request.task,
                context: request
                    .context
                    .iter()
                    .map(core_entry_to_agent_context_entry)
                    .collect(),
                model: provider_model.unwrap_or_else(|| "default".to_owned()),
                max_turns: request.max_turns,
                system_prompt: request.system_prompt.unwrap_or_default(),
                critical_system_reminder: request.critical_system_reminder,
                omit_claude_md: request.omit_claude_md,
                omit_git_status: request.omit_git_status,
                tools: request.allowed_tools,
                permission_mode: request.permission_mode,
                working_dir: request.working_dir,
                additional_working_directories: request.additional_working_directories,
                skip_transcript: request.skip_transcript,
            })
            .await?;
        Ok(SubAgentExecutionResult {
            output: result.output,
            success: result.success,
            turns: result.turns,
            usage: claude_core::UsageSummary {
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
                cache_read_input_tokens: result.usage.cache_read_tokens,
                cache_creation_input_tokens: result.usage.cache_creation_tokens,
                ..Default::default()
            },
        })
    }
}

pub(crate) fn build_remote_code_sub_agent_runtime(
    config: &RuntimeConfig,
    completion: Arc<dyn SubAgentCompletion>,
    read_file_state: claude_tools::FileStateCache,
) -> Arc<dyn SubAgentCompletion> {
    Arc::new(RemoteCodeSubAgentRuntime::new(
        config,
        completion,
        read_file_state,
    ))
}

#[async_trait]
impl AgentExecutor for RemoteCodeAgentExecutor {
    async fn execute(&self, request: AgentExecutionRequest) -> Result<AgentRunResult> {
        let mut config = self.base_config.clone();
        config.cwd = request.working_dir.clone();
        let parent_session_id = self.base_config.session_id;
        restamp_runtime_session(&mut config, Uuid::new_v4());
        let _ephemeral_session = if request.skip_transcript {
            Some(install_ephemeral_session_paths(&mut config)?)
        } else {
            None
        };
        config.max_turns = usize::try_from(request.max_turns).unwrap_or(usize::MAX);
        if let Some(mode) = request.permission_mode {
            config.permission_mode = mode;
        }
        config.session_name = Some(format!(
            "agent:{}:{}",
            request
                .agent_name
                .as_deref()
                .unwrap_or(request.agent_type.as_str()),
            truncate_single_line(&request.task, 48)
        ));
        if !request.model.is_empty() && request.model != "default" {
            config.provider.model = Some(request.model.clone());
        }

        let store = SessionStore::open(config.paths.clone())?;
        store.ensure_session_with_parent(
            config.session_id,
            &config.cwd,
            &config.provider.name,
            config.provider.model.as_deref(),
            config.session_name.as_deref(),
            Some(parent_session_id),
        )?;
        let should_copy_plan_state = request.agent_type.eq_ignore_ascii_case(FORK_SUBAGENT_TYPE)
            || request.permission_mode == Some(claude_core::PermissionMode::Plan)
            || self.base_config.permission_mode == claude_core::PermissionMode::Plan;
        if should_copy_plan_state && !request.skip_transcript {
            let _copied = copy_plan_mode_state_for_fork(
                &store,
                &config.paths,
                parent_session_id,
                config.session_id,
            )?;
        }
        let backend =
            ProviderCompatBackend::new(Arc::new(ProviderClient::new()?), &config.provider);
        let discovered_tool_scope = backend.discovered_tool_scope();
        let (plan_mode_controller, broker) = build_runtime_plan_mode(&config, &store)?;
        let _plan_mode_runtime = install_plan_mode_runtime(plan_mode_controller)?;
        let discovery = discover_runtime_hooks(&config, &[]);
        let mut conversation = initialize_conversation(&store, &config, Some(&request.task))?;
        restore_discovered_tool_scope(&store, config.session_id, &discovered_tool_scope)?;
        append_conversation_context(&mut conversation, &request.context);
        let mut hook_state = HookRunState::load(&store, config.session_id)?;
        ensure_session_start_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &mut hook_state,
        )
        .await?;
        let agent_system_prompt = resolve_runtime_agent_system_prompt(&config, &request);

        let outcome = run_prompt_with_query_engine_compat_overrides(
            &config,
            &store,
            Arc::new(backend),
            discovered_tool_scope,
            broker,
            None,
            &discovery,
            &mut hook_state,
            &mut conversation,
            &request.task,
            CompatRunOverrides {
                agent_system_prompt: Some(agent_system_prompt),
                allowed_tools: (!request.tools.is_empty()).then_some(request.tools),
                critical_system_reminder: request.critical_system_reminder,
                omit_claude_md: request.omit_claude_md,
                omit_git_status: request.omit_git_status,
                ..CompatRunOverrides::default()
            },
            CompatExecutionOptions {
                query_source: QuerySource::Agent,
                agent_id: request
                    .agent_name
                    .as_deref()
                    .or(Some(request.agent_type.as_str()))
                    .map(claude_core::AgentId::from),
                fork_snapshot: None,
                ..CompatExecutionOptions::default()
            },
        )
        .await?;

        Ok(AgentRunResult {
            output: outcome.text,
            success: true,
            turns: outcome.num_turns,
            usage: claude_agents::UsageSummary {
                input_tokens: outcome.usage.input_tokens,
                output_tokens: outcome.usage.output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

impl RemoteCodeAgentExecutor {
    async fn execute_fork(
        &self,
        request: SubAgentExecutionRequest,
        read_file_state: claude_tools::FileStateCache,
    ) -> Result<SubAgentExecutionResult> {
        let fork_snapshot = request
            .fork_snapshot
            .ok_or_else(|| anyhow!("fork execution requires a fork snapshot"))?;
        let mut config = self.base_config.clone();
        config.cwd = request.working_dir.clone();
        config.max_turns = usize::try_from(request.max_turns).unwrap_or(usize::MAX);
        if let Some(mode) = request.permission_mode {
            config.permission_mode = mode;
        }
        if let Some(model) =
            resolve_requested_agent_model(&self.base_config, request.model.as_deref())
        {
            config.provider.model = Some(model);
        }

        let store = SessionStore::open(config.paths.clone())?;
        let backend =
            ProviderCompatBackend::new(Arc::new(ProviderClient::new()?), &config.provider);
        let discovered_tool_scope = backend.discovered_tool_scope();
        restore_discovered_tool_scope(&store, self.base_config.session_id, &discovered_tool_scope)?;
        let (plan_mode_controller, broker) = build_runtime_plan_mode(&config, &store)?;
        let _plan_mode_runtime = install_plan_mode_runtime(plan_mode_controller)?;
        let discovery = discover_runtime_hooks(&config, &[]);
        let mut hook_state = HookRunState::load(&store, self.base_config.session_id)?;
        let fork_cache = ForkCacheSafeParams {
            fork_context_messages: fork_snapshot.fork_context_messages,
            system_prompt: fork_snapshot.system_prompt,
            user_context: fork_snapshot.user_context,
            system_context: fork_snapshot.system_context,
            read_file_state: Some(read_file_state),
        };
        let tool_results_dir = config
            .paths
            .sessions_dir
            .join(self.base_config.session_id.to_string())
            .join("fork-tool-results");
        let outcome = run_no_persist_forked_query(
            &config,
            &store,
            Arc::new(backend),
            discovered_tool_scope,
            broker,
            &discovery,
            &mut hook_state,
            fork_cache,
            &request.task,
            CompatRunOverrides {
                allowed_tools: (!request.allowed_tools.is_empty()).then_some(request.allowed_tools),
                critical_system_reminder: request.critical_system_reminder,
                omit_claude_md: request.omit_claude_md,
                omit_git_status: request.omit_git_status,
                ..CompatRunOverrides::default()
            },
            QuerySource::Agent,
            Some(request.max_turns),
            tool_results_dir,
        )
        .await?;

        let output = outcome
            .messages
            .iter()
            .rev()
            .find(|entry| {
                entry.role == ConversationRole::Assistant && !entry.text.trim().is_empty()
            })
            .map(|entry| entry.text.clone())
            .unwrap_or_default();
        Ok(SubAgentExecutionResult {
            output,
            success: true,
            turns: outcome.num_turns,
            usage: claude_core::UsageSummary {
                input_tokens: outcome.usage.input_tokens,
                output_tokens: outcome.usage.output_tokens,
                cache_read_input_tokens: outcome.cache_read_input_tokens,
                cache_creation_input_tokens: outcome.cache_creation_input_tokens,
                ..Default::default()
            },
        })
    }
}

pub(crate) fn install_ephemeral_session_paths(config: &mut RuntimeConfig) -> Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("remote-code-agent-ephemeral-");
    let tempdir = match std::env::var_os("CLAUDE_CODE_TMPDIR") {
        Some(tmpdir) => {
            let tmpdir = PathBuf::from(tmpdir);
            fs::create_dir_all(&tmpdir)
                .with_context(|| format!("failed to create {}", tmpdir.display()))?;
            builder
                .tempdir_in(tmpdir)
                .context("failed to create ephemeral agent session directory")?
        }
        None => builder
            .tempdir()
            .context("failed to create ephemeral agent session directory")?,
    };
    let profile_dir = tempdir.path().join("profile");
    config.paths.state_db_path = profile_dir.join("state.db");
    config.paths.sessions_dir = profile_dir.join("sessions");
    config.paths.artifacts_dir = profile_dir.join("artifacts");
    Ok(tempdir)
}

fn resolve_requested_agent_model(
    config: &RuntimeConfig,
    requested_model: Option<&str>,
) -> Option<String> {
    let requested_model = requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if requested_model.eq_ignore_ascii_case("inherit") {
        return None;
    }
    if !is_model_alias(requested_model) {
        return Some(requested_model.to_owned());
    }

    let current_model = config.provider.model.as_deref().unwrap_or_default();
    let current_model_is_claude = current_model.to_ascii_lowercase().contains("claude");
    let base_url = config.provider.base_url.as_deref();
    let can_resolve_alias = current_model_is_claude
        || matches!(
            config.provider.protocol,
            ProviderProtocol::Bedrock | ProviderProtocol::Vertex
        )
        || base_url.is_some_and(is_first_party_base_url);

    if !can_resolve_alias {
        tracing::info!(
            requested_model,
            current_model,
            "Ignoring agent model alias for a non-Claude-compatible runtime and inheriting the parent model"
        );
        return None;
    }

    let provider = detect_model_provider(config, base_url);
    let ctx = ResolveContext {
        provider: provider.clone(),
        ..Default::default()
    };
    let resolved = parse_user_specified_model_with_ctx(requested_model, &ctx);
    Some(provider_model_id(&resolved, &provider))
}

fn detect_model_provider(config: &RuntimeConfig, base_url: Option<&str>) -> ModelProvider {
    match config.provider.protocol {
        ProviderProtocol::Anthropic => {
            if let Some(base_url) = base_url
                && !is_first_party_base_url(base_url)
            {
                return ModelProvider::OpenAiCompatible {
                    base_url: base_url.to_owned(),
                };
            }
            ModelProvider::Anthropic
        }
        ProviderProtocol::Bedrock => ModelProvider::AwsBedrock { region: None },
        ProviderProtocol::Vertex => ModelProvider::GcpVertex { project: None },
        ProviderProtocol::OpenAi => detect_provider(&claude_model::ProviderConfig {
            openai_base_url: config.provider.base_url.clone(),
            provider: Some("openai_compatible".to_owned()),
            ..Default::default()
        }),
    }
}

const CLAUDE_CODE_GUIDE_AGENT_TYPE: &str = "claude-code-guide";

fn resolve_runtime_agent_system_prompt(
    config: &RuntimeConfig,
    request: &AgentExecutionRequest,
) -> String {
    let base_prompt = if request.agent_type == CLAUDE_CODE_GUIDE_AGENT_TYPE {
        build_claude_code_guide_runtime_prompt(config, &request.system_prompt)
    } else {
        request.system_prompt.clone()
    };

    if request.agent_type != CLAUDE_CODE_GUIDE_AGENT_TYPE {
        return base_prompt;
    }

    base_prompt
}

fn build_claude_code_guide_runtime_prompt(config: &RuntimeConfig, base_prompt: &str) -> String {
    let context_sections = build_claude_code_guide_context_sections(config);
    if context_sections.is_empty() {
        return base_prompt.to_owned();
    }

    format!(
        "{base_prompt}\n\n---\n\n# User's Current Configuration\n\nThe user has the following custom setup in their environment:\n\n{}\n\nWhen answering questions, consider these configured features and proactively suggest them when relevant.",
        context_sections.join("\n\n")
    )
}

fn build_claude_code_guide_context_sections(config: &RuntimeConfig) -> Vec<String> {
    let mut sections = Vec::new();

    let custom_skills = discover_guide_custom_skills(config);
    if !custom_skills.is_empty() {
        sections.push(format!(
            "**Available custom skills in this project:**\n{}",
            render_bulleted_entries(
                &custom_skills
                    .into_iter()
                    .map(|(name, description)| format!("/{name}: {description}"))
                    .collect::<Vec<_>>()
            )
        ));
    }

    let custom_agents = discover_guide_custom_agents(config);
    if !custom_agents.is_empty() {
        sections.push(format!(
            "**Available custom agents configured:**\n{}",
            render_bulleted_entries(
                &custom_agents
                    .into_iter()
                    .map(|(name, description)| format!("{name}: {description}"))
                    .collect::<Vec<_>>()
            )
        ));
    }

    let mcp_servers = discover_runtime_extensions(config).mcp_servers;
    if !mcp_servers.is_empty() {
        sections.push(format!(
            "**Configured MCP servers:**\n{}",
            render_bulleted_entries(&mcp_servers)
        ));
    }

    let plugin_skills = discover_guide_plugin_skills(config);
    if !plugin_skills.is_empty() {
        sections.push(format!(
            "**Available plugin skills:**\n{}",
            render_bulleted_entries(
                &plugin_skills
                    .into_iter()
                    .map(|(name, description)| format!("/{name}: {description}"))
                    .collect::<Vec<_>>()
            )
        ));
    }

    if let Some(settings_json) = merged_runtime_settings_json(config) {
        sections.push(format!(
            "**User's settings.json:**\n```json\n{settings_json}\n```"
        ));
    }

    sections
}

fn render_bulleted_entries(entries: &[String]) -> String {
    entries
        .iter()
        .map(|entry| format!("- {entry}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn discover_guide_custom_skills(config: &RuntimeConfig) -> Vec<(String, String)> {
    let mut skills = BTreeMap::new();
    for root in guide_skill_roots(config) {
        collect_skill_entries(&root, &mut skills);
    }
    skills.into_iter().collect()
}

fn guide_skill_roots(config: &RuntimeConfig) -> Vec<PathBuf> {
    vec![
        config.paths.skills_dir.clone(),
        config.cwd.join(".claude").join("skills"),
        config.cwd.join(".remote-code").join("skills"),
    ]
}

fn collect_skill_entries(root: &Path, skills: &mut BTreeMap<String, String>) {
    if !root.exists() {
        return;
    }

    match claude_skills::discover_skills(root) {
        Ok(discovered) => {
            for skill in discovered {
                let description = skill
                    .metadata
                    .summary
                    .clone()
                    .unwrap_or_else(|| skill.metadata.title.clone());
                skills.insert(skill.metadata.slug.clone(), description);
            }
        }
        Err(error) => tracing::warn!(
            path = %root.display(),
            "failed to discover guide custom skills: {error}"
        ),
    }
}

fn discover_guide_custom_agents(config: &RuntimeConfig) -> Vec<(String, String)> {
    let user_agents_dir = config.paths.profile_dir.join("agents");
    let project_agents_dir = config.cwd.join(".claude").join("agents");
    load_all_agents(
        Some(user_agents_dir.as_path()),
        Some(project_agents_dir.as_path()),
    )
    .active_agents
    .into_iter()
    .filter(|agent| agent.source != AgentSource::BuiltIn)
    .map(|agent| (agent.agent_type, agent.when_to_use))
    .collect()
}

fn discover_guide_plugin_skills(config: &RuntimeConfig) -> Vec<(String, String)> {
    let mut skills = BTreeMap::new();
    match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
        Ok(plugins) => {
            for plugin in plugins {
                match plugin.discover_bundled_skills() {
                    Ok(discovered) => {
                        for skill in discovered {
                            let description = skill
                                .metadata
                                .summary
                                .clone()
                                .unwrap_or_else(|| skill.metadata.title.clone());
                            skills.insert(skill.metadata.slug.clone(), description);
                        }
                    }
                    Err(error) => tracing::warn!(
                        plugin = %plugin.manifest.name,
                        "failed to discover bundled plugin skills: {error}"
                    ),
                }
            }
        }
        Err(error) => tracing::warn!("failed to discover plugins for guide agent prompt: {error}"),
    }
    skills.into_iter().collect()
}

fn merged_runtime_settings_json(config: &RuntimeConfig) -> Option<String> {
    let mut merged = serde_json::Value::Object(serde_json::Map::new());
    let mut saw_settings = false;

    for path in &config.settings_files {
        let Some(value) = parse_settings_document(path) else {
            continue;
        };
        merge_json_value(&mut merged, value);
        saw_settings = true;
    }

    if !saw_settings || merged.as_object().is_some_and(|object| object.is_empty()) {
        return None;
    }

    serde_json::to_string_pretty(&merged).ok()
}

fn parse_settings_document(path: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    let parsed = match extension {
        "json" => serde_json::from_str(&raw).ok(),
        "toml" => toml::from_str::<toml::Value>(&raw)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
        _ => serde_json::from_str(&raw).ok().or_else(|| {
            toml::from_str::<toml::Value>(&raw)
                .ok()
                .and_then(|value| serde_json::to_value(value).ok())
        }),
    };

    if parsed.is_none() {
        tracing::warn!(path = %path.display(), "failed to parse settings document for guide agent prompt");
    }

    parsed
}

fn merge_json_value(target: &mut serde_json::Value, source: serde_json::Value) {
    match (target, source) {
        (serde_json::Value::Object(target), serde_json::Value::Object(source)) => {
            for (key, value) in source {
                if let Some(existing) = target.get_mut(&key) {
                    merge_json_value(existing, value);
                } else {
                    target.insert(key, value);
                }
            }
        }
        (target, source) => *target = source,
    }
}

fn core_entry_to_agent_context_entry(
    entry: &CoreConversationEntry,
) -> claude_agents::ConversationEntry {
    let role = match entry.role {
        ConversationRole::System => "system",
        ConversationRole::Assistant => "assistant",
        ConversationRole::User => "user",
        ConversationRole::Tool => "tool",
    };
    claude_agents::ConversationEntry {
        role: role.to_owned(),
        content: entry.text.clone(),
    }
}

struct ScheduledAgentRun {
    task_id: Uuid,
    agent: AgentIdentity,
    task: AgentTask,
    runner: AgentRunner,
    prompt: String,
}

pub(crate) async fn run_agents(config: &RuntimeConfig, command: AgentsCommand) -> Result<()> {
    match command {
        AgentsCommand::Plan(args) => run_agents_plan(config, &args).await,
    }
}

pub(crate) async fn run_agents_plan(config: &RuntimeConfig, args: &AgentsPlanArgs) -> Result<()> {
    let mut scheduler = AgentScheduler::new(args.lead.clone(), args.objective.clone());
    let agents = if args.agents.is_empty() {
        default_agent_specs(config)
    } else {
        args.agents
            .iter()
            .map(|spec| parse_agent_spec(spec))
            .collect::<Result<Vec<_>>>()?
    };
    for agent in agents {
        scheduler.register_agent(agent);
    }

    let tasks = if args.tasks.is_empty() {
        vec![default_task_for_objective(&args.objective, config)]
    } else {
        args.tasks
            .iter()
            .map(|spec| parse_task_spec(spec))
            .collect::<Result<Vec<_>>>()?
    };
    for task in tasks {
        scheduler.add_task(task);
    }

    let available_tools = available_runtime_agent_tools().await;
    let mut scheduled_runs = Vec::new();

    while let Some((task_id, agent_id)) = scheduler.assign_next_task() {
        let agent = scheduler
            .agents()
            .into_iter()
            .find(|candidate| candidate.agent_id == agent_id)
            .ok_or_else(|| anyhow!("assigned agent {agent_id} was not found"))?;
        let task = scheduler
            .tasks()
            .into_iter()
            .find(|candidate| candidate.id == task_id)
            .ok_or_else(|| anyhow!("assigned task {task_id} was not found"))?;

        let _ = scheduler.queue_instruction(
            agent_id,
            args.lead.clone(),
            format!("Task: {}", task.title),
            format!(
                "Objective: {}\nTask: {}\nOwnership: {}",
                args.objective,
                task.title,
                if task.ownership_paths.is_empty() {
                    "(unscoped)".to_owned()
                } else {
                    task.ownership_paths.join(", ")
                }
            ),
        );

        let mailbox = scheduler.drain_mailbox(agent_id);
        let definition = agent_definition_for_identity(&agent);
        let prompt = build_task_prompt(&args.objective, &agent, &task, &mailbox, &definition);
        let runner = AgentRunner::new(
            definition,
            AgentRunConfig {
                max_turns: 0,
                model: String::new(),
                tools: available_tools.clone(),
                system_prompt: None,
                working_dir: config.cwd.clone(),
                additional_working_directories: Vec::new(),
            },
        );
        let _ = scheduler.start_task(task_id);
        scheduled_runs.push(ScheduledAgentRun {
            task_id,
            agent,
            task,
            runner,
            prompt,
        });
    }

    let executor = Arc::new(RemoteCodeAgentExecutor::new(config));
    let mut join_set = JoinSet::new();

    for scheduled in scheduled_runs {
        let executor = Arc::clone(&executor);
        join_set.spawn(async move {
            let result = scheduled
                .runner
                .run_with_executor(&scheduled.prompt, &[], executor.as_ref())
                .await;
            (scheduled, result)
        });
    }

    while let Some(joined) = join_set.join_next().await {
        let (scheduled, result) =
            joined.map_err(|error| anyhow!("agent execution task panicked: {error}"))?;
        match result {
            Ok(run_result) if run_result.success => {
                let summary = truncate_single_line(&run_result.output, 160);
                let _ = scheduler.complete_task(scheduled.task_id, summary.clone());
                if !args.json {
                    println!(
                        "Completed `{}` by {} ({}) in {} turn(s)",
                        scheduled.task.title,
                        scheduled.agent.name,
                        scheduled.runner.definition().agent_type,
                        run_result.turns
                    );
                    if !summary.is_empty() {
                        println!("  {}", summary);
                    }
                }
            }
            Ok(run_result) => {
                let message = truncate_single_line(&run_result.output, 160);
                let _ = scheduler.fail_task(scheduled.task_id, message.clone());
                if !args.json {
                    println!(
                        "Failed `{}` by {} ({})",
                        scheduled.task.title,
                        scheduled.agent.name,
                        scheduled.runner.definition().agent_type
                    );
                    if !message.is_empty() {
                        println!("  {}", message);
                    }
                }
            }
            Err(error) => {
                let _ = scheduler.fail_task(scheduled.task_id, error.to_string());
                if !args.json {
                    println!(
                        "Error `{}` by {} ({})",
                        scheduled.task.title,
                        scheduled.agent.name,
                        scheduled.runner.definition().agent_type
                    );
                    println!("  {}", error);
                }
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&scheduler.snapshot())?);
    } else {
        let summary = scheduler.summary();
        println!(
            "\nTeam {}: {} agent(s), {} task(s), {} completed, {} failed, {} pending message(s)",
            summary.team_id,
            summary.total_agents,
            summary.total_tasks,
            summary.completed_tasks,
            summary.failed_tasks,
            summary.pending_messages
        );
    }
    Ok(())
}

fn append_conversation_context(
    conversation: &mut Vec<CoreConversationEntry>,
    context: &[claude_agents::ConversationEntry],
) {
    conversation.extend(context.iter().map(
        |entry| match entry.role.to_ascii_lowercase().as_str() {
            "system" => CoreConversationEntry::system(entry.content.clone()),
            "assistant" => CoreConversationEntry::assistant(entry.content.clone()),
            "tool" => CoreConversationEntry::user(format!("[tool context]\n{}", entry.content)),
            _ => CoreConversationEntry {
                uuid: Uuid::new_v4(),
                role: ConversationRole::User,
                text: entry.content.clone(),
                history_text: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                attachments: Vec::new(),
                tool_call_id: None,
                name: None,
                is_error: false,
            },
        },
    ));
}

fn agent_definition_for_identity(agent: &AgentIdentity) -> AgentDefinition {
    let role = agent.role.to_ascii_lowercase();
    if role.contains("plan")
        || agent
            .labels
            .get("phase")
            .is_some_and(|phase| phase == "plan")
    {
        plan_agent()
    } else if role.contains("review")
        || role.contains("verify")
        || agent
            .labels
            .get("phase")
            .is_some_and(|phase| phase == "review")
    {
        verification_agent()
    } else if role.contains("explore")
        || role.contains("research")
        || agent.name.eq_ignore_ascii_case("explore")
    {
        explore_agent()
    } else {
        general_purpose_agent()
    }
}

fn build_task_prompt(
    objective: &str,
    agent: &AgentIdentity,
    task: &AgentTask,
    mailbox: &[claude_agents::AgentMailboxMessage],
    definition: &AgentDefinition,
) -> String {
    let mut sections = vec![
        format!("You are assigned to advance this objective:\n{objective}"),
        format!(
            "Assigned agent: {} ({}) using {}",
            agent.name, agent.role, definition.agent_type
        ),
        format!("Task title: {}", task.title),
    ];

    if !task.description.trim().is_empty() {
        sections.push(format!("Task description:\n{}", task.description));
    }
    if !task.ownership_paths.is_empty() {
        sections.push(format!(
            "Primary ownership paths:\n{}",
            task.ownership_paths.join("\n")
        ));
    }
    if !task.required_labels.is_empty() {
        sections.push(format!(
            "Required labels:\n{}",
            task.required_labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !mailbox.is_empty() {
        sections.push(format!(
            "Coordinator messages:\n{}",
            mailbox
                .iter()
                .map(|message| format!("{}:\n{}", message.subject, message.body))
                .collect::<Vec<_>>()
                .join("\n\n")
        ));
    }

    let completion = match definition.agent_type.as_str() {
        "Plan" | "Explore" => {
            "This is a read-only assignment. Do not modify files. Investigate the codebase thoroughly and return a concrete implementation-oriented report."
        }
        "verification" => {
            "Independently verify the current state. Prefer finding concrete defects, regressions, or missing tests. Only change files if strictly necessary to complete verification."
        }
        _ => {
            "Implement the requested work directly in the workspace, use tools as needed, run relevant validation where practical, and finish with a concise summary of changes and verification."
        }
    };
    sections.push(format!("Completion expectations:\n{completion}"));

    sections.join("\n\n")
}

fn insert_runtime_tool_aliases(spec: &ToolSpec, tools: &mut BTreeSet<String>) {
    tools.insert(spec.name.clone());
    tools.insert(spec.protocol_name.clone());
    match spec.name.as_str() {
        "read_file" => {
            tools.insert("Read".to_owned());
        }
        "write_file" => {
            tools.insert("Write".to_owned());
        }
        "edit_file" | "replace_in_file" => {
            tools.insert("Edit".to_owned());
        }
        "bash_command" => {
            tools.insert("Bash".to_owned());
        }
        "glob" => {
            tools.insert("Glob".to_owned());
        }
        "grep" => {
            tools.insert("Grep".to_owned());
        }
        "ask_user" => {
            tools.insert("AskUserQuestion".to_owned());
        }
        "agent" => {
            tools.insert("Agent".to_owned());
        }
        "task_create" => {
            tools.insert("Task".to_owned());
            tools.insert("TaskCreate".to_owned());
        }
        "todo_write" => {
            tools.insert("TodoWrite".to_owned());
        }
        "send_message" => {
            tools.insert("SendMessage".to_owned());
        }
        "skill_execute" | "discover_skills" => {
            tools.insert("Skill".to_owned());
        }
        "sleep" => {
            tools.insert("Sleep".to_owned());
        }
        _ => {}
    }
}

async fn available_runtime_agent_tools() -> Vec<String> {
    let mut tools = BTreeSet::new();
    for spec in runtime_provider_tool_specs().await {
        insert_runtime_tool_aliases(&spec, &mut tools);
    }
    tools.into_iter().collect()
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single_line = text.lines().next().unwrap_or_default().trim();
    if single_line.chars().count() <= max_chars {
        return single_line.to_owned();
    }

    let truncated = single_line
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_owned();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use tempfile::tempdir;

    #[test]
    fn default_agent_specs_include_workspace_owner() {
        let agents = default_agent_specs_for_workspace(r"C:\work\sample-project");
        let workspace_agent = agents
            .iter()
            .find(|agent| agent.name == "workspace")
            .expect("workspace agent should be present");
        assert_eq!(
            workspace_agent.ownership_paths,
            vec![r"C:\work\sample-project".to_owned()]
        );
    }

    #[test]
    fn default_team_assigns_current_workspace_task() {
        let workspace = r"C:\work\sample-project";
        let mut scheduler = AgentScheduler::new("lead", "Inspect workspace");
        for agent in default_agent_specs_for_workspace(workspace) {
            scheduler.register_agent(agent);
        }

        let mut task = AgentTask::new("Inspect workspace");
        task.ownership_paths = vec![workspace.to_owned()];
        scheduler.add_task(task);

        let (_, agent_id) = scheduler
            .assign_next_task()
            .expect("a workspace-scoped task should be assignable");
        let assigned_agent = scheduler
            .agents()
            .into_iter()
            .find(|agent| agent.agent_id == agent_id)
            .expect("assigned agent should exist");
        assert_eq!(assigned_agent.name, "workspace");
    }

    #[test]
    fn parse_agent_spec_parses_paths_and_labels() {
        let agent = parse_agent_spec("reviewer;review;src,tests;phase=review,lang=rust")
            .expect("agent spec should parse");
        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.role, "review");
        assert_eq!(agent.ownership_paths, vec!["src", "tests"]);
        assert_eq!(
            agent.labels.get("phase").map(String::as_str),
            Some("review")
        );
        assert_eq!(agent.labels.get("lang").map(String::as_str), Some("rust"));
    }

    #[test]
    fn parse_task_spec_sets_budget_paths_labels_and_description() {
        let task = parse_task_spec("Refactor service;src/core;phase=backend;Tighten boundaries")
            .expect("task spec should parse");
        assert_eq!(task.title, "Refactor service");
        assert_eq!(task.ownership_paths, vec!["src/core"]);
        assert_eq!(
            task.required_labels.get("phase").map(String::as_str),
            Some("backend")
        );
        assert_eq!(task.description, "Tighten boundaries");
        assert_eq!(task.budget.read_calls, 32);
        assert_eq!(task.budget.edit_calls, 12);
        assert_eq!(task.budget.command_calls, 8);
    }

    #[test]
    fn parse_agent_spec_rejects_missing_role() {
        let error = parse_agent_spec("reviewer;;;").expect_err("missing role should fail");
        assert!(error.to_string().contains("role is missing"));
    }

    #[test]
    fn agent_definition_for_identity_selects_specialized_roles() {
        let mut planner = AgentIdentity::new("planner", "planner");
        planner.labels.insert("phase".to_owned(), "plan".to_owned());
        assert_eq!(
            agent_definition_for_identity(&planner).agent_type,
            plan_agent().agent_type
        );

        let reviewer = AgentIdentity::new("review", "reviewer");
        assert_eq!(
            agent_definition_for_identity(&reviewer).agent_type,
            verification_agent().agent_type
        );

        let explore = AgentIdentity::new("explore", "researcher");
        assert_eq!(
            agent_definition_for_identity(&explore).agent_type,
            explore_agent().agent_type
        );

        let implementer = AgentIdentity::new("workspace", "implementer");
        assert_eq!(
            agent_definition_for_identity(&implementer).agent_type,
            general_purpose_agent().agent_type
        );
    }

    #[test]
    fn append_conversation_context_maps_tool_role_to_user_context_message() {
        let mut conversation = Vec::new();
        append_conversation_context(
            &mut conversation,
            &[claude_agents::ConversationEntry {
                role: "tool".to_owned(),
                content: "cargo check passed".to_owned(),
            }],
        );
        assert_eq!(conversation.len(), 1);
        assert!(matches!(conversation[0].role, ConversationRole::User));
        assert!(conversation[0].text.contains("[tool context]"));
        assert!(conversation[0].text.contains("cargo check passed"));
    }

    #[test]
    fn guide_agent_runtime_prompt_appends_research_ordered_configuration_sections() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plugin_root = profile.join("plugins").join("sample");
        fs::create_dir_all(cwd.join(".claude").join("agents")).expect("project agents dir");
        fs::create_dir_all(profile.join("skills").join("profile-skill")).expect("profile skills");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin manifest dir");
        fs::create_dir_all(plugin_root.join("skills").join("bundled")).expect("plugin skills dir");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::write(
            profile
                .join("skills")
                .join("profile-skill")
                .join("SKILL.md"),
            "+++\nsummary = \"Profile skill summary\"\n+++\n# Profile Skill\n\nUse it.\n",
        )
        .expect("write profile skill");
        fs::write(
            cwd.join(".claude").join("agents").join("reviewer.md"),
            "---\nname: reviewer\ndescription: Review custom changes\n---\nYou review.\n",
        )
        .expect("write custom agent");
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "sample",
                "version": "0.1.0",
                "skills": "./skills"
            }"#,
        )
        .expect("write plugin manifest");
        fs::write(
            plugin_root.join("skills").join("bundled").join("SKILL.md"),
            "+++\nsummary = \"Plugin skill summary\"\n+++\n# Bundled\n\nUse it.\n",
        )
        .expect("write plugin skill");
        fs::write(
            profile.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            r#"[servers.context7]
command = "python"
args = ["server.py"]"#,
        )
        .expect("write profile mcp");
        fs::write(
            profile.join("settings.json"),
            r#"{
                "provider": {
                    "name": "demo"
                },
                "language": "zh"
            }"#,
        )
        .expect("write settings");

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

        let prompt = build_claude_code_guide_runtime_prompt(
            &config,
            "Base guide prompt.\n- When you cannot find an answer or the feature doesn't exist, direct the user to use /feedback to report a feature request or bug",
        );

        let custom_skills_idx = prompt
            .find("**Available custom skills in this project:**")
            .expect("custom skills section");
        let custom_agents_idx = prompt
            .find("**Available custom agents configured:**")
            .expect("custom agents section");
        let mcp_idx = prompt
            .find("**Configured MCP servers:**")
            .expect("mcp section");
        let plugin_idx = prompt
            .find("**Available plugin skills:**")
            .expect("plugin section");
        let settings_idx = prompt
            .find("**User's settings.json:**")
            .expect("settings section");
        assert!(custom_skills_idx < custom_agents_idx);
        assert!(custom_agents_idx < mcp_idx);
        assert!(mcp_idx < plugin_idx);
        assert!(plugin_idx < settings_idx);
        assert!(prompt.contains("- /profile-skill: Profile skill summary"));
        assert!(prompt.contains("- reviewer: Review custom changes"));
        assert!(prompt.contains("- context7"));
        assert!(prompt.contains("- /bundled: Plugin skill summary"));
        assert!(prompt.contains("\"language\": \"zh\""));
        assert!(prompt.contains("# User's Current Configuration"));
    }

    #[test]
    fn guide_agent_runtime_prompt_leaves_base_prompt_unchanged_without_dynamic_context() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("workspace");

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

        let base_prompt = "Base guide prompt.";
        assert_eq!(
            build_claude_code_guide_runtime_prompt(&config, base_prompt),
            base_prompt
        );
    }

    #[test]
    fn ephemeral_session_paths_isolate_transcript_state() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("workspace");
        let mut config = load_runtime_config(
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
            RuntimeOverrides::default(),
        )
        .expect("config");
        let original_paths = config.paths.clone();
        let original_transcript_path = SessionStore::open(original_paths.clone())
            .expect("original store")
            .session_transcript_path(config.session_id);

        let ephemeral = install_ephemeral_session_paths(&mut config).expect("ephemeral paths");
        assert_ne!(config.paths.state_db_path, original_paths.state_db_path);
        assert_ne!(config.paths.sessions_dir, original_paths.sessions_dir);
        assert_ne!(config.paths.artifacts_dir, original_paths.artifacts_dir);
        assert_eq!(config.paths.profile_dir, original_paths.profile_dir);
        assert!(config.paths.state_db_path.starts_with(ephemeral.path()));
        assert!(config.paths.sessions_dir.starts_with(ephemeral.path()));
        assert!(config.paths.artifacts_dir.starts_with(ephemeral.path()));

        let store = SessionStore::open(config.paths.clone()).expect("ephemeral store");
        store
            .ensure_session(
                config.session_id,
                &cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("ephemeral"),
            )
            .expect("ensure ephemeral session");
        assert!(config.paths.state_db_path.exists());
        assert!(store.session_transcript_path(config.session_id).exists());
        assert!(!original_transcript_path.exists());
    }
}
