mod auto_memory;

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use auto_memory::{
    default_memory_dir_for_permissions, has_valid_cowork_memory_path_override,
    load_cowork_memory_mechanics_prompt, load_default_memory_prompt_with_features,
    memory_dir_for_read_permissions, sanitize_path_component,
    team_memory_dir_for_read_permissions_with_features,
};
use chrono::Local;
use claude_agents::coordinator::{
    COORDINATOR_MODE_ALLOWED_TOOLS, McpClientInfo as CoordinatorMcpClientInfo,
    get_coordinator_system_prompt, get_coordinator_user_context, is_coordinator_mode,
};
use claude_config::{RuntimeConfig, SettingSource};
use claude_context::RuntimeIdentityContext;
use claude_core::{ConversationEntry, ConversationRole, ProviderProtocol};
use claude_model::is_first_party_base_url;
use claude_provider::{DiscoveredToolScope, provider_runtime_tool_specs_for_request};
use claude_system_prompt::{
    EffectiveSystemPromptOptions, McpClientInfo as PromptMcpClientInfo,
    OutputStyleConfig as PromptOutputStyleConfig, PromptContext, PromptFeatures,
    SystemPromptSplitOptions, build_default_system_prompt_for_session,
    build_effective_system_prompt, clear_system_prompt_sections_for_session,
    render_system_prompt_for_api,
};
use claude_tools::{ToolSpec, is_runtime_dynamic_mcp_tool_name};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

const MEMORY_INSTRUCTION_PROMPT: &str = "Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.";
const SCRATCHPAD_FEATURE_KEY: &str = "tengu_scratch";
const SCRATCHPAD_DIRNAME: &str = "scratchpad";
static SESSION_START_DATE: OnceLock<String> = OnceLock::new();

pub use auto_memory::{
    MemoryHeader as AutoMemoryHeader, MemoryPromptFeatures, MemoryScope as AutoMemoryScope,
    MemoryType as AutoMemoryType, SessionMemoryFileType, agent_memory_dir, agent_memory_dirs,
    agent_memory_entrypoint, auto_memory_daily_log_path, auto_memory_entrypoint,
    build_extract_auto_only_prompt as build_extract_memory_auto_only_prompt,
    build_extract_combined_prompt as build_extract_memory_combined_prompt, claude_config_home,
    detect_session_file_type as detect_memory_session_file_type,
    detect_session_pattern_type as detect_memory_session_pattern_type,
    format_memory_manifest as format_auto_memory_manifest, is_agent_memory_path,
    is_auto_managed_memory_file_with_features, is_auto_managed_memory_pattern,
    is_auto_memory_enabled, is_auto_memory_path, is_memory_directory_with_features,
    is_shell_command_targeting_memory_with_features, is_team_memory_file_with_features,
    memory_base_dir, memory_scope_for_path_with_features,
    parse_memory_type as parse_auto_memory_type, scan_memory_files as scan_auto_memory_files,
    team_memory_entrypoint_with_features, team_memory_path_with_features,
};

#[derive(Debug, Clone, Default)]
pub struct PromptRuntimeOverrides {
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub override_system_prompt: Option<String>,
    pub agent_system_prompt: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub critical_system_reminder: Option<String>,
    pub omit_claude_md: bool,
    pub omit_git_status: bool,
}

#[must_use]
pub fn effective_allowed_tool_names(
    overrides: &PromptRuntimeOverrides,
    tool_specs: &[ToolSpec],
) -> Option<HashSet<String>> {
    let mut allowed = overrides
        .allowed_tools
        .as_ref()
        .map(|requested| expand_requested_tool_names(requested, tool_specs));

    if is_coordinator_mode() && overrides.agent_system_prompt.is_none() {
        let coordinator_requested = COORDINATOR_MODE_ALLOWED_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect::<Vec<_>>();
        let mut coordinator_allowed =
            expand_requested_tool_names(&coordinator_requested, tool_specs);
        for spec in tool_specs {
            if is_coordinator_pr_subscription_tool(spec) {
                insert_prompt_tool_aliases(spec, &mut coordinator_allowed);
            }
        }
        allowed = Some(match allowed {
            Some(existing) => existing
                .intersection(&coordinator_allowed)
                .cloned()
                .collect(),
            None => coordinator_allowed,
        });
    }

    allowed
}

fn is_coordinator_pr_subscription_tool(spec: &ToolSpec) -> bool {
    [
        spec.name.as_str(),
        spec.protocol_name.as_str(),
        spec.permission_tool_name.as_str(),
    ]
    .iter()
    .any(|name| {
        name.ends_with("subscribe_pr_activity") || name.ends_with("unsubscribe_pr_activity")
    })
}

#[derive(Debug, Clone)]
pub struct RuntimePromptSettings {
    pub language: Option<String>,
    pub output_style: Option<String>,
    pub proactive_active: bool,
    pub brief_enabled: bool,
    pub mcp_instructions_delta_enabled: bool,
    pub is_non_interactive: bool,
    pub user_invocable_skills_available: bool,
    pub include_token_budget_prompt: bool,
    pub scratchpad_enabled: bool,
    pub scratchpad_dir: Option<String>,
    pub project_temp_dir: Option<String>,
    pub auto_memory_read_dir: Option<String>,
    pub auto_memory_permission_dir: Option<String>,
    pub team_memory_read_dir: Option<String>,
    pub memory_prompt_features: MemoryPromptFeatures,
    pub additional_working_directories: Vec<PathBuf>,
    pub runtime_identity: RuntimeIdentityContext,
}

impl RuntimePromptSettings {
    #[must_use]
    pub fn from_config(config: &RuntimeConfig) -> Self {
        let runtime_identity = RuntimeIdentityContext::from_legacy_env();
        let memory_prompt_features = runtime_memory_prompt_features(&runtime_identity);
        let scratchpad = build_runtime_scratchpad_state(config);
        let tmp_root_override = env::var_os("CLAUDE_CODE_TMPDIR").map(PathBuf::from);
        let project_temp_dir = runtime_project_temp_dir(config, tmp_root_override.as_deref());
        let auto_memory_permission_dir = default_memory_dir_for_permissions(config)
            .ok()
            .flatten()
            .map(|path| path.to_string_lossy().into_owned());
        let auto_memory_read_dir = memory_dir_for_read_permissions(config)
            .ok()
            .flatten()
            .map(|path| path.to_string_lossy().into_owned());
        let team_memory_read_dir =
            team_memory_dir_for_read_permissions_with_features(config, &memory_prompt_features)
                .ok()
                .flatten()
                .map(|path| path.to_string_lossy().into_owned());
        Self {
            language: config.language.clone(),
            output_style: config.output_style.clone(),
            proactive_active: config.proactive_active,
            brief_enabled: config.brief_enabled,
            mcp_instructions_delta_enabled: runtime_mcp_instructions_delta_enabled(),
            is_non_interactive: false,
            user_invocable_skills_available: discover_user_invocable_skills(config),
            include_token_budget_prompt: false,
            scratchpad_enabled: scratchpad.enabled,
            scratchpad_dir: scratchpad.dir,
            project_temp_dir: Some(project_temp_dir.to_string_lossy().into_owned()),
            auto_memory_read_dir,
            auto_memory_permission_dir,
            team_memory_read_dir,
            memory_prompt_features,
            additional_working_directories: Vec::new(),
            runtime_identity,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeSystemPrompt {
    pub text: String,
    pub content_blocks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RuntimeScratchpadState {
    enabled: bool,
    dir: Option<String>,
}

pub fn runtime_env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub fn runtime_env_defined_falsy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// Internal repos where Anthropic-internal model names are allowed.
///
/// This mirrors the upstream allowlist in `commitAttribution.ts`. It is
/// intentionally repo-specific rather than org-wide because the Anthropic orgs
/// also contain public repositories.
const INTERNAL_MODEL_REPOS: &[&str] = &[
    "github.com:anthropics/claude-cli-internal",
    "github.com/anthropics/claude-cli-internal",
    "github.com:anthropics/anthropic",
    "github.com/anthropics/anthropic",
    "github.com:anthropics/apps",
    "github.com/anthropics/apps",
    "github.com:anthropics/casino",
    "github.com/anthropics/casino",
    "github.com:anthropics/dbt",
    "github.com/anthropics/dbt",
    "github.com:anthropics/dotfiles",
    "github.com/anthropics/dotfiles",
    "github.com:anthropics/terraform-config",
    "github.com/anthropics/terraform-config",
    "github.com:anthropics/hex-export",
    "github.com/anthropics/hex-export",
    "github.com:anthropics/feedback-v2",
    "github.com/anthropics/feedback-v2",
    "github.com:anthropics/labs",
    "github.com/anthropics/labs",
    "github.com:anthropics/argo-rollouts",
    "github.com/anthropics/argo-rollouts",
    "github.com:anthropics/starling-configs",
    "github.com/anthropics/starling-configs",
    "github.com:anthropics/ts-tools",
    "github.com/anthropics/ts-tools",
    "github.com:anthropics/ts-capsules",
    "github.com/anthropics/ts-capsules",
    "github.com:anthropics/feldspar-testing",
    "github.com/anthropics/feldspar-testing",
    "github.com:anthropics/trellis",
    "github.com/anthropics/trellis",
    "github.com:anthropics/claude-for-hiring",
    "github.com/anthropics/claude-for-hiring",
    "github.com:anthropics/forge-web",
    "github.com/anthropics/forge-web",
    "github.com:anthropics/infra-manifests",
    "github.com/anthropics/infra-manifests",
    "github.com:anthropics/mycro_manifests",
    "github.com/anthropics/mycro_manifests",
    "github.com:anthropics/mycro_configs",
    "github.com/anthropics/mycro_configs",
    "github.com:anthropics/mobile-apps",
    "github.com/anthropics/mobile-apps",
];

fn remote_marks_internal_model_repo(remote_url: &str) -> bool {
    INTERNAL_MODEL_REPOS
        .iter()
        .any(|allowed_repo| remote_url.contains(allowed_repo))
}

fn runtime_repo_remote_url(cwd: &Path) -> Option<String> {
    run_git_command(cwd, &["remote", "get-url", "origin"])
        .or_else(|| run_git_command(cwd, &["config", "--get", "remote.origin.url"]))
}

fn runtime_undercover_active(
    config: &RuntimeConfig,
    runtime_identity: &RuntimeIdentityContext,
) -> bool {
    if !runtime_identity.is_ant_user() {
        return false;
    }
    if runtime_env_truthy("CLAUDE_CODE_UNDERCOVER") {
        return true;
    }
    runtime_repo_remote_url(&config.cwd)
        .as_deref()
        .is_none_or(|remote| !remote_marks_internal_model_repo(remote))
}

#[must_use]
pub fn runtime_deferred_tools_delta_enabled() -> bool {
    if runtime_env_truthy("CLAUDE_CODE_DEFERRED_TOOLS_DELTA") {
        return true;
    }
    if runtime_env_defined_falsy("CLAUDE_CODE_DEFERRED_TOOLS_DELTA") {
        return false;
    }
    true
}

#[must_use]
pub fn runtime_mcp_instructions_delta_enabled() -> bool {
    if runtime_env_truthy("CLAUDE_CODE_MCP_INSTR_DELTA") {
        return true;
    }
    if runtime_env_defined_falsy("CLAUDE_CODE_MCP_INSTR_DELTA") {
        return false;
    }
    true
}

#[must_use]
pub fn runtime_agent_listing_delta_enabled() -> bool {
    if runtime_env_truthy("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES") {
        return true;
    }
    if runtime_env_defined_falsy("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES") {
        return false;
    }
    false
}

fn custom_prompt_uses_main_thread_precedence(
    custom_system_prompt_provided: bool,
    override_system_prompt_provided: bool,
    agent_system_prompt_provided: bool,
) -> bool {
    custom_system_prompt_provided
        && !override_system_prompt_provided
        && !agent_system_prompt_provided
}

fn non_empty_prompt_block(value: Option<&str>) -> Option<String> {
    value
        .filter(|candidate| !candidate.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn append_runtime_prompt_suffixes(
    prompt_blocks: &mut Vec<String>,
    memory_mechanics_prompt: Option<String>,
    append_system_prompt: Option<&str>,
    override_system_prompt_provided: bool,
) {
    if override_system_prompt_provided {
        return;
    }
    if let Some(memory_mechanics_prompt) = memory_mechanics_prompt {
        prompt_blocks.push(memory_mechanics_prompt);
    }
    if let Some(append_system_prompt) = non_empty_prompt_block(append_system_prompt) {
        prompt_blocks.push(append_system_prompt);
    }
}

pub fn insert_prompt_tool_aliases(spec: &ToolSpec, enabled_tools: &mut HashSet<String>) {
    enabled_tools.insert(spec.name.clone());
    enabled_tools.insert(spec.protocol_name.clone());
    match spec.name.as_str() {
        "read_file" => {
            enabled_tools.insert("Read".to_owned());
        }
        "write_file" => {
            enabled_tools.insert("Write".to_owned());
        }
        "edit_file" | "replace_in_file" => {
            enabled_tools.insert("Edit".to_owned());
        }
        "bash_command" => {
            enabled_tools.insert("Bash".to_owned());
        }
        "glob" => {
            enabled_tools.insert("Glob".to_owned());
        }
        "grep" => {
            enabled_tools.insert("Grep".to_owned());
        }
        "ask_user" => {
            enabled_tools.insert("AskUserQuestion".to_owned());
        }
        "agent" => {
            enabled_tools.insert("Agent".to_owned());
        }
        "task_create" => {
            enabled_tools.insert("Task".to_owned());
            enabled_tools.insert("TaskCreate".to_owned());
        }
        "todo_write" => {
            enabled_tools.insert("TodoWrite".to_owned());
        }
        "send_message" => {
            enabled_tools.insert("SendMessage".to_owned());
        }
        "skill_execute" | "discover_skills" => {
            enabled_tools.insert("Skill".to_owned());
        }
        "sleep" => {
            enabled_tools.insert("Sleep".to_owned());
        }
        _ => {}
    }
}

#[must_use]
pub fn expand_requested_tool_names(
    requested_tools: &[String],
    tool_specs: &[ToolSpec],
) -> HashSet<String> {
    if requested_tools.is_empty() {
        return HashSet::new();
    }

    let requested = requested_tools
        .iter()
        .flat_map(|tool| {
            let trimmed = tool.trim();
            let base = requested_tool_filter_base(trimmed);
            if base == trimmed {
                vec![trimmed.to_owned()]
            } else {
                vec![trimmed.to_owned(), base.to_owned()]
            }
        })
        .collect::<HashSet<_>>();
    let mut expanded = requested_tools.iter().cloned().collect::<HashSet<_>>();

    for spec in tool_specs {
        let mut aliases = HashSet::new();
        insert_prompt_tool_aliases(spec, &mut aliases);
        if aliases
            .iter()
            .any(|alias| requested.contains(alias.as_str()))
        {
            expanded.extend(aliases);
            expanded.insert(spec.name.clone());
            expanded.insert(spec.protocol_name.clone());
        }
    }

    expanded
}

fn requested_tool_filter_base(tool: &str) -> &str {
    tool.split_once('(').map_or(tool, |(name, _)| name.trim())
}

#[must_use]
pub fn conversation_with_runtime_user_context(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    overrides: &PromptRuntimeOverrides,
) -> Vec<ConversationEntry> {
    let user_context_entries = base_runtime_user_context_entries(config, overrides);
    let system_context_entries = base_runtime_system_context_entries(config, overrides);
    augment_conversation_with_runtime_context(
        conversation,
        system_context_entries,
        user_context_entries,
    )
}

pub async fn conversation_with_runtime_user_context_with_settings(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    overrides: &PromptRuntimeOverrides,
    settings: &RuntimePromptSettings,
) -> Vec<ConversationEntry> {
    let user_context_entries =
        runtime_user_context_entries_with_settings(config, overrides, settings).await;
    let system_context_entries = runtime_system_context_entries(config, overrides);
    augment_conversation_with_runtime_context(
        conversation,
        system_context_entries,
        user_context_entries,
    )
}

fn runtime_session_start_date() -> String {
    SESSION_START_DATE
        .get_or_init(|| {
            env::var("CLAUDE_CODE_OVERRIDE_DATE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string())
        })
        .clone()
}

fn runtime_platform_name() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else {
        std::env::consts::OS
    }
}

fn augment_conversation_with_runtime_context(
    conversation: &[ConversationEntry],
    system_context_entries: Vec<(String, String)>,
    user_context_entries: Vec<(String, String)>,
) -> Vec<ConversationEntry> {
    let with_user_context =
        augment_conversation_with_runtime_user_context(conversation, user_context_entries);
    augment_conversation_with_runtime_system_context(&with_user_context, system_context_entries)
}

fn augment_conversation_with_runtime_system_context(
    conversation: &[ConversationEntry],
    context_entries: Vec<(String, String)>,
) -> Vec<ConversationEntry> {
    if context_entries.is_empty()
        || conversation.iter().any(|entry| {
            entry.role == ConversationRole::System
                && entry
                    .text
                    .contains("This is the git status at the start of the conversation.")
        })
    {
        return conversation.to_vec();
    }

    let body = context_entries
        .into_iter()
        .map(|(key, value)| format!("# {key}\n{value}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let reminder = ConversationEntry::system(body);

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

fn augment_conversation_with_runtime_user_context(
    conversation: &[ConversationEntry],
    context_entries: Vec<(String, String)>,
) -> Vec<ConversationEntry> {
    if context_entries.is_empty()
        || conversation.iter().any(|entry| {
            entry.role == ConversationRole::User
                && entry.text.contains(
                    "As you answer the user's questions, you can use the following context:",
                )
        })
    {
        return conversation.to_vec();
    }

    let body = context_entries
        .into_iter()
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

pub async fn build_runtime_system_prompt(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    overrides: &PromptRuntimeOverrides,
    settings: &RuntimePromptSettings,
    discovered_tool_scope: &DiscoveredToolScope,
) -> Result<RuntimeSystemPrompt> {
    let custom_system_prompt_provided = overrides.system_prompt.is_some();
    let override_system_prompt_provided = overrides.override_system_prompt.is_some();
    let agent_system_prompt_provided = overrides.agent_system_prompt.is_some();
    let use_default_system_prompt = !custom_system_prompt_provided
        && !override_system_prompt_provided
        && (!agent_system_prompt_provided || settings.proactive_active);
    let memory_mechanics_prompt = if custom_prompt_uses_main_thread_precedence(
        custom_system_prompt_provided,
        override_system_prompt_provided,
        agent_system_prompt_provided,
    ) && has_valid_cowork_memory_path_override()
    {
        load_cowork_memory_mechanics_prompt(config)?
    } else {
        None
    };
    let session_start_date = runtime_session_start_date();
    if runtime_env_truthy("CLAUDE_CODE_SIMPLE") {
        let default_prompt_blocks = if use_default_system_prompt {
            vec![format!(
                "You are Claude Code, Anthropic's official CLI for Claude.\n\nCWD: {}\nDate: {}",
                config.cwd.display(),
                session_start_date
            )]
        } else {
            Vec::new()
        };
        let mut prompt_blocks = build_effective_system_prompt(
            default_prompt_blocks,
            &EffectiveSystemPromptOptions {
                agent_system_prompt: overrides.agent_system_prompt.clone(),
                coordinator_system_prompt: is_coordinator_mode()
                    .then(|| get_coordinator_system_prompt(true)),
                custom_system_prompt: overrides.system_prompt.clone(),
                append_system_prompt: None,
                override_system_prompt: overrides.override_system_prompt.clone(),
                proactive_active: settings.proactive_active,
            },
        );
        append_runtime_prompt_suffixes(
            &mut prompt_blocks,
            memory_mechanics_prompt.clone(),
            overrides.append_system_prompt.as_deref(),
            override_system_prompt_provided,
        );
        let rendered = render_system_prompt_for_api(
            &prompt_blocks,
            &SystemPromptSplitOptions {
                skip_global_cache_for_system_prompt: true,
            },
        );
        return Ok(RuntimeSystemPrompt {
            text: rendered.text,
            content_blocks: rendered.content_blocks,
        });
    }

    let carried_discovered_tools = discovered_tool_scope.snapshot();
    let visible_tool_specs = provider_runtime_tool_specs_for_request(
        &config.provider,
        conversation,
        &carried_discovered_tools,
    )
    .await;
    let mcp_catalog = claude_tools::mcp_catalog::runtime_mcp_catalog().await;
    let mut enabled_tools = HashSet::new();
    for spec in &visible_tool_specs {
        insert_prompt_tool_aliases(spec, &mut enabled_tools);
    }
    if let Some(allowed) = effective_allowed_tool_names(overrides, &visible_tool_specs) {
        enabled_tools.retain(|tool| allowed.contains(tool.as_str()));
    }
    let enabled_tool_names = enabled_tools.clone();
    let use_global_prompt_cache = should_use_global_prompt_cache_scope(config);

    let prompt_ctx = PromptContext {
        model: config
            .provider
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        cwd: config.cwd.clone(),
        is_git: detect_git_repository(&config.cwd),
        platform: runtime_platform_name().to_owned(),
        shell: detect_shell_name(),
        os_version: detect_os_version(),
        enabled_tools,
        language: settings.language.clone(),
        output_style: runtime_output_style_config(config, settings.output_style.as_deref()),
        mcp_clients: mcp_catalog
            .clients
            .into_iter()
            .map(|client| PromptMcpClientInfo {
                name: client.server_name,
                instructions: client.instructions,
            })
            .collect(),
        mcp_instructions_delta_enabled: settings.mcp_instructions_delta_enabled,
        is_worktree: detect_git_worktree(&config.cwd),
        additional_dirs: settings.additional_working_directories.clone(),
        is_non_interactive: settings.is_non_interactive,
        is_fork_subagent_enabled: settings.runtime_identity.features.is_fork_subagent_enabled,
        session_start_date,
        features: PromptFeatures {
            ant_user: settings.runtime_identity.is_ant_user(),
            proactive_active: settings.proactive_active,
            brief_enabled: settings.brief_enabled,
            simple_mode: false,
            repl_mode_active: false,
            embedded_search_tools: settings.runtime_identity.features.embedded_search_tools,
            user_invocable_skills_available: settings.user_invocable_skills_available,
            explore_plan_agents_enabled: settings
                .runtime_identity
                .features
                .explore_plan_agents_enabled,
            verification_agent_enabled: settings
                .runtime_identity
                .features
                .verification_agent_enabled,
            memory_prompt: if use_default_system_prompt {
                load_default_memory_prompt_with_features(config, &settings.memory_prompt_features)?
            } else {
                None
            },
            scratchpad_enabled: settings.scratchpad_enabled,
            scratchpad_dir: settings.scratchpad_dir.clone(),
            function_result_keep_recent: None,
            include_token_budget_prompt: settings.include_token_budget_prompt,
        },
        is_undercover: runtime_undercover_active(config, &settings.runtime_identity),
    };

    let default_prompt_blocks = if use_default_system_prompt {
        build_default_system_prompt_for_session(
            config.session_id,
            &prompt_ctx,
            use_global_prompt_cache,
        )?
    } else {
        Vec::new()
    };
    let mut prompt_blocks = build_effective_system_prompt(
        default_prompt_blocks,
        &EffectiveSystemPromptOptions {
            agent_system_prompt: overrides.agent_system_prompt.clone(),
            coordinator_system_prompt: is_coordinator_mode()
                .then(|| get_coordinator_system_prompt(false)),
            custom_system_prompt: overrides.system_prompt.clone(),
            append_system_prompt: None,
            override_system_prompt: overrides.override_system_prompt.clone(),
            proactive_active: prompt_ctx.features.proactive_active,
        },
    );
    append_runtime_prompt_suffixes(
        &mut prompt_blocks,
        memory_mechanics_prompt,
        overrides.append_system_prompt.as_deref(),
        override_system_prompt_provided,
    );

    let skip_global_cache_for_system_prompt = use_global_prompt_cache
        && visible_tool_specs
            .iter()
            .filter(|spec| enabled_tool_names.contains(spec.name.as_str()))
            .any(|spec| is_runtime_dynamic_mcp_tool_name(&spec.name));
    let rendered = render_system_prompt_for_api(
        &prompt_blocks,
        &SystemPromptSplitOptions {
            skip_global_cache_for_system_prompt,
        },
    );

    Ok(RuntimeSystemPrompt {
        text: rendered.text,
        content_blocks: rendered.content_blocks,
    })
}

pub async fn refresh_runtime_system_prompt(
    config: &RuntimeConfig,
    conversation: &mut Vec<ConversationEntry>,
    overrides: &PromptRuntimeOverrides,
    settings: &RuntimePromptSettings,
    discovered_tool_scope: &DiscoveredToolScope,
) -> Result<()> {
    let prompt = build_runtime_system_prompt(
        config,
        conversation,
        overrides,
        settings,
        discovered_tool_scope,
    )
    .await?;
    apply_runtime_system_prompt(conversation, prompt);
    Ok(())
}

pub fn clear_runtime_system_prompt_state(session_id: uuid::Uuid) {
    clear_system_prompt_sections_for_session(session_id);
}

pub fn apply_runtime_system_prompt(
    conversation: &mut Vec<ConversationEntry>,
    prompt: RuntimeSystemPrompt,
) {
    if let Some(system_entry) = conversation
        .iter_mut()
        .find(|entry| matches!(entry.role, ConversationRole::System))
    {
        system_entry.text = prompt.text;
        system_entry.history_text = None;
        system_entry.content_blocks = prompt.content_blocks;
        return;
    }
    conversation.insert(
        0,
        ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::System,
            text: prompt.text,
            history_text: None,
            content_blocks: prompt.content_blocks,
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        },
    );
}

fn detect_shell_name() -> String {
    if cfg!(windows) {
        return "powershell".to_owned();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_owned())
}

fn detect_os_version() -> String {
    std::env::var("OS").unwrap_or_else(|_| std::env::consts::OS.to_owned())
}

fn detect_git_repository(cwd: &Path) -> bool {
    if cwd.join(".git").exists() {
        return true;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output()
        .ok()
        .is_some_and(|output| output.status.success())
}

const MAX_GIT_STATUS_CHARS: usize = 2000;

fn run_git_command(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

/// Generate the git status context text matching the TS `getGitStatus()` format exactly.
///
/// The output matches:
/// ```
/// This is the git status at the start of the conversation. Note that this status
/// is a snapshot in time, and will not update during the conversation.
///
/// Current branch: <branch>
///
/// Main branch (you will usually use this for PRs): <default-branch>
///
/// Git user: <username>
///
/// Status:
/// <git status --short output, truncated at 2000 chars>
///
/// Recent commits:
/// <git log --oneline -n 5 output>
/// ```
fn get_git_status_context(cwd: &Path) -> Option<String> {
    if runtime_env_truthy("CLAUDE_CODE_REMOTE") {
        return None;
    }
    if runtime_env_truthy("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS") {
        return None;
    }
    if !detect_git_repository(cwd) {
        return None;
    }

    let branch = run_git_command(cwd, &["branch", "--show-current"]);
    let main_branch = run_git_command(cwd, &["rev-parse", "--abbrev-ref", "origin/HEAD"])
        .map(|refname| {
            refname
                .strip_prefix("origin/")
                .unwrap_or(&refname)
                .to_owned()
        })
        .unwrap_or_else(|| "main".to_owned());

    let git_user = run_git_command(cwd, &["config", "user.name"]);

    let status_output =
        run_git_command(cwd, &["--no-optional-locks", "status", "--short"]).unwrap_or_default();

    let truncated_status = if status_output.len() > MAX_GIT_STATUS_CHARS {
        format!(
            "{}\n... (truncated because it exceeds 2k characters. If you need more information, run \"git status\" using BashTool)",
            &status_output[..MAX_GIT_STATUS_CHARS]
        )
    } else {
        status_output
    };

    let recent_log = run_git_command(cwd, &["--no-optional-locks", "log", "--oneline", "-n", "5"])
        .unwrap_or_else(|| "(no commits)".to_owned());

    let mut lines = vec![
        "This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.".to_owned(),
        "".to_owned(),
        format!("Current branch: {}", branch.as_deref().unwrap_or("(unknown)")),
        "".to_owned(),
        format!("Main branch (you will usually use this for PRs): {main_branch}"),
    ];

    if let Some(ref user) = git_user {
        lines.push("".to_owned());
        lines.push(format!("Git user: {user}"));
    }

    lines.push("".to_owned());
    lines.push(format!(
        "Status:\n{}",
        if truncated_status.is_empty() {
            "(clean)"
        } else {
            &truncated_status
        }
    ));

    lines.push("".to_owned());
    lines.push(format!("Recent commits:\n{recent_log}"));

    Some(lines.join("\n"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeMemoryType {
    Managed,
    User,
    Project,
    Local,
    AutoMem,
    TeamMem,
}

impl ClaudeMemoryType {
    fn description(self) -> &'static str {
        match self {
            Self::Project => " (project instructions, checked into the codebase)",
            Self::Local => " (user's private project instructions, not checked in)",
            Self::Managed | Self::User => " (user's private global instructions for all projects)",
            Self::AutoMem => " (user's auto-memory, persists across conversations)",
            Self::TeamMem => " (shared team memory, synced across the organization)",
        }
    }
}

#[derive(Debug, Clone)]
struct ClaudeMemoryFile {
    path: PathBuf,
    memory_type: ClaudeMemoryType,
    content: String,
    has_paths_frontmatter: bool,
    globs: Vec<String>,
    content_differs_from_disk: bool,
}

#[derive(Debug, Clone)]
struct ClaudeMemoryRoots {
    managed_dir: PathBuf,
    user_config_dir: PathBuf,
    additional_dirs: Vec<PathBuf>,
    additional_dirs_enabled: bool,
}

impl ClaudeMemoryRoots {
    fn from_runtime_settings(settings: Option<&RuntimePromptSettings>) -> Self {
        Self {
            managed_dir: managed_claude_root_dir(),
            user_config_dir: runtime_claude_config_home_dir(),
            additional_dirs: settings
                .map(|value| value.additional_working_directories.clone())
                .unwrap_or_default(),
            additional_dirs_enabled: runtime_env_truthy(
                "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD",
            ),
        }
    }
}

const MAX_CLAUDE_MD_INCLUDE_DEPTH: usize = 5;

fn runtime_home_dir() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home)
    } else if let Ok(userprofile) = env::var("USERPROFILE") {
        PathBuf::from(userprofile)
    } else {
        PathBuf::from(".")
    }
}

fn runtime_claude_config_home_dir() -> PathBuf {
    if let Ok(dir) = env::var("CLAUDE_CONFIG_DIR") {
        PathBuf::from(dir)
    } else {
        runtime_home_dir().join(".claude")
    }
}

fn managed_claude_root_dir() -> PathBuf {
    if env::var("USER_TYPE").as_deref() == Ok("ant")
        && let Ok(path) = env::var("CLAUDE_CODE_MANAGED_SETTINGS_PATH")
    {
        return PathBuf::from(path);
    }

    if cfg!(target_os = "windows") {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

fn normalize_path_for_comparison(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_in_original_cwd(path: &Path, original_cwd: &Path) -> bool {
    let candidate = canonical_or_original(path);
    let root = canonical_or_original(original_cwd);
    candidate.starts_with(root)
}

fn read_memory_file(
    path: &Path,
    memory_type: ClaudeMemoryType,
) -> Option<(ClaudeMemoryFile, Vec<PathBuf>)> {
    let raw = fs::read_to_string(path).ok()?;
    let (without_frontmatter, has_paths_frontmatter, globs) = strip_frontmatter(&raw);
    let without_comments = strip_html_comments_outside_fences(&without_frontmatter);
    let include_paths = extract_include_paths(path, &without_frontmatter);
    let content = without_comments.trim();
    if content.is_empty() {
        return None;
    }

    Some((
        ClaudeMemoryFile {
            path: path.to_path_buf(),
            memory_type,
            content: content.to_owned(),
            has_paths_frontmatter,
            globs,
            content_differs_from_disk: content != raw.trim(),
        },
        include_paths,
    ))
}

fn split_path_frontmatter(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_rule_globs(globs: Vec<String>) -> Vec<String> {
    if globs
        .iter()
        .any(|glob| matches!(glob.trim(), "*" | "**" | "**/*" | "./**" | "./**/*" | "."))
    {
        Vec::new()
    } else {
        globs
            .into_iter()
            .map(|glob| glob.trim().to_owned())
            .filter(|glob| !glob.is_empty())
            .collect()
    }
}

fn strip_frontmatter(raw: &str) -> (String, bool, Vec<String>) {
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return (raw.to_owned(), false, Vec::new());
    };

    let mut offset = raw.len() - rest.len();
    let mut frontmatter_lines = Vec::new();
    for line in rest.split_inclusive(['\n']) {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if trimmed == "---" {
            let mut globs = Vec::new();
            let has_paths_frontmatter = frontmatter_lines.iter().any(|entry: &String| {
                entry.split_once(':').is_some_and(|(key, value)| {
                    if key.trim() == "paths" {
                        globs.extend(split_path_frontmatter(value));
                        true
                    } else {
                        false
                    }
                })
            });
            return (
                raw[offset..].to_owned(),
                has_paths_frontmatter,
                normalize_rule_globs(globs),
            );
        }
        frontmatter_lines.push(trimmed.to_owned());
    }

    (raw.to_owned(), false, Vec::new())
}

fn strip_html_comments_outside_fences(content: &str) -> String {
    let mut result = String::new();
    let mut in_fence: Option<String> = None;
    let mut in_comment = false;

    for line in content.split_inclusive(['\n']) {
        let trimmed = line.trim_start();
        if !in_comment {
            if let Some(fence) = &in_fence {
                if trimmed.starts_with(fence) {
                    in_fence = None;
                }
                result.push_str(line);
                continue;
            }

            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = Some(trimmed.chars().take(3).collect());
                result.push_str(line);
                continue;
            }
        }

        let mut cursor = 0usize;
        while cursor < line.len() {
            if in_comment {
                if let Some(end) = line[cursor..].find("-->") {
                    cursor += end + 3;
                    in_comment = false;
                } else {
                    cursor = line.len();
                }
                continue;
            }

            if let Some(start) = line[cursor..].find("<!--") {
                result.push_str(&line[cursor..cursor + start]);
                cursor += start + 4;
                in_comment = true;
            } else {
                result.push_str(&line[cursor..]);
                cursor = line.len();
            }
        }
    }

    result
}

fn extract_include_paths(file_path: &Path, content: &str) -> Vec<PathBuf> {
    let mut include_paths = Vec::new();
    let mut in_fence: Option<String> = None;
    let mut in_comment = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if !in_comment {
            if let Some(fence) = &in_fence {
                if trimmed.starts_with(fence) {
                    in_fence = None;
                }
                continue;
            }
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = Some(trimmed.chars().take(3).collect());
                continue;
            }
            if trimmed.starts_with("    ") || trimmed.starts_with('\t') {
                continue;
            }
        }

        let mut cleaned = String::new();
        let chars = line.as_bytes();
        let mut index = 0usize;
        let mut in_codespan = false;
        let mut previous_backtick = false;
        while index < chars.len() {
            let slice = &line[index..];
            if in_comment {
                if let Some(end) = slice.find("-->") {
                    index += end + 3;
                    in_comment = false;
                } else {
                    index = chars.len();
                }
                continue;
            }
            if slice.starts_with("<!--") {
                index += 4;
                in_comment = true;
                continue;
            }
            if chars[index] == b'`' {
                if previous_backtick {
                    in_codespan = false;
                    previous_backtick = false;
                } else {
                    in_codespan = !in_codespan;
                    previous_backtick = true;
                }
                index += 1;
                continue;
            }
            previous_backtick = false;
            if !in_codespan {
                cleaned.push(chars[index] as char);
            }
            index += 1;
        }

        include_paths.extend(extract_include_paths_from_text(
            &cleaned,
            file_path.parent().unwrap_or_else(|| Path::new("")),
        ));
    }

    include_paths
}

fn extract_include_paths_from_text(text: &str, base_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }

        let preceded_by_whitespace = index == 0 || bytes[index - 1].is_ascii_whitespace();
        if !preceded_by_whitespace {
            index += 1;
            continue;
        }

        let mut cursor = index + 1;
        let mut raw_path = String::new();
        while cursor < bytes.len() {
            if bytes[cursor].is_ascii_whitespace() {
                break;
            }
            if bytes[cursor] == b'\\' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b' ' {
                raw_path.push(' ');
                cursor += 2;
                continue;
            }
            if bytes[cursor] == b'\\' {
                break;
            }
            raw_path.push(bytes[cursor] as char);
            cursor += 1;
        }

        index = cursor;
        if raw_path.is_empty() {
            continue;
        }

        let path_without_fragment = raw_path
            .split_once('#')
            .map_or(raw_path.as_str(), |(path, _)| path);
        if let Some(path) = resolve_include_path(base_dir, path_without_fragment) {
            paths.push(path);
        }
    }

    paths
}

fn resolve_include_path(base_dir: &Path, raw_path: &str) -> Option<PathBuf> {
    if raw_path.is_empty() {
        return None;
    }

    let first = raw_path.chars().next()?;
    let valid = raw_path.starts_with("./")
        || raw_path.starts_with("~/")
        || (raw_path.starts_with('/') && raw_path.len() > 1)
        || (first.is_ascii_alphanumeric() || matches!(first, '.' | '_' | '-'));
    if !valid || raw_path.starts_with('@') || "#%^&*()".contains(first) {
        return None;
    }

    let resolved = if let Some(rest) = raw_path.strip_prefix("~/") {
        runtime_home_dir().join(rest)
    } else {
        let candidate = PathBuf::from(raw_path);
        if candidate.is_absolute() || raw_path.starts_with('/') {
            candidate
        } else {
            base_dir.join(raw_path)
        }
    };

    if is_non_text_path(&resolved) {
        return None;
    }

    Some(resolved)
}

fn is_non_text_path(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    !matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "txt"
            | "text"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "csv"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "mjs"
            | "cjs"
            | "mts"
            | "cts"
            | "py"
            | "pyi"
            | "pyw"
            | "rb"
            | "erb"
            | "rake"
            | "go"
            | "rs"
            | "java"
            | "kt"
            | "kts"
            | "scala"
            | "c"
            | "cpp"
            | "cc"
            | "cxx"
            | "h"
            | "hpp"
            | "hxx"
            | "cs"
            | "swift"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "bat"
            | "cmd"
            | "env"
            | "ini"
            | "cfg"
            | "conf"
            | "config"
            | "properties"
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
            | "vue"
            | "svelte"
            | "astro"
            | "ejs"
            | "hbs"
            | "pug"
            | "jade"
            | "php"
            | "pl"
            | "pm"
            | "lua"
            | "r"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "clj"
            | "cljs"
            | "cljc"
            | "edn"
            | "hs"
            | "lhs"
            | "elm"
            | "ml"
            | "mli"
            | "f"
            | "f90"
            | "f95"
            | "for"
            | "cmake"
            | "make"
            | "makefile"
            | "gradle"
            | "sbt"
            | "rst"
            | "adoc"
            | "asciidoc"
            | "org"
            | "tex"
            | "latex"
            | "lock"
            | "log"
            | "diff"
            | "patch"
    )
}

fn process_memory_file(
    path: &Path,
    memory_type: ClaudeMemoryType,
    processed_paths: &mut HashSet<String>,
    include_external: bool,
    original_cwd: &Path,
    depth: usize,
    results: &mut Vec<ClaudeMemoryFile>,
) {
    if depth >= MAX_CLAUDE_MD_INCLUDE_DEPTH {
        return;
    }

    let normalized_path = normalize_path_for_comparison(path);
    if !processed_paths.insert(normalized_path) {
        return;
    }

    let canonical_path = fs::canonicalize(path).ok();
    if let Some(canonical_path) = canonical_path.as_deref() {
        processed_paths.insert(normalize_path_for_comparison(canonical_path));
    }

    let Some((memory_file, include_paths)) = read_memory_file(path, memory_type) else {
        return;
    };
    results.push(memory_file);

    for include_path in include_paths {
        if !include_external && !path_in_original_cwd(&include_path, original_cwd) {
            continue;
        }
        process_memory_file(
            &include_path,
            memory_type,
            processed_paths,
            include_external,
            original_cwd,
            depth + 1,
            results,
        );
    }
}

fn process_rules_dir(
    rules_dir: &Path,
    memory_type: ClaudeMemoryType,
    processed_paths: &mut HashSet<String>,
    include_external: bool,
    original_cwd: &Path,
    results: &mut Vec<ClaudeMemoryFile>,
) {
    let Ok(entries) = fs::read_dir(rules_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            process_rules_dir(
                &path,
                memory_type,
                processed_paths,
                include_external,
                original_cwd,
                results,
            );
            continue;
        }
        if !file_type.is_file()
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("md"))
        {
            continue;
        }

        let mut file_results = Vec::new();
        process_memory_file(
            &path,
            memory_type,
            processed_paths,
            include_external,
            original_cwd,
            0,
            &mut file_results,
        );
        results.extend(
            file_results
                .into_iter()
                .filter(|file| !file.has_paths_frontmatter),
        );
    }
}

fn rule_glob_base_dir(
    rules_dir: &Path,
    memory_type: ClaudeMemoryType,
    original_cwd: &Path,
) -> PathBuf {
    match memory_type {
        ClaudeMemoryType::Managed | ClaudeMemoryType::User => canonical_or_original(original_cwd),
        ClaudeMemoryType::Project | ClaudeMemoryType::Local => rules_dir
            .parent()
            .and_then(Path::parent)
            .map(canonical_or_original)
            .unwrap_or_else(|| canonical_or_original(original_cwd)),
        ClaudeMemoryType::AutoMem | ClaudeMemoryType::TeamMem => {
            canonical_or_original(original_cwd)
        }
    }
}

fn build_glob_set(globs: &[String]) -> Option<GlobSet> {
    if globs.is_empty() {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        let glob = GlobBuilder::new(&glob.replace('\\', "/"))
            .literal_separator(true)
            .build()
            .ok()?;
        builder.add(glob);
    }
    builder.build().ok()
}

fn memory_rule_matches_target(
    file: &ClaudeMemoryFile,
    rules_dir: &Path,
    target_path: &Path,
    original_cwd: &Path,
) -> bool {
    if file.globs.is_empty() {
        return true;
    }
    let base_dir = rule_glob_base_dir(rules_dir, file.memory_type, original_cwd);
    let canonical = canonical_or_original(target_path);
    let Ok(relative) = canonical.strip_prefix(&base_dir) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() || relative.starts_with("../") || Path::new(&relative).is_absolute() {
        return false;
    }

    build_glob_set(&file.globs).is_some_and(|set: GlobSet| set.is_match(&relative))
}

fn process_conditioned_rules_dir(
    rules_dir: &Path,
    memory_type: ClaudeMemoryType,
    processed_paths: &mut HashSet<String>,
    include_external: bool,
    original_cwd: &Path,
    target_path: &Path,
    results: &mut Vec<ClaudeMemoryFile>,
) {
    let Ok(entries) = fs::read_dir(rules_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            process_conditioned_rules_dir(
                &path,
                memory_type,
                processed_paths,
                include_external,
                original_cwd,
                target_path,
                results,
            );
            continue;
        }
        if !file_type.is_file()
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("md"))
        {
            continue;
        }

        let mut file_results = Vec::new();
        process_memory_file(
            &path,
            memory_type,
            processed_paths,
            include_external,
            original_cwd,
            0,
            &mut file_results,
        );
        results.extend(file_results.into_iter().filter(|file| {
            file.has_paths_frontmatter
                && memory_rule_matches_target(file, rules_dir, target_path, original_cwd)
        }));
    }
}

fn dedup_runtime_memory_files(files: Vec<ClaudeMemoryFile>) -> Vec<ClaudeMemoryFile> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for file in files {
        if seen.insert(normalize_path_for_comparison(&file.path)) {
            deduped.push(file);
        }
    }
    deduped
}

#[derive(Debug, Clone)]
pub struct RuntimeClaudeMemoryFile {
    pub path: PathBuf,
    pub content: String,
    pub globs: Vec<String>,
    pub content_differs_from_disk: bool,
    pub memory_type: String,
}

impl RuntimeClaudeMemoryFile {
    fn from_claude_memory(file: ClaudeMemoryFile) -> Self {
        Self {
            path: file.path,
            content: file.content,
            globs: file.globs,
            content_differs_from_disk: file.content_differs_from_disk,
            memory_type: format!("{:?}", file.memory_type).to_ascii_lowercase(),
        }
    }
}

fn nested_memory_directories(
    config: &RuntimeConfig,
    target_path: &Path,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let original_cwd = canonical_or_original(&config.original_cwd);
    let target_parent = canonical_or_original(target_path.parent().unwrap_or(target_path));

    let mut nested_dirs = Vec::new();
    if target_parent.starts_with(&original_cwd) {
        let mut current = original_cwd.clone();
        loop {
            nested_dirs.push(current.clone());
            if current == target_parent {
                break;
            }
            let Ok(relative) = target_parent.strip_prefix(&current) else {
                break;
            };
            let Some(next_component) = relative.components().next() else {
                break;
            };
            current = current.join(next_component.as_os_str());
        }
    }

    let mut cwd_level_dirs = config
        .original_cwd
        .ancestors()
        .map(canonical_or_original)
        .collect::<Vec<_>>();
    cwd_level_dirs.reverse();
    (nested_dirs, cwd_level_dirs)
}

pub fn get_managed_and_user_conditional_rules(
    config: &RuntimeConfig,
    target_path: &Path,
    settings: Option<&RuntimePromptSettings>,
) -> Vec<RuntimeClaudeMemoryFile> {
    let roots = ClaudeMemoryRoots::from_runtime_settings(settings);
    let mut processed_paths = HashSet::new();
    let mut results = Vec::new();

    process_conditioned_rules_dir(
        &roots.managed_dir.join(".claude").join("rules"),
        ClaudeMemoryType::Managed,
        &mut processed_paths,
        false,
        &config.original_cwd,
        target_path,
        &mut results,
    );

    if config
        .allowed_setting_sources
        .contains(&SettingSource::User)
    {
        process_conditioned_rules_dir(
            &roots.user_config_dir.join("rules"),
            ClaudeMemoryType::User,
            &mut processed_paths,
            true,
            &config.original_cwd,
            target_path,
            &mut results,
        );
    }

    dedup_runtime_memory_files(results)
        .into_iter()
        .map(RuntimeClaudeMemoryFile::from_claude_memory)
        .collect()
}

pub fn get_memory_files_for_nested_directory(
    config: &RuntimeConfig,
    directory: &Path,
    target_path: &Path,
) -> Vec<RuntimeClaudeMemoryFile> {
    let mut processed_paths = HashSet::new();
    let mut results = Vec::new();

    process_memory_file(
        &directory.join("CLAUDE.md"),
        ClaudeMemoryType::Project,
        &mut processed_paths,
        false,
        &config.original_cwd,
        0,
        &mut results,
    );
    process_memory_file(
        &directory.join(".claude").join("CLAUDE.md"),
        ClaudeMemoryType::Project,
        &mut processed_paths,
        false,
        &config.original_cwd,
        0,
        &mut results,
    );
    process_memory_file(
        &directory.join("CLAUDE.local.md"),
        ClaudeMemoryType::Local,
        &mut processed_paths,
        false,
        &config.original_cwd,
        0,
        &mut results,
    );
    process_rules_dir(
        &directory.join(".claude").join("rules"),
        ClaudeMemoryType::Project,
        &mut processed_paths,
        false,
        &config.original_cwd,
        &mut results,
    );
    process_conditioned_rules_dir(
        &directory.join(".claude").join("rules"),
        ClaudeMemoryType::Project,
        &mut processed_paths,
        false,
        &config.original_cwd,
        target_path,
        &mut results,
    );

    dedup_runtime_memory_files(results)
        .into_iter()
        .map(RuntimeClaudeMemoryFile::from_claude_memory)
        .collect()
}

pub fn get_conditional_rules_for_cwd_level_directory(
    config: &RuntimeConfig,
    directory: &Path,
    target_path: &Path,
) -> Vec<RuntimeClaudeMemoryFile> {
    let mut processed_paths = HashSet::new();
    let mut results = Vec::new();
    process_conditioned_rules_dir(
        &directory.join(".claude").join("rules"),
        ClaudeMemoryType::Project,
        &mut processed_paths,
        false,
        &config.original_cwd,
        target_path,
        &mut results,
    );

    dedup_runtime_memory_files(results)
        .into_iter()
        .map(RuntimeClaudeMemoryFile::from_claude_memory)
        .collect()
}

pub fn get_nested_memory_files_for_target(
    config: &RuntimeConfig,
    target_path: &Path,
    settings: Option<&RuntimePromptSettings>,
) -> Vec<RuntimeClaudeMemoryFile> {
    let target = canonical_or_original(target_path);
    let (nested_dirs, cwd_level_dirs) = nested_memory_directories(config, &target);
    let nested_dir_set = nested_dirs
        .iter()
        .map(|path| normalize_path_for_comparison(path))
        .collect::<HashSet<_>>();
    let mut results = get_managed_and_user_conditional_rules(config, &target, settings);

    for dir in nested_dirs {
        results.extend(get_memory_files_for_nested_directory(config, &dir, &target));
    }
    for dir in cwd_level_dirs {
        if nested_dir_set.contains(&normalize_path_for_comparison(&dir)) {
            continue;
        }
        results.extend(get_conditional_rules_for_cwd_level_directory(
            config, &dir, &target,
        ));
    }

    let mut seen = HashSet::new();
    results.retain(|file| seen.insert(normalize_path_for_comparison(&file.path)));
    results
}

fn collect_claude_md_context_with_roots(
    config: &RuntimeConfig,
    roots: &ClaudeMemoryRoots,
) -> Option<String> {
    let mut processed_paths = HashSet::new();
    let mut memory_files = Vec::new();

    process_memory_file(
        &roots.managed_dir.join("CLAUDE.md"),
        ClaudeMemoryType::Managed,
        &mut processed_paths,
        false,
        &config.original_cwd,
        0,
        &mut memory_files,
    );
    process_rules_dir(
        &roots.managed_dir.join(".claude").join("rules"),
        ClaudeMemoryType::Managed,
        &mut processed_paths,
        false,
        &config.original_cwd,
        &mut memory_files,
    );

    if config
        .allowed_setting_sources
        .contains(&SettingSource::User)
    {
        process_memory_file(
            &roots.user_config_dir.join("CLAUDE.md"),
            ClaudeMemoryType::User,
            &mut processed_paths,
            true,
            &config.original_cwd,
            0,
            &mut memory_files,
        );
        process_rules_dir(
            &roots.user_config_dir.join("rules"),
            ClaudeMemoryType::User,
            &mut processed_paths,
            true,
            &config.original_cwd,
            &mut memory_files,
        );
    }

    let mut directories = config.original_cwd.ancestors().collect::<Vec<_>>();
    directories.reverse();
    for dir in directories {
        if config
            .allowed_setting_sources
            .contains(&SettingSource::Project)
        {
            for path in [dir.join("CLAUDE.md"), dir.join(".claude").join("CLAUDE.md")] {
                process_memory_file(
                    &path,
                    ClaudeMemoryType::Project,
                    &mut processed_paths,
                    false,
                    &config.original_cwd,
                    0,
                    &mut memory_files,
                );
            }
            process_rules_dir(
                &dir.join(".claude").join("rules"),
                ClaudeMemoryType::Project,
                &mut processed_paths,
                false,
                &config.original_cwd,
                &mut memory_files,
            );
        }

        if config
            .allowed_setting_sources
            .contains(&SettingSource::Local)
        {
            process_memory_file(
                &dir.join("CLAUDE.local.md"),
                ClaudeMemoryType::Local,
                &mut processed_paths,
                false,
                &config.original_cwd,
                0,
                &mut memory_files,
            );
        }
    }

    if roots.additional_dirs_enabled {
        for dir in &roots.additional_dirs {
            for path in [dir.join("CLAUDE.md"), dir.join(".claude").join("CLAUDE.md")] {
                process_memory_file(
                    &path,
                    ClaudeMemoryType::Project,
                    &mut processed_paths,
                    false,
                    &config.original_cwd,
                    0,
                    &mut memory_files,
                );
            }
            process_rules_dir(
                &dir.join(".claude").join("rules"),
                ClaudeMemoryType::Project,
                &mut processed_paths,
                false,
                &config.original_cwd,
                &mut memory_files,
            );
        }
    }

    // Load AutoMem MEMORY.md entrypoint (matching TS getMemoryFiles AutoMem loading)
    // Content is truncated to match TS truncateEntrypointContent (200 lines / 25KB caps)
    if is_auto_memory_enabled(config) {
        if let Ok(Some(auto_entrypoint)) = auto_memory_entrypoint(config) {
            if let Some((mut auto_file, _)) =
                read_memory_file(&auto_entrypoint, ClaudeMemoryType::AutoMem)
            {
                auto_file.content = auto_memory::truncate_entrypoint_content(&auto_file.content);
                memory_files.push(auto_file);
            }
        }
    }

    // Load TeamMem MEMORY.md entrypoint (matching TS getMemoryFiles TeamMem loading)
    if let Ok(Some(team_entrypoint)) =
        team_memory_entrypoint_with_features(config, &MemoryPromptFeatures::default())
    {
        if let Some((mut team_file, _)) =
            read_memory_file(&team_entrypoint, ClaudeMemoryType::TeamMem)
        {
            team_file.content = auto_memory::truncate_entrypoint_content(&team_file.content);
            memory_files.push(team_file);
        }
    }

    if memory_files.is_empty() {
        return None;
    }

    let formatted = memory_files
        .into_iter()
        .map(|file| {
            let description = file.memory_type.description();
            if matches!(file.memory_type, ClaudeMemoryType::TeamMem) {
                format!(
                    "Contents of {}{}:\n\n<team-memory-content source=\"shared\">\n{}\n</team-memory-content>",
                    file.path.display(),
                    description,
                    file.content
                )
            } else {
                format!(
                    "Contents of {}{}:\n\n{}",
                    file.path.display(),
                    description,
                    file.content
                )
            }
        })
        .collect::<Vec<_>>();
    Some(format!(
        "{MEMORY_INSTRUCTION_PROMPT}\n\n{}",
        formatted.join("\n\n")
    ))
}

fn collect_claude_md_context(
    config: &RuntimeConfig,
    settings: Option<&RuntimePromptSettings>,
) -> Option<String> {
    if runtime_env_truthy("CLAUDE_CODE_DISABLE_CLAUDE_MDS")
        || runtime_env_truthy("REMOTE_CODE_DISABLE_CLAUDE_MDS")
    {
        return None;
    }
    let roots = ClaudeMemoryRoots::from_runtime_settings(settings);
    if runtime_env_truthy("CLAUDE_CODE_SIMPLE") && roots.additional_dirs.is_empty() {
        return None;
    }
    collect_claude_md_context_with_roots(config, &roots)
}

fn base_runtime_user_context_entries(
    config: &RuntimeConfig,
    overrides: &PromptRuntimeOverrides,
) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    if !overrides.omit_claude_md
        && let Some(claude_md) = collect_claude_md_context(config, None)
    {
        entries.push(("claudeMd".to_owned(), claude_md));
    }
    entries.push((
        "currentDate".to_owned(),
        format!("Today's date is {}.", runtime_session_start_date()),
    ));
    entries
}

fn base_runtime_system_context_entries(
    config: &RuntimeConfig,
    overrides: &PromptRuntimeOverrides,
) -> Vec<(String, String)> {
    runtime_system_context_entries(config, overrides)
}

fn runtime_system_context_entries(
    config: &RuntimeConfig,
    overrides: &PromptRuntimeOverrides,
) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    if !overrides.omit_git_status
        && let Some(git_status) = get_git_status_context(&config.cwd)
    {
        entries.push(("gitStatus".to_owned(), git_status));
    }
    entries
}

async fn runtime_user_context_entries_with_settings(
    config: &RuntimeConfig,
    overrides: &PromptRuntimeOverrides,
    settings: &RuntimePromptSettings,
) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    if !overrides.omit_claude_md
        && let Some(claude_md) = collect_claude_md_context(config, Some(settings))
    {
        entries.push(("claudeMd".to_owned(), claude_md));
    }
    entries.push((
        "currentDate".to_owned(),
        format!("Today's date is {}.", runtime_session_start_date()),
    ));
    let mcp_catalog = claude_tools::mcp_catalog::runtime_mcp_catalog().await;
    let coordinator_mcp_clients = mcp_catalog
        .clients
        .into_iter()
        .map(|client| CoordinatorMcpClientInfo {
            name: client.server_name,
        })
        .collect::<Vec<_>>();
    entries.extend(get_coordinator_user_context(
        &coordinator_mcp_clients,
        settings.scratchpad_dir.as_deref(),
        runtime_env_truthy("CLAUDE_CODE_SIMPLE"),
        settings.scratchpad_enabled,
    ));
    entries
}

fn runtime_feature_gate_enabled(feature_key: &str, default: bool) -> bool {
    let env_suffix = feature_key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let env_names = [
        format!("CLAUDE_CODE_FEATURE_{env_suffix}"),
        format!("REMOTE_CODE_FEATURE_{env_suffix}"),
        env_suffix,
    ];

    for env_name in env_names {
        if runtime_env_truthy(&env_name) {
            return true;
        }
        if runtime_env_defined_falsy(&env_name) {
            return false;
        }
    }

    default
}

fn runtime_memory_prompt_features(
    runtime_identity: &RuntimeIdentityContext,
) -> MemoryPromptFeatures {
    MemoryPromptFeatures {
        team_memory_enabled: runtime_feature_gate_enabled("TEAMMEM", false)
            && runtime_feature_gate_enabled("tengu_herring_clock", false),
        skip_index: runtime_feature_gate_enabled("tengu_moth_copse", false),
        searching_past_context_enabled: runtime_feature_gate_enabled("tengu_coral_fern", false),
        kairos_active: runtime_identity.kairos_active,
        embedded_search_tools: runtime_identity.features.embedded_search_tools,
        repl_mode_active: false,
    }
}

fn runtime_scratchpad_enabled() -> bool {
    runtime_feature_gate_enabled(SCRATCHPAD_FEATURE_KEY, false)
}

fn build_runtime_scratchpad_state(config: &RuntimeConfig) -> RuntimeScratchpadState {
    let tmp_root_override = env::var_os("CLAUDE_CODE_TMPDIR").map(PathBuf::from);
    build_runtime_scratchpad_state_with(
        config,
        runtime_scratchpad_enabled(),
        tmp_root_override.as_deref(),
    )
}

fn build_runtime_scratchpad_state_with(
    config: &RuntimeConfig,
    gate_enabled: bool,
    tmp_root_override: Option<&Path>,
) -> RuntimeScratchpadState {
    if !gate_enabled {
        return RuntimeScratchpadState::default();
    }

    let scratchpad_dir = runtime_scratchpad_dir(config, tmp_root_override);
    let _ = ensure_runtime_scratchpad_dir(&scratchpad_dir);

    RuntimeScratchpadState {
        enabled: true,
        dir: Some(scratchpad_dir.to_string_lossy().into_owned()),
    }
}

fn runtime_scratchpad_dir(config: &RuntimeConfig, tmp_root_override: Option<&Path>) -> PathBuf {
    runtime_project_temp_dir(config, tmp_root_override)
        .join(config.session_id.to_string())
        .join(SCRATCHPAD_DIRNAME)
}

fn runtime_project_temp_dir(config: &RuntimeConfig, tmp_root_override: Option<&Path>) -> PathBuf {
    runtime_claude_temp_dir(tmp_root_override).join(sanitize_path_component(
        &config.original_cwd.to_string_lossy(),
    ))
}

fn runtime_claude_temp_dir(tmp_root_override: Option<&Path>) -> PathBuf {
    let base_tmp_dir = tmp_root_override.map(Path::to_path_buf).unwrap_or_else(|| {
        if cfg!(windows) {
            env::temp_dir()
        } else {
            PathBuf::from("/tmp")
        }
    });
    let resolved_base_tmp_dir = fs::canonicalize(&base_tmp_dir).unwrap_or(base_tmp_dir);
    resolved_base_tmp_dir.join(runtime_claude_temp_dir_name())
}

#[cfg(windows)]
fn runtime_claude_temp_dir_name() -> String {
    "claude".to_owned()
}

#[cfg(not(windows))]
fn runtime_claude_temp_dir_name() -> String {
    let uid = env::var("UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    format!("claude-{uid}")
}

fn ensure_runtime_scratchpad_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn detect_git_worktree(cwd: &Path) -> bool {
    cwd.join(".git").is_file()
}

fn discover_user_invocable_skills(config: &RuntimeConfig) -> bool {
    claude_skills::discover_skills(&config.paths.skills_dir)
        .map(|skills| !skills.is_empty())
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct RuntimeOutputStyleCandidate {
    config: PromptOutputStyleConfig,
    force_for_plugin: bool,
}

fn explanatory_feature_prompt() -> String {
    "\n## Insights\nIn order to encourage learning, before and after writing code, always provide brief educational explanations about implementation choices using (with backticks):\n\"`★ Insight ─────────────────────────────────────`\n[2-3 key educational points]\n`─────────────────────────────────────────────────`\"\n\nThese insights should be included in the conversation, not in the codebase. You should generally focus on interesting insights that are specific to the codebase or the code you just wrote, rather than general programming concepts.".to_owned()
}

fn builtin_output_style_candidates() -> BTreeMap<String, RuntimeOutputStyleCandidate> {
    let mut styles = BTreeMap::new();
    let feature_prompt = explanatory_feature_prompt();
    styles.insert(
        "Explanatory".to_owned(),
        RuntimeOutputStyleCandidate {
            config: PromptOutputStyleConfig {
                name: "Explanatory".to_owned(),
                prompt: format!(
                    "You are an interactive CLI tool that helps users with software engineering tasks. In addition to software engineering tasks, you should provide educational insights about the codebase along the way.\n\nYou should be clear and educational, providing helpful explanations while remaining focused on the task. Balance educational content with task completion. When providing insights, you may exceed typical length constraints, but remain focused and relevant.\n\n# Explanatory Style Active\n{feature_prompt}"
                ),
                keep_coding_instructions: true,
            },
            force_for_plugin: false,
        },
    );

    let feature_prompt = explanatory_feature_prompt();
    styles.insert(
        "Learning".to_owned(),
        RuntimeOutputStyleCandidate {
            config: PromptOutputStyleConfig {
                name: "Learning".to_owned(),
                prompt: format!(
                    "You are an interactive CLI tool that helps users with software engineering tasks. In addition to software engineering tasks, you should help users learn more about the codebase through hands-on practice and educational insights.\n\nYou should be collaborative and encouraging. Balance task completion with learning by requesting user input for meaningful design decisions while handling routine implementation yourself.   \n\n# Learning Style Active\n## Requesting Human Contributions\nIn order to encourage learning, ask the human to contribute 2-10 line code pieces when generating 20+ lines involving:\n- Design decisions (error handling, data structures)\n- Business logic with multiple valid approaches  \n- Key algorithms or interface definitions\n\n**TodoList Integration**: If using a TodoList for the overall task, include a specific todo item like \"Request human input on [specific decision]\" when planning to request human input. This ensures proper task tracking. Note: TodoList is not required for all tasks.\n\nExample TodoList flow:\n   ✓ \"Set up component structure with placeholder for logic\"\n   ✓ \"Request human collaboration on decision logic implementation\"\n   ✓ \"Integrate contribution and complete feature\"\n\n### Request Format\n```\n• **Learn by Doing**\n**Context:** [what's built and why this decision matters]\n**Your Task:** [specific function/section in file, mention file and TODO(human) but do not include line numbers]\n**Guidance:** [trade-offs and constraints to consider]\n```\n\n### Key Guidelines\n- Frame contributions as valuable design decisions, not busy work\n- You must first add a TODO(human) section into the codebase with your editing tools before making the Learn by Doing request      \n- Make sure there is one and only one TODO(human) section in the code\n- Don't take any action or output anything after the Learn by Doing request. Wait for human implementation before proceeding.\n\n### Example Requests\n\n**Whole Function Example:**\n```\n• **Learn by Doing**\n\n**Context:** I've set up the hint feature UI with a button that triggers the hint system. The infrastructure is ready: when clicked, it calls selectHintCell() to determine which cell to hint, then highlights that cell with a yellow background and shows possible values. The hint system needs to decide which empty cell would be most helpful to reveal to the user.\n\n**Your Task:** In sudoku.js, implement the selectHintCell(board) function. Look for TODO(human). This function should analyze the board and return {{row, col}} for the best cell to hint, or null if the puzzle is complete.\n\n**Guidance:** Consider multiple strategies: prioritize cells with only one possible value (naked singles), or cells that appear in rows/columns/boxes with many filled cells. You could also consider a balanced approach that helps without making it too easy. The board parameter is a 9x9 array where 0 represents empty cells.\n```\n\n**Partial Function Example:**\n```\n• **Learn by Doing**\n\n**Context:** I've built a file upload component that validates files before accepting them. The main validation logic is complete, but it needs specific handling for different file type categories in the switch statement.\n\n**Your Task:** In upload.js, inside the validateFile() function's switch statement, implement the 'case \"document\":' branch. Look for TODO(human). This should validate document files (pdf, doc, docx).\n\n**Guidance:** Consider checking file size limits (maybe 10MB for documents?), validating the file extension matches the MIME type, and returning {{valid: boolean, error?: string}}. The file object has properties: name, size, type.\n```\n\n**Debugging Example:**\n```\n• **Learn by Doing**\n\n**Context:** The user reported that number inputs aren't working correctly in the calculator. I've identified the handleInput() function as the likely source, but need to understand what values are being processed.\n\n**Your Task:** In calculator.js, inside the handleInput() function, add 2-3 console.log statements after the TODO(human) comment to help debug why number inputs fail.\n\n**Guidance:** Consider logging: the raw input value, the parsed result, and any validation state. This will help us understand where the conversion breaks.\n```\n\n### After Contributions\nShare one insight connecting their code to broader patterns or system effects. Avoid praise or repetition.\n\n## Insights\n{feature_prompt}"
                ),
                keep_coding_instructions: true,
            },
            force_for_plugin: false,
        },
    );
    styles
}

#[must_use]
pub fn available_runtime_output_style_names(config: &RuntimeConfig) -> Vec<String> {
    let mut names = vec!["default".to_owned()];
    names.extend(runtime_output_style_registry(config).into_keys());
    names
}

fn runtime_output_style_config(
    config: &RuntimeConfig,
    style_name: Option<&str>,
) -> Option<PromptOutputStyleConfig> {
    let styles = runtime_output_style_registry(config);
    if let Some(forced) = styles
        .values()
        .find(|style| style.force_for_plugin)
        .map(|style| style.config.clone())
    {
        return Some(forced);
    }

    match style_name.map(str::trim) {
        None | Some("" | "default") => None,
        Some(name) => styles.get(name).map(|style| style.config.clone()),
    }
}

fn runtime_output_style_registry(
    config: &RuntimeConfig,
) -> BTreeMap<String, RuntimeOutputStyleCandidate> {
    let mut styles = builtin_output_style_candidates();

    for style in runtime_plugin_output_styles(config) {
        styles.insert(style.config.name.clone(), style);
    }
    if config
        .allowed_setting_sources
        .contains(&SettingSource::User)
    {
        for dir in runtime_user_output_style_dirs(config) {
            for style in load_runtime_output_styles_dir(&dir, false) {
                styles.insert(style.config.name.clone(), style);
            }
        }
    }
    if config
        .allowed_setting_sources
        .contains(&SettingSource::Project)
    {
        for dir in runtime_project_output_style_dirs(config) {
            for style in load_runtime_output_styles_dir(&dir, false) {
                styles.insert(style.config.name.clone(), style);
            }
        }
    }
    styles
}

fn runtime_plugin_output_styles(config: &RuntimeConfig) -> Vec<RuntimeOutputStyleCandidate> {
    if !config
        .allowed_setting_sources
        .contains(&SettingSource::User)
        || !config.paths.plugins_dir.exists()
    {
        return Vec::new();
    }

    let Ok(plugins) = claude_plugins::discover_plugins(&config.paths.plugins_dir) else {
        return Vec::new();
    };

    plugins
        .iter()
        .flat_map(|plugin| {
            claude_plugins::load_output_styles::load_plugin_bundle_output_styles(plugin).styles
        })
        .map(|style| RuntimeOutputStyleCandidate {
            config: PromptOutputStyleConfig {
                name: style.name,
                prompt: style.prompt,
                keep_coding_instructions: style.keep_coding_instructions.unwrap_or(true),
            },
            force_for_plugin: style.force_for_plugin.unwrap_or(false),
        })
        .collect()
}

fn runtime_user_output_style_dirs(config: &RuntimeConfig) -> Vec<PathBuf> {
    let dirs = vec![
        runtime_claude_config_home_dir().join("output-styles"),
        config.paths.profile_dir.join("output-styles"),
    ];
    dedup_paths(dirs)
}

fn runtime_project_output_style_dirs(config: &RuntimeConfig) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut ancestors = config.original_cwd.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    for dir in ancestors {
        dirs.push(dir.join(".claude").join("output-styles"));
        dirs.push(dir.join(".remote-code-rust").join("output-styles"));
    }
    dedup_paths(dirs)
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        let key = normalize_path_for_comparison(&canonical_or_original(&path));
        if seen.insert(key) {
            deduped.push(path);
        }
    }
    deduped
}

fn load_runtime_output_styles_dir(
    dir: &Path,
    force_for_plugin: bool,
) -> Vec<RuntimeOutputStyleCandidate> {
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut styles = Vec::new();
    let mut paths = WalkDir::new(dir)
        .follow_links(true)
        .sort_by(|a, b| a.path().cmp(b.path()))
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(|ext| ext.to_str()) == Some("md")
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let file_stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("custom");
        if let Some(style) = parse_runtime_output_style_file(&content, file_stem, force_for_plugin)
        {
            styles.push(style);
        }
    }
    styles
}

fn parse_runtime_output_style_file(
    content: &str,
    file_stem: &str,
    force_for_plugin: bool,
) -> Option<RuntimeOutputStyleCandidate> {
    let (frontmatter, body) = split_markdown_frontmatter(content);
    let name = frontmatter
        .as_ref()
        .and_then(|fm| frontmatter_string(fm, "name"))
        .unwrap_or_else(|| file_stem.to_owned());
    let prompt = body.trim();
    if prompt.is_empty() {
        return None;
    }
    let keep_coding_instructions = frontmatter
        .as_ref()
        .and_then(|fm| frontmatter_bool(fm, "keep-coding-instructions"))
        .unwrap_or(true);

    Some(RuntimeOutputStyleCandidate {
        config: PromptOutputStyleConfig {
            name,
            prompt: prompt.to_owned(),
            keep_coding_instructions,
        },
        force_for_plugin,
    })
}

fn split_markdown_frontmatter(content: &str) -> (Option<String>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_owned());
    }
    let rest = &trimmed[3..];
    let Some(end) = rest.find("\n---") else {
        return (None, content.to_owned());
    };
    let frontmatter = rest[..end].to_owned();
    let mut body = rest[end + 4..].to_owned();
    if let Some(stripped) = body.strip_prefix('\r') {
        body = stripped.to_owned();
    }
    if let Some(stripped) = body.strip_prefix('\n') {
        body = stripped.to_owned();
    }
    (Some(frontmatter), body)
}

fn frontmatter_string(frontmatter: &str, key: &str) -> Option<String> {
    let value = frontmatter_value(frontmatter, key)?;
    let value = unquote_frontmatter_value(value.trim());
    (!value.is_empty()).then(|| value.to_owned())
}

fn frontmatter_bool(frontmatter: &str, key: &str) -> Option<bool> {
    let value = unquote_frontmatter_value(frontmatter_value(frontmatter, key)?.trim());
    match value.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn frontmatter_value<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    frontmatter.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key).then_some(value.trim())
    })
}

fn unquote_frontmatter_value(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|inner| inner.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn should_use_global_prompt_cache_scope(config: &RuntimeConfig) -> bool {
    matches!(config.provider.protocol, ProviderProtocol::Anthropic)
        && config
            .provider
            .base_url
            .as_deref()
            .is_some_and(is_first_party_base_url)
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeMemoryRoots, MemoryPromptFeatures, PromptRuntimeOverrides, RuntimePromptSettings,
        augment_conversation_with_runtime_context, build_runtime_scratchpad_state_with,
        build_runtime_system_prompt, clear_runtime_system_prompt_state,
        collect_claude_md_context_with_roots, expand_requested_tool_names,
        remote_marks_internal_model_repo, runtime_claude_temp_dir_name,
        runtime_output_style_config, runtime_user_context_entries_with_settings,
        sanitize_path_component,
    };
    use claude_config::settings_layers::RuntimeOverrides;
    use claude_config::{ProviderOverrides, SettingSource, load_runtime_config};
    use claude_context::{RuntimeIdentityContext, RuntimeUserType};
    use claude_core::{
        ConversationEntry, ConversationRole, InputFormat, OutputFormat, PermissionMode,
        ProviderProtocol,
    };
    use claude_provider::DiscoveredToolScope;
    use claude_tools::ToolSpec;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use tempfile::tempdir;
    use tokio::sync::{Mutex, MutexGuard};

    fn test_config(explicit_settings: Option<std::path::PathBuf>) -> claude_config::RuntimeConfig {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        std::mem::forget(tempdir);

        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::BypassPermissions,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides::default(),
            RuntimeOverrides {
                settings_files: explicit_settings.into_iter().collect(),
                ..RuntimeOverrides::default()
            },
        )
        .expect("runtime config");
        config.provider.protocol = ProviderProtocol::Anthropic;
        config.provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        config
    }

    fn test_settings(config: &claude_config::RuntimeConfig) -> RuntimePromptSettings {
        RuntimePromptSettings {
            language: config.language.clone(),
            output_style: config.output_style.clone(),
            proactive_active: config.proactive_active,
            brief_enabled: config.brief_enabled,
            mcp_instructions_delta_enabled: true,
            is_non_interactive: false,
            user_invocable_skills_available: false,
            include_token_budget_prompt: false,
            scratchpad_enabled: false,
            scratchpad_dir: None,
            project_temp_dir: None,
            auto_memory_read_dir: None,
            auto_memory_permission_dir: None,
            team_memory_read_dir: None,
            memory_prompt_features: MemoryPromptFeatures::default(),
            additional_working_directories: Vec::new(),
            runtime_identity: RuntimeIdentityContext::default(),
        }
    }

    async fn coordinator_override_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    #[tokio::test]
    async fn default_prompt_populates_memory_section() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let settings = test_settings(&config);

        let prompt = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("prompt");

        assert!(prompt.text.contains("# auto memory"));
        assert!(prompt.text.contains("## How to save memories"));
    }

    #[tokio::test]
    async fn ant_runtime_defaults_to_undercover_outside_internal_repo() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let mut settings = test_settings(&config);
        settings.runtime_identity.user_type = RuntimeUserType::Ant;

        let prompt = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("prompt");

        assert!(!prompt.text.contains("The exact model ID is"));
        assert!(!prompt.text.contains("Claude Code is available as a CLI"));
        assert!(!prompt.text.contains("Fast mode for Claude Code"));
    }

    #[test]
    fn internal_repo_remote_allowlist_matches_reference_shapes() {
        assert!(remote_marks_internal_model_repo(
            "git@github.com:anthropics/claude-cli-internal.git"
        ));
        assert!(remote_marks_internal_model_repo(
            "https://github.com/anthropics/anthropic.git"
        ));
        assert!(!remote_marks_internal_model_repo(
            "https://github.com/anthropics/claude-code.git"
        ));
        assert!(!remote_marks_internal_model_repo(
            "https://github.com/example/public.git"
        ));
    }

    #[test]
    fn requested_tool_expansion_accepts_reference_matcher_shape() {
        let specs = vec![ToolSpec {
            name: "bash_command".to_owned(),
            protocol_name: "Bash".to_owned(),
            permission_tool_name: "Bash".to_owned(),
            description: String::new(),
            requires_permission: true,
            input_schema: serde_json::json!({"type":"object"}),
        }];

        let expanded = expand_requested_tool_names(&["Bash(git:*)".to_owned()], &specs);

        assert!(expanded.contains("Bash(git:*)"));
        assert!(expanded.contains("Bash"));
        assert!(expanded.contains("bash_command"));
    }

    #[test]
    fn runtime_output_style_loads_project_frontmatter_style() {
        let config = test_config(None);
        let styles_dir = config.original_cwd.join(".claude").join("output-styles");
        fs::create_dir_all(&styles_dir).expect("styles dir");
        fs::write(
            styles_dir.join("reviewer.md"),
            "---\nname: Reviewer\ndescription: Review style\nkeep-coding-instructions: false\n---\nFocus on defects first.",
        )
        .expect("style file");

        let style = runtime_output_style_config(&config, Some("Reviewer")).expect("style");
        let names = super::available_runtime_output_style_names(&config);

        assert_eq!(style.name, "Reviewer");
        assert_eq!(style.prompt, "Focus on defects first.");
        assert!(!style.keep_coding_instructions);
        assert!(names.contains(&"Reviewer".to_owned()));
    }

    #[test]
    fn runtime_output_style_forced_plugin_style_overrides_settings() {
        let mut config = test_config(None);
        config.output_style = Some("Explanatory".to_owned());
        let plugin_root = config.paths.plugins_dir.join("coach");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("manifest dir");
        fs::create_dir_all(plugin_root.join("output-styles")).expect("styles dir");
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"coach","version":"1.0.0"}"#,
        )
        .expect("manifest");
        fs::write(
            plugin_root.join("output-styles").join("pair.md"),
            "---\nname: Pair\nforce-for-plugin: true\n---\nPair with the user.",
        )
        .expect("style file");

        let style = runtime_output_style_config(&config, config.output_style.as_deref())
            .expect("forced plugin style");

        assert_eq!(style.name, "coach:Pair");
        assert_eq!(style.prompt, "Pair with the user.");
    }

    #[tokio::test]
    async fn custom_prompt_still_skips_default_memory_section() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let settings = test_settings(&config);

        let prompt = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides {
                system_prompt: Some("Custom prompt".to_owned()),
                ..PromptRuntimeOverrides::default()
            },
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("prompt");

        assert!(prompt.text.contains("Custom prompt"));
        assert!(!prompt.text.contains("# auto memory"));
    }

    #[tokio::test]
    async fn runtime_prompt_includes_additional_working_directories() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let mut settings = test_settings(&config);
        settings.additional_working_directories = vec![
            PathBuf::from("C:/workspace/extra-one"),
            PathBuf::from("D:/workspace/extra-two"),
        ];

        let prompt = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("prompt");

        assert!(prompt.text.contains("Additional working directories:"));
        assert!(prompt.text.contains("C:/workspace/extra-one"));
        assert!(prompt.text.contains("D:/workspace/extra-two"));
    }

    #[test]
    fn scratchpad_state_derives_session_directory_from_original_cwd() {
        let tempdir = tempdir().expect("tempdir");
        let config = test_config(None);

        let scratchpad = build_runtime_scratchpad_state_with(&config, true, Some(tempdir.path()));
        let expected = fs::canonicalize(tempdir.path())
            .unwrap_or_else(|_| tempdir.path().to_path_buf())
            .join(runtime_claude_temp_dir_name())
            .join(sanitize_path_component(
                &config.original_cwd.to_string_lossy(),
            ))
            .join(config.session_id.to_string())
            .join("scratchpad")
            .to_string_lossy()
            .into_owned();

        assert!(scratchpad.enabled);
        assert_eq!(scratchpad.dir.as_deref(), Some(expected.as_str()));
        assert!(std::path::Path::new(&expected).is_dir());
    }

    #[tokio::test]
    async fn runtime_user_context_entries_include_worker_tools_context_when_coordinator_enabled() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        claude_agents::coordinator::match_session_mode(Some(
            claude_agents::coordinator::CoordinatorMode::Coordinator,
        ));

        let config = test_config(None);
        let mut settings = test_settings(&config);
        settings.scratchpad_enabled = true;
        settings.scratchpad_dir = Some("C:/scratchpad/session".to_owned());

        let entries = runtime_user_context_entries_with_settings(
            &config,
            &PromptRuntimeOverrides::default(),
            &settings,
        )
        .await;

        let worker_context = entries
            .iter()
            .find(|(key, _)| key == "workerToolsContext")
            .expect("workerToolsContext entry");
        assert!(
            worker_context
                .1
                .contains("Workers spawned via the Agent tool")
        );
        assert!(
            worker_context
                .1
                .contains("Scratchpad directory: C:/scratchpad/session")
        );

        claude_agents::coordinator::reset_coordinator_override();
    }

    #[test]
    fn runtime_context_places_git_status_in_system_context() {
        let conversation = vec![
            ConversationEntry::system("base system"),
            ConversationEntry::user("hello"),
        ];

        let augmented = augment_conversation_with_runtime_context(
            &conversation,
            vec![(
                "gitStatus".to_owned(),
                "This is the git status at the start of the conversation.".to_owned(),
            )],
            vec![(
                "currentDate".to_owned(),
                "Today's date is 2026-05-17.".to_owned(),
            )],
        );

        assert_eq!(augmented[0].role, ConversationRole::System);
        assert_eq!(augmented[1].role, ConversationRole::System);
        assert!(augmented[1].text.contains("# gitStatus"));
        assert_eq!(augmented[2].role, ConversationRole::User);
        assert!(augmented[2].text.contains("# currentDate"));
        assert!(!augmented[2].text.contains("# gitStatus"));
    }

    #[tokio::test]
    async fn default_prompt_populates_scratchpad_section_when_enabled() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let mut settings = test_settings(&config);
        settings.scratchpad_enabled = true;
        settings.scratchpad_dir = Some("C:/scratchpad/session".to_owned());

        let prompt = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("prompt");

        assert!(prompt.text.contains("# Scratchpad Directory"));
        assert!(prompt.text.contains("C:/scratchpad/session"));
    }

    #[tokio::test]
    async fn default_prompt_reuses_session_cached_sections_until_cleared() {
        let _guard = coordinator_override_lock().await;
        claude_agents::coordinator::reset_coordinator_override();
        let config = test_config(None);
        let mut settings = test_settings(&config);
        settings.language = Some("English".to_owned());

        let first = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("first prompt");
        assert!(first.text.contains("Always respond in English."));

        settings.language = Some("Chinese".to_owned());
        let second = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("second prompt");
        assert!(second.text.contains("Always respond in English."));
        assert!(!second.text.contains("Always respond in Chinese."));

        clear_runtime_system_prompt_state(config.session_id);

        let third = build_runtime_system_prompt(
            &config,
            &[ConversationEntry::user("test")],
            &PromptRuntimeOverrides::default(),
            &settings,
            &DiscoveredToolScope::default(),
        )
        .await
        .expect("third prompt");
        assert!(third.text.contains("Always respond in Chinese."));
    }

    #[test]
    fn claude_md_context_loads_expected_memory_order() {
        let temp = tempdir().expect("tempdir");
        let managed = temp.path().join("managed");
        let user = temp.path().join("user");
        let repo = temp.path().join("repo");
        let nested = repo.join("nested");
        let extra = temp.path().join("extra");
        for dir in [
            managed.join(".claude").join("rules"),
            user.join("rules"),
            repo.join(".claude").join("rules"),
            nested.join(".claude").join("rules"),
            extra.join(".claude").join("rules"),
        ] {
            fs::create_dir_all(dir).expect("dir");
        }
        fs::write(managed.join("CLAUDE.md"), "TOKEN_MANAGED").expect("managed");
        fs::write(
            managed
                .join(".claude")
                .join("rules")
                .join("managed-rule.md"),
            "TOKEN_MANAGED_RULE",
        )
        .expect("managed rule");
        fs::write(user.join("CLAUDE.md"), "TOKEN_USER").expect("user");
        fs::write(user.join("rules").join("user-rule.md"), "TOKEN_USER_RULE").expect("user rule");
        fs::write(repo.join("CLAUDE.md"), "TOKEN_REPO").expect("repo");
        fs::write(repo.join(".claude").join("CLAUDE.md"), "TOKEN_REPO_DOT").expect("repo dot");
        fs::write(
            repo.join(".claude").join("rules").join("repo-rule.md"),
            "TOKEN_REPO_RULE",
        )
        .expect("repo rule");
        fs::write(nested.join("CLAUDE.md"), "TOKEN_NESTED").expect("nested");
        fs::write(nested.join("CLAUDE.local.md"), "TOKEN_LOCAL").expect("local");
        fs::write(
            nested.join(".claude").join("rules").join("conditional.md"),
            "---\npaths: src/**/*.rs\n---\nconditional rule",
        )
        .expect("conditional");
        fs::write(extra.join("CLAUDE.md"), "TOKEN_EXTRA").expect("extra");

        let mut config = test_config(None);
        config.cwd = nested.clone();
        config.original_cwd = nested.clone();
        config.allowed_setting_sources = vec![
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ];

        let context = collect_claude_md_context_with_roots(
            &config,
            &ClaudeMemoryRoots {
                managed_dir: managed,
                user_config_dir: user,
                additional_dirs: vec![extra],
                additional_dirs_enabled: true,
            },
        )
        .expect("claude md context");

        let managed_index = context.find("TOKEN_MANAGED").expect("managed");
        let user_index = context.find("TOKEN_USER").expect("user");
        let repo_index = context.find("TOKEN_REPO").expect("repo");
        let nested_index = context.find("TOKEN_NESTED").expect("nested");
        let local_index = context.find("TOKEN_LOCAL").expect("local instructions");
        let extra_index = context.find("TOKEN_EXTRA").expect("extra");

        assert!(user_index > managed_index);
        assert!(repo_index > user_index);
        assert!(nested_index > repo_index);
        assert!(local_index > nested_index);
        assert!(extra_index > local_index);
        assert!(context.contains("(project instructions, checked into the codebase)"));
        assert!(context.contains("(user's private project instructions, not checked in)"));
        assert!(
            !context.contains("conditional rule"),
            "conditional rules should not be eagerly injected"
        );
    }

    #[test]
    fn claude_md_context_honors_setting_source_gates() {
        let temp = tempdir().expect("tempdir");
        let managed = temp.path().join("managed");
        let user = temp.path().join("user");
        let repo = temp.path().join("repo");
        fs::create_dir_all(&managed).expect("managed");
        fs::create_dir_all(&user).expect("user");
        fs::create_dir_all(&repo).expect("repo");
        fs::write(managed.join("CLAUDE.md"), "TOKEN_MANAGED_GATE").expect("managed");
        fs::write(user.join("CLAUDE.md"), "TOKEN_USER_GATE").expect("user");
        fs::write(repo.join("CLAUDE.md"), "TOKEN_PROJECT_GATE").expect("project");
        fs::write(repo.join("CLAUDE.local.md"), "TOKEN_LOCAL_GATE").expect("local");

        let mut config = test_config(None);
        config.cwd = repo.clone();
        config.original_cwd = repo.clone();
        config.allowed_setting_sources = vec![SettingSource::Project];

        let context = collect_claude_md_context_with_roots(
            &config,
            &ClaudeMemoryRoots {
                managed_dir: managed,
                user_config_dir: user,
                additional_dirs: Vec::new(),
                additional_dirs_enabled: false,
            },
        )
        .expect("context");

        assert!(context.contains("TOKEN_MANAGED_GATE"));
        assert!(context.contains("TOKEN_PROJECT_GATE"));
        assert!(!context.contains("TOKEN_USER_GATE"));
        assert!(!context.contains("TOKEN_LOCAL_GATE"));
    }

    #[test]
    fn claude_md_context_processes_includes_and_blocks_external_project_includes() {
        let temp = tempdir().expect("tempdir");
        let managed = temp.path().join("managed");
        let user = temp.path().join("user");
        let repo = temp.path().join("repo");
        let external = temp.path().join("external");
        fs::create_dir_all(&managed).expect("managed");
        fs::create_dir_all(&user).expect("user");
        fs::create_dir_all(&repo).expect("repo");
        fs::create_dir_all(&external).expect("external");
        fs::write(repo.join("child.md"), "TOKEN_CHILD_INCLUDE").expect("child");
        fs::write(external.join("outside.md"), "TOKEN_OUTSIDE_INCLUDE").expect("outside");
        fs::write(repo.join("ignored-too.md"), "TOKEN_IGNORED_INCLUDE").expect("ignored");
        fs::write(
            repo.join("CLAUDE.md"),
            "project parent @./child.md @../external/outside.md\n`@./ignored.md`\n```text\n@./ignored-too.md\n```",
        )
        .expect("project claude");
        fs::write(
            user.join("CLAUDE.md"),
            "user parent @../external/outside.md",
        )
        .expect("user claude");

        let mut config = test_config(None);
        config.cwd = repo.clone();
        config.original_cwd = repo.clone();
        config.allowed_setting_sources = vec![
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ];

        let context = collect_claude_md_context_with_roots(
            &config,
            &ClaudeMemoryRoots {
                managed_dir: managed,
                user_config_dir: user,
                additional_dirs: Vec::new(),
                additional_dirs_enabled: false,
            },
        )
        .expect("context");

        let parent_index = context.find("project parent").expect("parent");
        let child_index = context.find("TOKEN_CHILD_INCLUDE").expect("child include");
        let outside_index = context
            .find("TOKEN_OUTSIDE_INCLUDE")
            .expect("outside include");
        assert!(
            child_index > parent_index,
            "includes should come after the parent"
        );
        assert!(
            outside_index < parent_index,
            "user external include should load, project external include should not"
        );
        assert!(
            !context.contains("TOKEN_IGNORED_INCLUDE"),
            "fenced code includes should be ignored"
        );
    }
}
