use std::collections::{BTreeMap, HashMap};
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
// Stdio removed — no longer needed after Roo in-process migration
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use claude_config::{
    AppPaths, ProviderConfig as RuntimeProviderConfig, ProviderOverrides, RuntimeConfig,
    RuntimeOverrides, SettingSource, discover_env_providers, load_runtime_config,
    normalize_base_url, validate_provider_config,
};
use claude_core::{
    ConversationEntry, ConversationRole, PermissionMode, ProviderProtocol, ToolCall, UsageSummary,
};
use claude_mcp::{
    DEFAULT_MCP_CONFIG_FILE, McpClientInfo, McpConfig, McpServerConfig, McpServerInspection,
    McpTransport, McpTransportConfig, inspect_server,
};
use claude_permissions::{
    LayeredPermissionBroker, PermissionBroker, PermissionClass, PermissionDecision,
    PermissionRequest, PermissionUpdate, PermissionUpdateDestination, auto_allows, classify_tool,
    load_layered_rules, rules::summarize_rule_sources,
};
use claude_plugins::{PluginBundle, discover_plugins_including_disabled};
use claude_provider::ProviderClient;
use claude_provider::model_info::{ModelCapability, get_model_info};
use claude_session::runtime_context::{
    persist_runtime_config_session_context, restore_runtime_config_session_context,
};
use claude_session::{SessionStore, SessionSummary, conversation::ensure_conversation_initialized};
use claude_skills::discover_skills;
use claude_tools::shell::ShellExecutionPolicy;
use claude_tools::{
    ToolRuntimePolicy, configure_tool_runtime_policy,
    mcp_runtime::{
        RuntimeMcpServerObservation, observe_runtime_mcp_servers, runtime_mcp_inventory_summary,
        runtime_mcp_policy_entries,
    },
    runtime_plan_mode::RuntimePlanModeController,
    tasks::load_persisted_ui_task_snapshots,
};
use claude_ui_bridge::{
    UiProviderStatusSnapshot, UiRuntimeMcpServerStatus, UiRuntimeStatusSnapshot,
};
use codex_app_server_protocol::{
    CancelLoginAccountParams, CommandExecResizeParams, CommandExecTerminalSize,
    CommandExecWriteParams, ConfigBatchWriteParams, ConfigEdit, ConfigValueWriteParams,
    DeviceKeyCreateParams, DeviceKeyPublicParams, DeviceKeySignParams,
    ExternalAgentConfigDetectParams, ExternalAgentConfigImportParams, FsCopyParams,
    FsCreateDirectoryParams, FsGetMetadataParams, FsReadDirectoryParams, FsReadFileParams,
    FsRemoveParams, FsUnwatchParams, FsWatchParams, FsWriteFileParams, FuzzyFileSearchParams,
    FuzzyFileSearchSessionStartParams, FuzzyFileSearchSessionStopParams,
    FuzzyFileSearchSessionUpdateParams, LoginAccountParams, McpServerStatusDetail, MergeStrategy,
    SendAddCreditsNudgeEmailParams, ThreadApproveGuardianDeniedActionParams, ThreadForkParams,
    ThreadInjectItemsParams, ThreadMetadataGitInfoUpdateParams, ThreadMetadataUpdateParams,
    ThreadRealtimeAppendAudioParams, ThreadRealtimeAppendTextParams, ThreadRealtimeStartParams,
    ThreadRealtimeStopParams, ThreadResumeParams, ThreadStartParams, TurnStartParams,
    WindowsSandboxSetupStartParams,
};
use rc_agent_protocol::AgentAdapter;
use rc_agent_protocol::permission::PermissionDecision as AgentPermissionDecision;
use rc_agent_protocol::types::AgentType as ProtocolAgentType;
use rc_claude_adapter::ClaudeInProcessAdapter;
use rc_codex_adapter::{
    CodexAdapterOptions, CodexExecRequest, CodexFeedbackRequest, CodexInProcessAdapter,
    CodexPluginRefRequest, CodexServerRequestResolution, CodexThreadListRequest,
    CodexThreadRollbackRequest, CodexTurnInterruptRequest, CodexTurnSteerRequest,
};
use rc_roo_adapter::RooInProcessAdapter;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{Mutex, mpsc, oneshot};
// TokioCommand removed — no longer needed after Roo in-process migration
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::dto::*;
use crate::state::*;

mod agent_routing;
mod bootstrap;
mod codex_commands;
mod mcp_commands;
mod permission_commands;
mod project_commands;
mod provider_commands;
mod session_commands;

pub use bootstrap::run;

fn usage_info_from_codex_token_usage(
    value: &serde_json::Value,
) -> Option<rc_agent_protocol::events::UsageInfo> {
    let params = value.get("params").unwrap_or(value);
    let total = params.get("tokenUsage")?.get("total")?;
    Some(rc_agent_protocol::events::UsageInfo {
        input_tokens: total
            .get("inputTokens")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0),
        output_tokens: total
            .get("outputTokens")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0),
        cache_read: total
            .get("cachedInputTokens")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0),
        cache_write: 0,
    })
}

fn gui_storage_path(paths: &AppPaths, file_name: &str) -> PathBuf {
    paths.profile_dir.join(file_name)
}

fn profile_override_from_env() -> Option<PathBuf> {
    env::var("REMOTE_CODE_PROFILE_DIR")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn load_json_file<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let parsed = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(parsed)
}

fn save_json_file<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let contents = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn load_projects(paths: &AppPaths) -> Result<Vec<ProjectEntry>> {
    let file: ProjectListFile = load_json_file(&gui_storage_path(paths, PROJECTS_FILE_NAME))?;
    Ok(normalize_project_entries(file.projects))
}

fn save_projects(paths: &AppPaths, projects: &[ProjectEntry]) -> Result<()> {
    let file = ProjectListFile {
        projects: normalize_project_entries(projects.to_vec()),
    };
    save_json_file(&gui_storage_path(paths, PROJECTS_FILE_NAME), &file)
}

fn load_provider_configs(paths: &AppPaths) -> Result<ProviderConfigList> {
    load_json_file(&gui_storage_path(paths, PROVIDERS_FILE_NAME))
}

fn save_provider_configs(paths: &AppPaths, configs: &ProviderConfigList) -> Result<()> {
    save_json_file(&gui_storage_path(paths, PROVIDERS_FILE_NAME), configs)
}

fn load_gui_settings(paths: &AppPaths) -> Result<GuiSettingsFile> {
    load_json_file(&gui_storage_path(paths, SETTINGS_FILE_NAME))
}

fn save_gui_settings(paths: &AppPaths, settings: &GuiSettingsFile) -> Result<()> {
    save_json_file(&gui_storage_path(paths, SETTINGS_FILE_NAME), settings)
}

fn parse_protocol(value: Option<&str>) -> Option<ProviderProtocol> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "openai" | "open-ai" | "open_ai" => Some(ProviderProtocol::OpenAi),
        "anthropic" | "claude" => Some(ProviderProtocol::Anthropic),
        "bedrock" | "aws" | "amazon" => Some(ProviderProtocol::Bedrock),
        "vertex" | "google" | "gemini" => Some(ProviderProtocol::Vertex),
        _ => None,
    }
}

fn parse_permission_mode(value: Option<&str>) -> Option<PermissionMode> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "default" | "suggest" => Some(PermissionMode::Default),
        "acceptedits" | "accept-edits" | "accept_edits" | "auto-edit" | "auto_edit" => {
            Some(PermissionMode::AcceptEdits)
        }
        "dontask" | "dont-ask" | "dont_ask" => Some(PermissionMode::DontAsk),
        "bypasspermissions" | "bypass-permissions" | "bypass_permissions" | "full-auto"
        | "full_auto" | "yolo" => Some(PermissionMode::BypassPermissions),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

fn normalize_provider_config(input: ProviderConfig) -> Result<ProviderConfig> {
    let name = input.name.trim().to_owned();
    if name.is_empty() {
        return Err(anyhow!("provider name cannot be empty"));
    }
    let protocol = parse_protocol(Some(&input.protocol)).unwrap_or(ProviderProtocol::OpenAi);
    let base_url = normalize_base_url(trimmed_option(input.base_url), protocol);
    let api_key = trimmed_option(input.api_key);
    let model = trimmed_option(input.model);
    Ok(ProviderConfig {
        name,
        protocol: protocol.as_str().to_owned(),
        base_url,
        api_key,
        model,
        profiles: input.profiles,
        active_profile: input.active_profile,
        api_key_stored: false,
    })
}

fn trimmed_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn strip_windows_unc_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if cfg!(windows) && raw.starts_with(r"\\?\") {
        PathBuf::from(raw.trim_start_matches(r"\\?\"))
    } else {
        path
    }
}

fn normalize_existing_path(path: &Path) -> Result<PathBuf> {
    let path = if path.exists() {
        std::fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    Ok(strip_windows_unc_prefix(path))
}

fn path_identity(path: &Path) -> String {
    let raw = if cfg!(windows) {
        path.to_string_lossy().replace('/', "\\")
    } else {
        path.to_string_lossy().into_owned()
    };
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let normalized = raw.trim_end_matches(separator).to_owned();
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn normalize_project_entries(projects: Vec<ProjectEntry>) -> Vec<ProjectEntry> {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for project in projects {
        let normalized_path =
            normalize_existing_path(&project.path).unwrap_or_else(|_| project.path.clone());
        let key = path_identity(&normalized_path);
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let fallback_name = normalized_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("project")
            .to_owned();
        let name = project.name.trim();
        deduped.push(ProjectEntry {
            path: normalized_path,
            name: if name.is_empty() {
                fallback_name
            } else {
                name.to_owned()
            },
        });
    }

    deduped.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    deduped
}

fn project_entry_from_path(path: &Path) -> ProjectEntry {
    let normalized_path = normalize_existing_path(path).unwrap_or_else(|_| path.to_path_buf());
    let fallback_name = normalized_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("project")
        .to_owned();
    ProjectEntry {
        path: normalized_path,
        name: fallback_name,
    }
}

fn ensure_sessions_have_projects(
    projects: &mut Vec<ProjectEntry>,
    sessions: &[SessionSummary],
) -> bool {
    let mut merged = projects.clone();
    let mut seen = merged
        .iter()
        .map(|project| path_identity(&project.path))
        .collect::<std::collections::HashSet<_>>();
    let mut changed = false;

    for session in sessions {
        let normalized_path =
            normalize_existing_path(&session.cwd).unwrap_or_else(|_| session.cwd.clone());
        let key = path_identity(&normalized_path);
        if seen.insert(key) {
            merged.push(project_entry_from_path(&normalized_path));
            changed = true;
        }
    }

    if changed {
        *projects = normalize_project_entries(merged);
    }

    changed
}

fn ensure_project_entry(projects: &mut Vec<ProjectEntry>, path: &Path) -> bool {
    let normalized_path = normalize_existing_path(path).unwrap_or_else(|_| path.to_path_buf());
    let key = path_identity(&normalized_path);
    if projects
        .iter()
        .any(|project| path_identity(&project.path) == key)
    {
        return false;
    }
    let mut merged = projects.clone();
    merged.push(project_entry_from_path(&normalized_path));
    *projects = normalize_project_entries(merged);
    true
}

fn project_session_count(project_path: &Path, sessions: &[SessionSummary]) -> usize {
    let key = path_identity(project_path);
    sessions
        .iter()
        .filter(|summary| path_identity(&summary.cwd) == key)
        .count()
}

fn provider_info_from_runtime(provider: &RuntimeProviderConfig) -> ProviderInfoDto {
    ProviderInfoDto {
        name: provider.name.clone(),
        model: provider.model.clone(),
        protocol: provider.protocol.as_str().to_owned(),
        base_url: provider.base_url.clone(),
    }
}

fn runtime_status_snapshot_from_config(config: &RuntimeConfig) -> UiRuntimeStatusSnapshot {
    UiRuntimeStatusSnapshot {
        session_name: config.session_name.clone(),
        provider: UiProviderStatusSnapshot {
            name: config.provider.name.clone(),
            model: config.provider.model.clone(),
            protocol: config.provider.protocol.as_str().to_owned(),
            base_url: config.provider.base_url.clone(),
            auth_source: config.auth_source.clone(),
            effort: config.effort.clone(),
            fallback_model: config.fallback_model.clone(),
        },
        permission_mode: config.permission_mode.as_legacy_str().to_owned(),
        output_style: config.output_style.clone(),
        language: config.language.clone(),
        brief_enabled: config.brief_enabled,
        proactive_active: config.proactive_active,
        setting_sources: config.setting_sources.clone(),
        allowed_setting_sources: config
            .allowed_setting_sources
            .iter()
            .map(|source| source.as_str().to_owned())
            .collect(),
        allowed_tools: config.allowed_tools.clone(),
        disallowed_tools: config.disallowed_tools.clone(),
        mcp: runtime_mcp_inventory_summary(config, &[]),
    }
}

fn emit_runtime_status(app: &AppHandle, config: &RuntimeConfig) {
    let _ = app.emit(
        APP_EVENT_RUNTIME_STATUS,
        runtime_status_snapshot_from_config(config),
    );
}

fn repository_slug() -> Option<String> {
    let repository = env!("CARGO_PKG_REPOSITORY").trim();
    let repository = repository
        .strip_suffix(".git")
        .unwrap_or(repository)
        .trim_end_matches('/');
    if let Some(stripped) = repository.strip_prefix("https://github.com/") {
        return Some(stripped.to_owned());
    }
    if let Some(stripped) = repository.strip_prefix("http://github.com/") {
        return Some(stripped.to_owned());
    }
    repository
        .strip_prefix("git@github.com:")
        .map(ToOwned::to_owned)
}

fn provider_endpoint_url(provider: &RuntimeProviderConfig) -> Option<String> {
    provider
        .base_url
        .clone()
        .or_else(|| match provider.protocol {
            ProviderProtocol::Anthropic => Some("https://api.anthropic.com/v1/messages".to_owned()),
            ProviderProtocol::OpenAi => {
                Some("https://api.openai.com/v1/chat/completions".to_owned())
            }
            ProviderProtocol::Bedrock | ProviderProtocol::Vertex => None,
        })
}

fn classify_probe_status(status: reqwest::StatusCode) -> (GuiDoctorProbeOutcomeDto, String) {
    let code = status.as_u16();
    if status.is_success() {
        return (
            GuiDoctorProbeOutcomeDto::Reachable,
            format!("HTTP {code} confirms the endpoint is reachable"),
        );
    }

    match code {
        400 | 404 | 405 | 406 | 409 | 415 | 422 => (
            GuiDoctorProbeOutcomeDto::Reachable,
            format!("HTTP {code} confirms the endpoint is reachable"),
        ),
        401 | 403 => (
            GuiDoctorProbeOutcomeDto::AuthRejected,
            format!("HTTP {code} indicates the endpoint rejected the supplied credentials"),
        ),
        429 => (
            GuiDoctorProbeOutcomeDto::RateLimited,
            "HTTP 429 indicates the endpoint is reachable but currently rate limited".to_owned(),
        ),
        500..=599 => (
            GuiDoctorProbeOutcomeDto::ServerError,
            format!("HTTP {code} indicates an upstream server failure"),
        ),
        _ => (
            GuiDoctorProbeOutcomeDto::Reachable,
            format!("HTTP {code} returned from the endpoint"),
        ),
    }
}

fn probe_is_issue(probe: &GuiDoctorProbeDto) -> bool {
    matches!(
        probe.outcome,
        GuiDoctorProbeOutcomeDto::AuthRejected
            | GuiDoctorProbeOutcomeDto::ServerError
            | GuiDoctorProbeOutcomeDto::TransportError
    )
}

fn probe_is_warning(probe: &GuiDoctorProbeDto) -> bool {
    matches!(probe.outcome, GuiDoctorProbeOutcomeDto::RateLimited)
}

async fn run_doctor_probe(
    label: impl Into<String>,
    url: impl Into<String>,
    headers: &BTreeMap<String, String>,
) -> GuiDoctorProbeDto {
    let label = label.into();
    let url = url.into();
    let client = match reqwest::Client::builder()
        .user_agent("remote-code-gui-doctor")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return GuiDoctorProbeDto {
                label,
                url,
                outcome: GuiDoctorProbeOutcomeDto::TransportError,
                status_code: None,
                latency_ms: 0,
                detail: format!("failed to build HTTP client: {error}"),
            };
        }
    };

    let started = Instant::now();
    let mut request = client.get(&url);
    for (name, value) in headers {
        request = request.header(name, value);
    }

    match request.send().await {
        Ok(response) => {
            let (outcome, detail) = classify_probe_status(response.status());
            GuiDoctorProbeDto {
                label,
                url,
                outcome,
                status_code: Some(response.status().as_u16()),
                latency_ms: started.elapsed().as_millis(),
                detail,
            }
        }
        Err(error) => GuiDoctorProbeDto {
            label,
            url,
            outcome: GuiDoctorProbeOutcomeDto::TransportError,
            status_code: None,
            latency_ms: started.elapsed().as_millis(),
            detail: error.to_string(),
        },
    }
}

fn count_managed_mcp_servers(path: &Path, warnings: &mut Vec<String>) -> usize {
    if !path.exists() {
        return 0;
    }
    match McpConfig::load(path) {
        Ok(config) => config.servers.len(),
        Err(error) => {
            warnings.push(format!(
                "Failed to load MCP config {}: {error}",
                path.display()
            ));
            0
        }
    }
}

fn count_plugin_mcp_servers(plugins: &[PluginBundle], warnings: &mut Vec<String>) -> usize {
    let mut count = 0usize;
    for plugin in plugins {
        let Some(path) = plugin.mcp_config_path() else {
            continue;
        };
        match McpConfig::load(&path) {
            Ok(config) => count += config.servers.len(),
            Err(error) => warnings.push(format!(
                "Failed to load plugin MCP config for {}: {error}",
                plugin.manifest.name
            )),
        }
    }
    count
}

async fn build_gui_doctor_report(
    config: &RuntimeConfig,
    probe_network: bool,
    probe_provider: bool,
    probe_mcp: bool,
    include_env_providers: bool,
) -> Result<GuiDoctorReportDto> {
    let validation = validate_provider_config(&config.provider);
    let model_info = get_model_info(config.provider.model.as_deref().unwrap_or("unknown"));
    let layered_rules = load_layered_rules(
        &config.cwd,
        &config.paths.profile_dir,
        &config.settings_files,
        &config.cli_settings_files,
    );
    let mcp_runtime = observe_runtime_mcp_servers(
        config,
        &[],
        probe_mcp,
        &McpClientInfo::new("remote-code-gui", env!("CARGO_PKG_VERSION")),
    )
    .await;

    let mut warnings = Vec::new();
    let mut issues = validation.issues.clone();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);
    let project_sources_enabled = setting_source_enabled(config, SettingSource::Project);

    let skills = if user_sources_enabled {
        match discover_skills(&config.paths.skills_dir) {
            Ok(skills) => skills,
            Err(error) => {
                warnings.push(format!("Failed to discover skills: {error}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let all_plugins = if user_sources_enabled {
        match discover_plugins_including_disabled(&config.paths.plugins_dir) {
            Ok(plugins) => plugins,
            Err(error) => {
                warnings.push(format!("Failed to discover plugins: {error}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let disabled_plugins = all_plugins
        .iter()
        .filter(|plugin| plugin.is_disabled())
        .count();
    let plugins = all_plugins
        .into_iter()
        .filter(|plugin| !plugin.is_disabled())
        .collect::<Vec<_>>();
    let managed_mcp_servers = if user_sources_enabled {
        count_managed_mcp_servers(
            &config.paths.profile_dir.join(DEFAULT_MCP_CONFIG_FILE),
            &mut warnings,
        )
    } else {
        0
    } + if project_sources_enabled {
        count_managed_mcp_servers(&config.cwd.join(DEFAULT_MCP_CONFIG_FILE), &mut warnings)
    } else {
        0
    };
    let plugin_mcp_servers = if user_sources_enabled {
        count_plugin_mcp_servers(&plugins, &mut warnings)
    } else {
        0
    };
    extend_unique_strings(&mut warnings, mcp_runtime.warnings.clone());

    let provider_probe = if probe_provider {
        if let Some(url) = provider_endpoint_url(&config.provider) {
            let mut headers = config.provider.request_header_overrides.clone();
            match config.provider.protocol {
                ProviderProtocol::Anthropic => {
                    headers.insert("anthropic-version".to_owned(), "2023-06-01".to_owned());
                    if let Some(api_key) = &config.provider.api_key {
                        headers.insert("x-api-key".to_owned(), api_key.clone());
                    }
                }
                ProviderProtocol::OpenAi => {
                    if let Some(api_key) = &config.provider.api_key {
                        headers.insert("authorization".to_owned(), format!("Bearer {api_key}"));
                    }
                }
                ProviderProtocol::Bedrock | ProviderProtocol::Vertex => {}
            }
            let probe =
                run_doctor_probe(format!("provider:{}", config.provider.name), url, &headers).await;
            if probe_is_issue(&probe) {
                issues.push(format!("Provider probe failed: {}", probe.detail));
            } else if probe_is_warning(&probe) {
                warnings.push(format!("Provider probe warning: {}", probe.detail));
            }
            Some(probe)
        } else {
            warnings.push(
                "Provider probe skipped: no probeable endpoint for the active protocol.".to_owned(),
            );
            None
        }
    } else {
        None
    };

    let mut network = Vec::new();
    if probe_network {
        if let Some(slug) = repository_slug() {
            let github_probe = run_doctor_probe(
                "github:releases",
                format!("https://api.github.com/repos/{slug}/releases/latest"),
                &BTreeMap::new(),
            )
            .await;
            if probe_is_warning(&github_probe) || probe_is_issue(&github_probe) {
                warnings.push(format!("Network probe warning: {}", github_probe.detail));
            }
            network.push(github_probe);
        }
        if !probe_provider && let Some(url) = provider_endpoint_url(&config.provider) {
            let provider_network_probe =
                run_doctor_probe("provider:network", url, &BTreeMap::new()).await;
            if probe_is_warning(&provider_network_probe) || probe_is_issue(&provider_network_probe)
            {
                warnings.push(format!(
                    "Network probe warning: {}",
                    provider_network_probe.detail
                ));
            }
            network.push(provider_network_probe);
        }
    }

    let env_providers = if include_env_providers {
        discover_env_providers()
            .into_iter()
            .map(|provider| GuiDoctorEnvProviderDto {
                name: provider.name,
                protocol: provider.protocol.as_str().to_owned(),
                base_url: provider.base_url,
                model: provider.model,
                api_key_present: provider.api_key.is_some(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mcp_runtime = GuiDoctorMcpRuntimeDto {
        probed: probe_mcp,
        summary: mcp_runtime.inventory_summary(),
        servers: mcp_runtime
            .servers
            .into_iter()
            .map(|server| GuiDoctorMcpRuntimeServerDto {
                name: server.entry.server.name,
                status: server.status,
                enabled: server.entry.server.enabled,
                origin_kind: server.entry.origin_kind.to_owned(),
                origin_name: server.entry.origin_name,
                config_path: server.entry.config_path.display().to_string(),
                tool_count: server
                    .inspection
                    .as_ref()
                    .map_or(0, |inspection| inspection.tools.len()),
                error: server.error,
            })
            .collect(),
    };

    Ok(GuiDoctorReportDto {
        ok: issues.is_empty(),
        runtime: GuiDoctorRuntimeDto {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            cwd: config.cwd.display().to_string(),
            profile_dir: config.paths.profile_dir.display().to_string(),
            session_id: config.session_id.to_string(),
            session_name: config.session_name.clone(),
            permission_mode: config.permission_mode.as_legacy_str().to_owned(),
            setting_sources: config.setting_sources.clone(),
            allowed_setting_sources: config
                .allowed_setting_sources
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect(),
            settings_files: config
                .settings_files
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        },
        provider: GuiDoctorProviderDto {
            name: config.provider.name.clone(),
            protocol: config.provider.protocol.as_str().to_owned(),
            base_url: config.provider.base_url.clone(),
            model: config.provider.model.clone(),
            api_key_present: config.provider.api_key.is_some(),
            auth_source: config.auth_source.clone(),
            effort: config.effort.clone(),
            fallback_model: config.fallback_model.clone(),
            context_window_tokens: model_info.max_context,
            output_reserve_tokens: model_info.max_output,
            multimodal: model_info.multimodal,
            reasoning: model_info
                .capabilities
                .contains(&ModelCapability::Reasoning),
            validation_ok: validation.ok,
            validation_issues: validation.issues,
            probe: provider_probe,
        },
        tools: GuiDoctorToolsDto {
            builtin_tools: claude_tools::runtime_builtin_tool_specs().len(),
            allowed_tools: config.allowed_tools.clone(),
            disallowed_tools: config.disallowed_tools.clone(),
        },
        permissions: GuiDoctorPermissionsDto {
            layered_rules: layered_rules.len(),
            rule_sources: summarize_rule_sources(&layered_rules)
                .into_iter()
                .map(|(source, count)| GuiDoctorRuleSourceDto {
                    source: source.as_str().to_owned(),
                    count,
                })
                .collect(),
        },
        extensions: GuiDoctorExtensionsDto {
            skills: skills.len(),
            plugins: plugins.len(),
            disabled_plugins,
            managed_mcp_servers,
            plugin_mcp_servers,
        },
        mcp_runtime,
        network,
        env_providers,
        issues,
        warnings,
    })
}

fn extend_unique_strings(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

fn mcp_config_path_for_scope(
    config: &RuntimeConfig,
    scope: ConfigScopeDto,
    project_path: Option<&str>,
    projects: &[ProjectEntry],
) -> Result<PathBuf> {
    match scope {
        ConfigScopeDto::Profile => Ok(config.paths.profile_dir.join(DEFAULT_MCP_CONFIG_FILE)),
        ConfigScopeDto::Project => {
            let project_path = project_path.ok_or_else(|| {
                anyhow!("project path is required for project-scope MCP management")
            })?;
            let project_root = normalize_existing_path(Path::new(project_path))?;
            // Validate that the project_path is a known managed project.
            let key = path_identity(&project_root);
            if !projects
                .iter()
                .any(|project| path_identity(&project.path) == key)
            {
                anyhow::bail!(
                    "project path is not a managed project — MCP config writes are restricted to known projects"
                );
            }
            Ok(project_root.join(DEFAULT_MCP_CONFIG_FILE))
        }
    }
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn mcp_scope_enabled(config: &RuntimeConfig, scope: ConfigScopeDto) -> bool {
    match scope {
        ConfigScopeDto::Profile => setting_source_enabled(config, SettingSource::User),
        ConfigScopeDto::Project => setting_source_enabled(config, SettingSource::Project),
    }
}

fn load_managed_mcp_config_or_default(path: &Path) -> Result<McpConfig> {
    if path.exists() {
        Ok(McpConfig::load(path)?)
    } else {
        Ok(McpConfig::default())
    }
}

fn mcp_live_to_dto(inspection: McpServerInspection) -> McpServerLiveDto {
    McpServerLiveDto {
        status: UiRuntimeMcpServerStatus::Connected.as_str().to_owned(),
        protocol_version: Some(inspection.protocol_version),
        peer_name: inspection
            .server_info
            .as_ref()
            .map(|info| info.name.clone()),
        peer_version: inspection
            .server_info
            .as_ref()
            .and_then(|info| info.version.clone()),
        tool_count: inspection.tools.len(),
        tools: inspection
            .tools
            .into_iter()
            .map(|tool| McpToolInfoDto {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
            })
            .collect(),
        error: None,
    }
}

fn runtime_mcp_failed_live_to_dto(
    status: UiRuntimeMcpServerStatus,
    error: String,
) -> McpServerLiveDto {
    McpServerLiveDto {
        status: status.as_str().to_owned(),
        protocol_version: None,
        peer_name: None,
        peer_version: None,
        tool_count: 0,
        tools: Vec::new(),
        error: Some(error),
    }
}

fn mcp_transport_to_display(transport: McpTransport) -> String {
    match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Http => "http",
        McpTransport::WebSocket => "websocket",
    }
    .to_owned()
}

/// Transport fields extracted from an MCP server config for display.
struct McpTransportFields {
    command: Option<String>,
    url: Option<String>,
    args: Vec<String>,
    cwd: Option<String>,
    env_keys: Vec<String>,
}

fn mcp_server_transport_fields(server: &McpServerConfig) -> McpTransportFields {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut env_keys = env.keys().cloned().collect::<Vec<_>>();
            env_keys.sort();
            McpTransportFields {
                command: Some(command.clone()),
                url: None,
                args: args.clone(),
                cwd: cwd.as_ref().map(|path| path.display().to_string()),
                env_keys,
            }
        }
        McpTransportConfig::Http { url, headers, .. }
        | McpTransportConfig::WebSocket { url, headers, .. }
        | McpTransportConfig::Sse { url, headers, .. } => {
            let mut env_keys = headers.keys().cloned().collect::<Vec<_>>();
            env_keys.sort();
            McpTransportFields {
                command: None,
                url: Some(url.clone()),
                args: Vec::new(),
                cwd: None,
                env_keys,
            }
        }
        McpTransportConfig::SseIde { url, .. } | McpTransportConfig::WsIde { url, .. } => {
            McpTransportFields {
                command: None,
                url: Some(url.clone()),
                args: Vec::new(),
                cwd: None,
                env_keys: Vec::new(),
            }
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            McpTransportFields {
                command: None,
                url: None,
                args: Vec::new(),
                cwd: None,
                env_keys: Vec::new(),
            }
        }
    }
}

fn mcp_server_to_dto(
    config_path: &Path,
    server: McpServerConfig,
    live: Option<McpServerLiveDto>,
) -> McpServerDto {
    let fields = mcp_server_transport_fields(&server);
    let mut metadata_keys = server.metadata.keys().cloned().collect::<Vec<_>>();
    metadata_keys.sort();
    McpServerDto {
        name: server.name,
        enabled: server.enabled,
        transport: mcp_transport_to_display(server.transport.kind()),
        config_path: config_path.display().to_string(),
        command: fields.command,
        url: fields.url,
        args: fields.args,
        cwd: fields.cwd,
        env_keys: fields.env_keys,
        metadata_keys,
        startup_timeout_secs: server.startup_timeout_secs,
        request_timeout_secs: server.request_timeout_secs,
        live,
    }
}

fn runtime_mcp_server_to_dto(observation: RuntimeMcpServerObservation) -> RuntimeMcpServerDto {
    let entry = observation.entry;
    let fields = mcp_server_transport_fields(&entry.server);
    let mut metadata_keys = entry.server.metadata.keys().cloned().collect::<Vec<_>>();
    metadata_keys.sort();
    let live = match (observation.inspection, observation.error) {
        (Some(inspection), _) => Some(mcp_live_to_dto(inspection)),
        (None, Some(error)) => Some(runtime_mcp_failed_live_to_dto(observation.status, error)),
        (None, None) => None,
    };
    RuntimeMcpServerDto {
        name: entry.server.name.clone(),
        status: observation.status.as_str().to_owned(),
        enabled: entry.server.enabled,
        origin_kind: entry.origin_kind.to_owned(),
        origin_name: entry.origin_name,
        config_path: entry.config_path.display().to_string(),
        transport: mcp_transport_to_display(entry.server.transport.kind()),
        command: fields.command,
        url: fields.url,
        args: fields.args,
        cwd: fields.cwd,
        env_keys: fields.env_keys,
        metadata_keys,
        startup_timeout_secs: entry.server.startup_timeout_secs,
        request_timeout_secs: entry.server.request_timeout_secs,
        live,
    }
}

async fn build_mcp_server_list(
    config: &RuntimeConfig,
    scope: ConfigScopeDto,
    project_path: Option<&str>,
    projects: &[ProjectEntry],
    connect: bool,
    include_disabled: bool,
) -> Result<McpServerListDto> {
    let config_path = mcp_config_path_for_scope(config, scope, project_path, projects)?;
    if !mcp_scope_enabled(config, scope) {
        return Ok(McpServerListDto {
            scope: scope.label().to_owned(),
            config_path: config_path.display().to_string(),
            warnings: vec![format!(
                "{} MCP discovery is disabled by active setting sources",
                scope.label()
            )],
            servers: Vec::new(),
        });
    }
    let mcp_config = load_managed_mcp_config_or_default(&config_path)?;
    let mut warnings = Vec::new();
    let mut servers = Vec::new();

    for server in mcp_config.servers.into_values() {
        if !server.enabled && !include_disabled {
            continue;
        }
        let live = if connect {
            match inspect_server(
                &server,
                &McpClientInfo::new("remote-code-gui", env!("CARGO_PKG_VERSION")),
            )
            .await
            {
                Ok(inspection) => Some(mcp_live_to_dto(inspection)),
                Err(error) => Some(McpServerLiveDto {
                    status: UiRuntimeMcpServerStatus::Failed.as_str().to_owned(),
                    protocol_version: None,
                    peer_name: None,
                    peer_version: None,
                    tool_count: 0,
                    tools: Vec::new(),
                    error: Some(error.to_string()),
                }),
            }
        } else {
            None
        };
        servers.push(mcp_server_to_dto(&config_path, server, live));
    }

    servers.sort_by(|left, right| left.name.cmp(&right.name));
    if !config_path.exists() {
        warnings.push(format!(
            "Managed MCP config does not exist yet at {}",
            config_path.display()
        ));
    }

    Ok(McpServerListDto {
        scope: scope.label().to_owned(),
        config_path: config_path.display().to_string(),
        warnings,
        servers,
    })
}

async fn build_runtime_mcp_inventory(
    config: &RuntimeConfig,
    project_path: Option<&str>,
    _projects: &[ProjectEntry],
    connect: bool,
    include_disabled: bool,
) -> Result<RuntimeMcpInventoryDto> {
    let mut effective_config = config.clone();
    if let Some(project_path) = project_path.filter(|value| !value.trim().is_empty()) {
        effective_config.cwd = normalize_existing_path(Path::new(project_path))?;
    }

    let observation = observe_runtime_mcp_servers(
        &effective_config,
        &[],
        connect,
        &McpClientInfo::new("remote-code-gui", env!("CARGO_PKG_VERSION")),
    )
    .await;
    let summary = observation.inventory_summary();
    let mut servers = Vec::new();
    for server in observation.servers {
        if !server.entry.server.enabled && !include_disabled {
            continue;
        }
        servers.push(runtime_mcp_server_to_dto(server));
    }

    servers.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.origin_kind.cmp(&right.origin_kind))
            .then_with(|| left.origin_name.cmp(&right.origin_name))
            .then_with(|| left.config_path.cmp(&right.config_path))
    });

    Ok(RuntimeMcpInventoryDto {
        effective_cwd: effective_config.cwd.display().to_string(),
        warnings: observation.warnings,
        summary,
        servers,
    })
}

fn build_mcp_transport(request: &McpServerUpsertRequestDto) -> Result<McpTransportConfig> {
    match request.transport.trim().to_ascii_lowercase().as_str() {
        "stdio" => {
            let command = request
                .command
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("stdio MCP servers require a command"))?;
            Ok(McpTransportConfig::Stdio {
                command: command.to_owned(),
                args: request.args.clone(),
                cwd: request.cwd.as_deref().map(PathBuf::from),
                env: request.env.clone(),
            })
        }
        "http" => {
            let url = request
                .url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("http MCP servers require a url"))?;
            Ok(McpTransportConfig::Http {
                url: url.to_owned(),
                headers: request.headers.clone(),
                headers_helper: None,
            })
        }
        "websocket" | "ws" => {
            let url = request
                .url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("websocket MCP servers require a url"))?;
            Ok(McpTransportConfig::WebSocket {
                url: url.to_owned(),
                headers: request.headers.clone(),
                headers_helper: None,
            })
        }
        other => Err(anyhow!("unsupported MCP transport: {other}")),
    }
}

fn save_managed_mcp_server_at_path(
    config_path: &Path,
    scope: ConfigScopeDto,
    request: &McpServerUpsertRequestDto,
) -> Result<McpMutationResultDto> {
    let mut mcp_config = load_managed_mcp_config_or_default(config_path)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(anyhow!("MCP server name cannot be empty."));
    }
    let existed = mcp_config.servers.contains_key(name);
    let transport = build_mcp_transport(request)?;
    mcp_config.servers.insert(
        name.to_owned(),
        McpServerConfig {
            name: name.to_owned(),
            enabled: !request.disabled,
            transport,
            capabilities: claude_mcp::McpCapabilityMatrix::default(),
            startup_timeout_secs: request.startup_timeout_secs,
            request_timeout_secs: request.request_timeout_secs,
            metadata: request.metadata.clone(),
            oauth: None,
            tool_policy: claude_mcp::McpToolPolicy::default(),
        },
    );
    mcp_config.save(config_path)?;
    Ok(McpMutationResultDto {
        status: if existed { "updated" } else { "created" }.to_owned(),
        scope: scope.label().to_owned(),
        config_path: config_path.display().to_string(),
        name: Some(name.to_owned()),
        enabled: Some(!request.disabled),
    })
}

fn toggle_managed_mcp_server_at_path(
    config_path: &Path,
    scope: ConfigScopeDto,
    name: &str,
    enabled: bool,
    if_exists: bool,
) -> Result<McpMutationResultDto> {
    let mut mcp_config = load_managed_mcp_config_or_default(config_path)?;
    let name = name.trim().to_owned();
    let Some(server) = mcp_config.servers.get_mut(&name) else {
        if if_exists {
            return Ok(McpMutationResultDto {
                status: "noop".to_owned(),
                scope: scope.label().to_owned(),
                config_path: config_path.display().to_string(),
                name: Some(name),
                enabled: Some(enabled),
            });
        }
        return Err(anyhow!(
            "No MCP server named `{}` exists in {}",
            name,
            config_path.display()
        ));
    };

    let status = if server.enabled == enabled {
        "noop"
    } else {
        server.enabled = enabled;
        mcp_config.save(config_path)?;
        if enabled { "enabled" } else { "disabled" }
    };

    Ok(McpMutationResultDto {
        status: status.to_owned(),
        scope: scope.label().to_owned(),
        config_path: config_path.display().to_string(),
        name: Some(name),
        enabled: Some(enabled),
    })
}

fn remove_managed_mcp_server_at_path(
    config_path: &Path,
    scope: ConfigScopeDto,
    name: &str,
    if_exists: bool,
) -> Result<McpMutationResultDto> {
    let mut mcp_config = load_managed_mcp_config_or_default(config_path)?;
    let name = name.trim().to_owned();
    let removed = mcp_config.servers.remove(&name);
    if removed.is_none() && !if_exists {
        return Err(anyhow!(
            "No MCP server named `{}` exists in {}",
            name,
            config_path.display()
        ));
    }
    mcp_config.save(config_path)?;
    Ok(McpMutationResultDto {
        status: if removed.is_some() { "removed" } else { "noop" }.to_owned(),
        scope: scope.label().to_owned(),
        config_path: config_path.display().to_string(),
        name: Some(name),
        enabled: None,
    })
}

fn reset_managed_mcp_config_at_path(
    config_path: &Path,
    scope: ConfigScopeDto,
    if_exists: bool,
) -> Result<McpMutationResultDto> {
    let status = if config_path.exists() {
        std::fs::remove_file(config_path)?;
        "reset"
    } else if if_exists {
        "noop"
    } else {
        return Err(anyhow!(
            "Managed MCP config {} does not exist",
            config_path.display()
        ));
    };

    Ok(McpMutationResultDto {
        status: status.to_owned(),
        scope: scope.label().to_owned(),
        config_path: config_path.display().to_string(),
        name: None,
        enabled: None,
    })
}

fn export_session_bundle_for_store(
    store: &SessionStore,
    session_id: Uuid,
    format: SessionExportFormatDto,
) -> Result<SessionExportResultDto> {
    let path = match format {
        SessionExportFormatDto::Json => store.export_session_bundle_json(session_id, None),
        SessionExportFormatDto::Ndjson => store.export_session(session_id, None),
    }?;

    Ok(SessionExportResultDto {
        session_id: session_id.to_string(),
        format: format.label().to_owned(),
        path: path.display().to_string(),
    })
}

fn usage_to_dto(usage: &UsageSummary) -> UsageDto {
    UsageDto {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.input_tokens + usage.output_tokens,
    }
}

fn task_dir_for_paths(paths: &AppPaths, session_id: Uuid) -> PathBuf {
    paths
        .artifacts_dir
        .join("tasks")
        .join(session_id.to_string())
}

fn shell_output_dir_for_paths(paths: &AppPaths, session_id: Uuid) -> PathBuf {
    paths
        .artifacts_dir
        .join("shell")
        .join(session_id.to_string())
}

fn configure_runtime_policy_for_config(config: &RuntimeConfig) -> Result<()> {
    let session_dir = config
        .paths
        .sessions_dir
        .join(config.session_id.to_string());
    configure_tool_runtime_policy(ToolRuntimePolicy {
        allowed_tools: config.allowed_tools.clone(),
        disallowed_tools: config.disallowed_tools.clone(),
        task_output_dir: Some(task_dir_for_paths(&config.paths, config.session_id)),
        tasks_dir: Some(claude_tools::tasks::task_list_base_dir()),
        tool_results_dir: Some(session_dir.join("tool-results")),
        mcp_servers: runtime_mcp_policy_entries(config, &[]),
        shell_policy: ShellExecutionPolicy {
            block_inline_cwd: true,
            allow_background: true,
            block_destructive_git: true,
            max_capture_chars: ShellExecutionPolicy::default().max_capture_chars,
            output_dir: Some(shell_output_dir_for_paths(&config.paths, config.session_id)),
            tool_results_dir: Some(session_dir.join("tool-results")),
            task_output_dir: Some(task_dir_for_paths(&config.paths, config.session_id)),
        },
    })
}

fn ui_task_node_to_dto(session_id: &str, task: claude_ui_bridge::UiTaskNode) -> SessionTaskDto {
    SessionTaskDto {
        session_id: session_id.to_owned(),
        task_id: task.id,
        parent_task_id: task.parent_task_id,
        description: task.title,
        depth: task.depth,
        status: match task.status {
            claude_ui_bridge::UiTaskStatus::Pending => "pending",
            claude_ui_bridge::UiTaskStatus::Running => "running",
            claude_ui_bridge::UiTaskStatus::Completed => "completed",
            claude_ui_bridge::UiTaskStatus::Failed => "failed",
            claude_ui_bridge::UiTaskStatus::Stopped => "stopped",
        }
        .to_owned(),
        summary: task.summary.clone(),
        output_preview: if task.summary.trim().is_empty() {
            None
        } else {
            Some(task.summary)
        },
        turns_used: task.turns_used,
        kind: match task.kind {
            claude_ui_bridge::UiTaskKind::Background => "background",
            claude_ui_bridge::UiTaskKind::Delegation => "delegation",
            claude_ui_bridge::UiTaskKind::Batch => "batch",
        }
        .to_owned(),
        output_path: task.output_path,
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

fn load_session_tasks_from_paths(
    paths: &AppPaths,
    session_id: Uuid,
) -> Result<Vec<SessionTaskDto>> {
    let session_id_string = session_id.to_string();
    Ok(
        load_persisted_ui_task_snapshots(&task_dir_for_paths(paths, session_id))?
            .into_iter()
            .map(|task| ui_task_node_to_dto(&session_id_string, task))
            .collect(),
    )
}

pub(crate) fn emit_task_snapshot_for_session(app: &AppHandle, paths: &AppPaths, session_id: Uuid) {
    if let Ok(tasks) = load_session_tasks_from_paths(paths, session_id) {
        let _ = app.emit(
            APP_EVENT_TASK_SNAPSHOT,
            TaskSnapshotDto {
                session_id: session_id.to_string(),
                tasks,
            },
        );
    }
}

fn tool_call_to_dto(call: &ToolCall) -> ToolCallDto {
    ToolCallDto {
        id: call.id.clone(),
        name: call.name.clone(),
        input: call.input.clone(),
    }
}

fn conversation_entry_to_dto(entry: &ConversationEntry) -> ConversationEntryDto {
    ConversationEntryDto {
        role: match entry.role {
            ConversationRole::System => "system",
            ConversationRole::User => "user",
            ConversationRole::Assistant => "assistant",
            ConversationRole::Tool => "tool",
        }
        .to_owned(),
        text: entry.text.clone(),
        content_blocks: entry.content_blocks.clone(),
        tool_calls: entry.tool_calls.iter().map(tool_call_to_dto).collect(),
        tool_call_id: entry.tool_call_id.clone(),
        name: entry.name.clone(),
        is_error: entry.is_error,
    }
}

fn session_summary_to_dto(summary: SessionSummary) -> SessionSummaryDto {
    SessionSummaryDto {
        id: summary.session_id.to_string(),
        title: summary.title,
        cwd: summary.cwd.display().to_string(),
        provider_name: summary.provider_name,
        model: summary.model,
        created_at: summary.created_at.to_rfc3339(),
        updated_at: summary.updated_at.to_rfc3339(),
        archived: summary.archived,
    }
}

fn full_settings_from_runtime(
    config: &RuntimeConfig,
    gui_settings: &GuiSettingsFile,
) -> FullSettingsDto {
    FullSettingsDto {
        provider_name: config.provider.name.clone(),
        provider_model: config.provider.model.clone(),
        provider_base_url: config.provider.base_url.clone(),
        provider_protocol: config.provider.protocol.as_str().to_owned(),
        provider_api_key_set: config.provider.api_key.is_some(),
        max_output_tokens: config.provider.max_output_tokens,
        thinking_budget: config.provider.thinking_budget,
        max_retries: config.provider.max_retries,
        timeout_ms: config.provider.timeout_ms,
        retry_initial_backoff_ms: config.provider.retry_initial_backoff_ms,
        retry_max_backoff_ms: config.provider.retry_max_backoff_ms,
        respect_retry_after: config.provider.respect_retry_after,
        permission_mode: config.permission_mode.as_legacy_str().to_owned(),
        max_turns: config.max_turns,
        verbose: gui_settings.verbose.unwrap_or(config.verbose),
        codex_model_provider: gui_settings.codex_model_provider.clone(),
        codex_approval_policy: gui_settings.codex_approval_policy.clone(),
        codex_sandbox_mode: gui_settings.codex_sandbox_mode.clone(),
        codex_persist_extended_history: gui_settings.codex_persist_extended_history.unwrap_or(true),
        codex_memories_enabled: gui_settings.codex_memories_enabled.unwrap_or(true),
        codex_thread_store_endpoint: gui_settings.codex_thread_store_endpoint.clone(),
        codex_config_overrides: gui_settings.codex_config_overrides.clone(),
        codex_permission_profile: gui_settings.codex_permission_profile.clone(),
        codex_service_tier: gui_settings.codex_service_tier.clone(),
        codex_ephemeral: gui_settings.codex_ephemeral,
    }
}

fn apply_gui_settings_to_runtime(
    config: &mut RuntimeConfig,
    gui_settings: &GuiSettingsFile,
) -> Result<()> {
    if let Some(provider_name) = gui_settings.provider_name.as_deref() {
        config.provider.name = provider_name.trim().to_owned();
    }
    if let Some(model) = gui_settings.provider_model.clone() {
        config.provider.model = Some(model);
    }
    if let Some(base_url) = gui_settings.provider_base_url.clone() {
        let protocol = config.provider.protocol;
        config.provider.base_url = normalize_base_url(Some(base_url), protocol);
    }
    if let Some(protocol) = parse_protocol(gui_settings.provider_protocol.as_deref()) {
        config.provider.protocol = protocol;
        config.provider.base_url =
            normalize_base_url(config.provider.base_url.clone(), config.provider.protocol);
    }
    if let Some(max_output_tokens) = gui_settings.max_output_tokens {
        config.provider.max_output_tokens = max_output_tokens.max(256);
    }
    if let Some(thinking_budget) = gui_settings.thinking_budget {
        config.provider.thinking_budget = thinking_budget;
    }
    if let Some(max_retries) = gui_settings.max_retries {
        config.provider.max_retries = max_retries;
    }
    if let Some(timeout_ms) = gui_settings.timeout_ms {
        config.provider.timeout_ms = timeout_ms.max(1_000);
    }
    if let Some(retry_initial_backoff_ms) = gui_settings.retry_initial_backoff_ms {
        config.provider.retry_initial_backoff_ms = retry_initial_backoff_ms.max(50);
    }
    if let Some(retry_max_backoff_ms) = gui_settings.retry_max_backoff_ms {
        config.provider.retry_max_backoff_ms =
            retry_max_backoff_ms.max(config.provider.retry_initial_backoff_ms);
    }
    if let Some(respect_retry_after) = gui_settings.respect_retry_after {
        config.provider.respect_retry_after = respect_retry_after;
    }
    if let Some(permission_mode) = parse_permission_mode(gui_settings.permission_mode.as_deref()) {
        config.permission_mode = permission_mode;
    }
    if let Some(verbose) = gui_settings.verbose {
        config.verbose = verbose;
    }
    if let Some(thinking_budget) = config.provider.thinking_budget
        && thinking_budget >= config.provider.max_output_tokens
    {
        return Err(anyhow!(
            "thinking budget must be lower than max output tokens"
        ));
    }
    Ok(())
}

fn provider_config_to_runtime(
    stored: &ProviderConfig,
    current: &RuntimeProviderConfig,
) -> Result<RuntimeProviderConfig> {
    let protocol = parse_protocol(Some(&stored.protocol)).unwrap_or(ProviderProtocol::OpenAi);
    let base_url = normalize_base_url(stored.base_url.clone(), protocol);
    Ok(RuntimeProviderConfig {
        name: stored.name.clone(),
        base_url,
        api_key: trimmed_option(stored.api_key.clone()),
        model: trimmed_option(stored.model.clone()),
        protocol,
        timeout_ms: current.timeout_ms,
        max_output_tokens: current.max_output_tokens,
        max_retries: current.max_retries,
        retry_initial_backoff_ms: current.retry_initial_backoff_ms,
        retry_max_backoff_ms: current.retry_max_backoff_ms,
        respect_retry_after: current.respect_retry_after,
        request_header_overrides: current.request_header_overrides.clone(),
        request_metadata: current.request_metadata.clone(),
        thinking_budget: current.thinking_budget,
        temperature: None,
        top_p: None,
        top_k: None,
    })
}

fn sync_active_provider_from_runtime(config: &RuntimeConfig, configs: &mut ProviderConfigList) {
    if configs.providers.is_empty() {
        configs.active_provider = None;
        return;
    }
    if configs.active_provider.as_ref().is_some_and(|name| {
        configs
            .providers
            .iter()
            .any(|provider| provider.name == *name)
    }) {
        return;
    }
    if configs
        .providers
        .iter()
        .any(|provider| provider.name == config.provider.name)
    {
        configs.active_provider = Some(config.provider.name.clone());
        return;
    }
    configs.active_provider = configs
        .providers
        .first()
        .map(|provider| provider.name.clone());
}

fn active_provider_config(configs: &ProviderConfigList) -> Option<&ProviderConfig> {
    let active_name = configs.active_provider.as_ref()?;
    configs
        .providers
        .iter()
        .find(|provider| provider.name == *active_name)
}

fn load_base_runtime_config(profile_override: Option<PathBuf>) -> Result<RuntimeConfig> {
    load_runtime_config(
        None,
        profile_override,
        None,
        PermissionMode::Default,
        claude_core::InputFormat::Text,
        claude_core::OutputFormat::Text,
        false,
        false,
        false,
        false,
        DEFAULT_MAX_TURNS,
        ProviderOverrides::default(),
        RuntimeOverrides::default(),
    )
}

fn build_runtime_state() -> Result<RuntimeState> {
    let profile_override = profile_override_from_env();
    let mut config = load_base_runtime_config(profile_override)?;
    let mut provider_configs = load_provider_configs(&config.paths)?;
    let gui_settings = load_gui_settings(&config.paths)?;

    sync_active_provider_from_runtime(&config, &mut provider_configs);
    if let Some(stored) = active_provider_config(&provider_configs) {
        config.provider = provider_config_to_runtime(stored, &config.provider)?;
    }
    apply_gui_settings_to_runtime(&mut config, &gui_settings)?;

    let readiness = validate_provider_config(&config.provider);
    if !readiness.ok && readiness.issues.len() > 2 {
        config.provider.base_url =
            normalize_base_url(config.provider.base_url.clone(), config.provider.protocol);
    }
    configure_runtime_policy_for_config(&config)?;

    let provider = Arc::new(ProviderClient::new()?);
    let session_store = Arc::new(SessionStore::open(config.paths.clone())?);
    let sessions = session_store.list_active_sessions()?;
    let mut projects = load_projects(&config.paths)?;
    if ensure_sessions_have_projects(&mut projects, &sessions) {
        save_projects(&config.paths, &projects)?;
    }

    Ok(RuntimeState {
        config,
        provider,
        session_store,
        projects,
        provider_configs,
        gui_settings,
    })
}

fn persist_runtime_files(state: &RuntimeState) -> Result<()> {
    save_projects(&state.config.paths, &state.projects)?;
    save_provider_configs(&state.config.paths, &state.provider_configs)?;
    save_gui_settings(&state.config.paths, &state.gui_settings)?;
    Ok(())
}

pub(crate) fn persist_session_context(store: &SessionStore, config: &RuntimeConfig) -> Result<()> {
    persist_runtime_config_session_context(store, config)
}

fn restore_session_context(store: &SessionStore, config: &mut RuntimeConfig) -> Result<()> {
    restore_runtime_config_session_context(store, config)
}

pub(crate) fn initialize_session_conversation(
    store: &SessionStore,
    config: &RuntimeConfig,
    title_hint: Option<&str>,
) -> Result<Vec<ConversationEntry>> {
    persist_session_context(store, config)?;
    ensure_conversation_initialized(
        store,
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        title_hint,
    )
}

fn build_project_infos(
    stored_projects: &[ProjectEntry],
    sessions: &[SessionSummary],
) -> Vec<ProjectInfoDto> {
    let mut projects = Vec::new();

    for project in stored_projects {
        projects.push(ProjectInfoDto {
            path: project.path.display().to_string(),
            name: project.name.clone(),
            session_count: project_session_count(&project.path, sessions),
            is_auto_detected: false,
        });
    }

    projects.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    projects
}

fn find_provider_config_index(configs: &ProviderConfigList, name: &str) -> Option<usize> {
    configs
        .providers
        .iter()
        .position(|provider| provider.name == name)
}

fn apply_provider_credentials_from_configs(
    provider: &mut RuntimeProviderConfig,
    provider_configs: &ProviderConfigList,
) {
    let Some(index) = find_provider_config_index(provider_configs, &provider.name) else {
        return;
    };
    let stored = &provider_configs.providers[index];
    // Prefer OS keychain, fall back to JSON plaintext (backward compat).
    if let Some(api_key) = keyring_retrieve(&stored.name) {
        provider.api_key = Some(api_key);
    } else if let Some(api_key) = trimmed_option(stored.api_key.clone()) {
        provider.api_key = Some(api_key);
    }
    // Apply active profile model override if set.
    let profile_model = stored.active_profile.as_ref().and_then(|profile_name| {
        stored
            .profiles
            .iter()
            .find(|p| p.name == *profile_name)
            .and_then(|p| p.model.clone())
    });
    if let Some(ref model) = profile_model {
        provider.model = Some(model.clone());
    } else if provider.model.is_none() {
        provider.model = trimmed_option(stored.model.clone());
    }
    if provider.base_url.is_none() {
        provider.base_url = normalize_base_url(stored.base_url.clone(), provider.protocol);
    }
}

fn store_provider_selection(state: &mut RuntimeState, config: &RuntimeProviderConfig) {
    state.gui_settings.provider_name = Some(config.name.clone());
    state.gui_settings.provider_model = config.model.clone();
    state.gui_settings.provider_base_url = config.base_url.clone();
    state.gui_settings.provider_protocol = Some(config.protocol.as_str().to_owned());
}

fn roo_provider_id_from_runtime(provider: &RuntimeProviderConfig) -> String {
    let lowered_name = provider.name.trim().to_ascii_lowercase();
    let compact = lowered_name.replace([' ', '_'], "-");
    let exact = [
        "anthropic",
        "openai",
        "openai-native",
        "openrouter",
        "deepseek",
        "gemini",
        "google",
        "ollama",
        "lmstudio",
        "xai",
        "mistral",
        "fireworks",
        "litellm",
        "qwen",
        "qwen-code",
        "minimax",
        "fake-ai",
        "moonshot",
        "zai",
        "sambanova",
        "baseten",
        "poe",
        "requesty",
        "unbound",
        "vercel",
        "vercel-ai-gateway",
        "bedrock",
        "aws",
        "kuaikat",
        "kuai-kat",
        "kat",
        "kat-coder",
        "kat-coder-pro",
        "streamlake",
    ];
    if compact == "deepseek"
        && provider
            .base_url
            .as_deref()
            .map(|url| url.to_ascii_lowercase().contains("/anthropic"))
            .unwrap_or(false)
    {
        return "anthropic".to_string();
    }
    if exact.contains(&compact.as_str()) {
        return compact;
    }
    if lowered_name.contains("kuaikat")
        || lowered_name.contains("kuai kat")
        || lowered_name.contains("kat-coder")
        || lowered_name.contains("streamlake")
    {
        return "kuaikat".to_string();
    }
    if lowered_name.contains("minimax") {
        return "minimax".to_string();
    }
    if lowered_name.contains("anthropic") || lowered_name.contains("claude") {
        return "anthropic".to_string();
    }
    if lowered_name.contains("openai") || lowered_name.contains("gpt") {
        return "openai".to_string();
    }

    if let Some(url) = provider.base_url.as_deref() {
        let lowered_url = url.to_ascii_lowercase();
        if lowered_url.contains("minimax") || lowered_url.contains("minimaxi") {
            return "minimax".to_string();
        }
        if lowered_url.contains("streamlakeapi") || lowered_url.contains("claude-code-proxy") {
            return "kuaikat".to_string();
        }
        if lowered_url.contains("/anthropic") && lowered_url.contains("deepseek") {
            return "anthropic".to_string();
        }
        if lowered_url.contains("anthropic") {
            return "anthropic".to_string();
        }
        if lowered_url.contains("openai") {
            return "openai".to_string();
        }
    }

    match provider.protocol {
        ProviderProtocol::Anthropic => "anthropic".to_string(),
        ProviderProtocol::OpenAi => "openai".to_string(),
        _ => provider.name.clone(),
    }
}

/// Build MCP server JSON entries from the centralized runtime config.
///
/// `format_url_transport` receives the `server` map, `url`, `headers`, and the
/// transport type so the caller can customize how URL-based transports are
/// serialized (different header key names, explicit `"type"` field, etc.).
fn build_mcp_server_entries(
    config: &RuntimeConfig,
    include_timeouts: bool,
    format_url_transport: impl Fn(
        &mut serde_json::Map<String, serde_json::Value>,
        &str,
        &std::collections::BTreeMap<String, String>,
        &McpTransportConfig,
    ),
) -> HashMap<String, serde_json::Value> {
    let mut servers = HashMap::new();
    for entry in runtime_mcp_policy_entries(config, &config.mcp_config_paths) {
        if !entry.server.enabled {
            continue;
        }
        let mut server = serde_json::Map::new();

        if include_timeouts {
            server.insert("enabled".to_owned(), serde_json::Value::Bool(true));
            if let Some(timeout) = entry.server.startup_timeout_secs {
                server.insert("startup_timeout_sec".to_owned(), serde_json::json!(timeout));
            }
            if let Some(timeout) = entry.server.request_timeout_secs {
                server.insert("tool_timeout_sec".to_owned(), serde_json::json!(timeout));
            }
        }

        match &entry.server.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                cwd,
                env,
            } => {
                server.insert("command".to_owned(), serde_json::json!(command));
                if !args.is_empty() {
                    server.insert("args".to_owned(), serde_json::json!(args));
                }
                if let Some(cwd) = cwd {
                    server.insert("cwd".to_owned(), serde_json::json!(cwd));
                }
                if !env.is_empty() {
                    server.insert("env".to_owned(), serde_json::json!(env));
                }
            }
            McpTransportConfig::Http { url, headers, .. }
            | McpTransportConfig::Sse { url, headers, .. }
            | McpTransportConfig::WebSocket { url, headers, .. } => {
                format_url_transport(&mut server, url, headers, &entry.server.transport);
            }
            McpTransportConfig::SseIde { url, .. } | McpTransportConfig::WsIde { url, .. } => {
                server.insert("url".to_owned(), serde_json::json!(url));
            }
            McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {}
        }

        servers.insert(entry.server.name, serde_json::Value::Object(server));
    }
    servers
}

/// Convert MCP servers to Roo-compatible JSON (uses `"headers"`, requires `"type"`).
fn roo_mcp_server_overrides(config: &RuntimeConfig) -> HashMap<String, serde_json::Value> {
    build_mcp_server_entries(config, false, |server, url, headers, transport| {
        let type_str = match transport {
            McpTransportConfig::Http { .. } => "streamable-http",
            _ => "sse",
        };
        server.insert("type".to_owned(), serde_json::json!(type_str));
        server.insert("url".to_owned(), serde_json::json!(url));
        if !headers.is_empty() {
            server.insert("headers".to_owned(), serde_json::json!(headers));
        }
    })
}

fn codex_permission_decision(
    allowed: bool,
    permission_updates: &[PermissionUpdate],
) -> AgentPermissionDecision {
    if !allowed {
        return AgentPermissionDecision::Deny;
    }

    let session_scoped = permission_updates.iter().any(|update| match update {
        PermissionUpdate::AddRules { destination, .. }
        | PermissionUpdate::ReplaceRules { destination, .. }
        | PermissionUpdate::RemoveRules { destination, .. }
        | PermissionUpdate::SetMode { destination, .. }
        | PermissionUpdate::AddDirectories { destination, .. }
        | PermissionUpdate::RemoveDirectories { destination, .. } => {
            *destination == PermissionUpdateDestination::Session
        }
    });

    if session_scoped {
        AgentPermissionDecision::AllowAll
    } else {
        AgentPermissionDecision::Allow
    }
}

fn codex_provider_id(provider: &RuntimeProviderConfig, gui_settings: &GuiSettingsFile) -> String {
    gui_settings
        .codex_model_provider
        .clone()
        .or_else(|| {
            Some(format!(
                "remote-code-{}",
                provider.name.to_ascii_lowercase()
            ))
        })
        .map(|value| {
            value
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
                .to_owned()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "remote-code-provider".to_owned())
}

fn codex_approval_policy_from_permission_mode(mode: PermissionMode) -> String {
    match mode {
        PermissionMode::BypassPermissions | PermissionMode::Plan => "never",
        PermissionMode::Auto
        | PermissionMode::Default
        | PermissionMode::AcceptEdits
        | PermissionMode::DontAsk => "on-request",
    }
    .to_owned()
}

fn codex_sandbox_from_permission_mode(mode: PermissionMode) -> String {
    match mode {
        PermissionMode::BypassPermissions => "danger-full-access",
        PermissionMode::Plan => "read-only",
        PermissionMode::Auto
        | PermissionMode::Default
        | PermissionMode::AcceptEdits
        | PermissionMode::DontAsk => "workspace-write",
    }
    .to_owned()
}

fn codex_mcp_server_overrides(config: &RuntimeConfig) -> HashMap<String, serde_json::Value> {
    build_mcp_server_entries(config, true, |server, url, headers, _| {
        server.insert("url".to_owned(), serde_json::json!(url));
        if !headers.is_empty() {
            server.insert("http_headers".to_owned(), serde_json::json!(headers));
        }
    })
}

fn codex_adapter_options_from_runtime(
    config: &RuntimeConfig,
    gui_settings: &GuiSettingsFile,
) -> CodexAdapterOptions {
    let mut config_overrides = gui_settings
        .codex_config_overrides
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    if let Some(effort) = config.effort.clone() {
        config_overrides
            .entry("model_reasoning_effort".to_owned())
            .or_insert(effort);
    }

    CodexAdapterOptions {
        cwd: config.cwd.clone(),
        model: config
            .provider
            .model
            .clone()
            .or_else(|| config.fallback_model.clone()),
        model_provider: Some(codex_provider_id(&config.provider, gui_settings)),
        api_key: config.provider.api_key.clone(),
        base_url: config.provider.base_url.clone(),
        approval_policy: gui_settings.codex_approval_policy.clone().or_else(|| {
            Some(codex_approval_policy_from_permission_mode(
                config.permission_mode,
            ))
        }),
        sandbox_mode: gui_settings
            .codex_sandbox_mode
            .clone()
            .or_else(|| Some(codex_sandbox_from_permission_mode(config.permission_mode))),
        permission_profile: gui_settings.codex_permission_profile.clone(),
        service_tier: gui_settings
            .codex_service_tier
            .as_ref()
            .map(|v| serde_json::json!(v.to_lowercase())),
        persist_extended_history: gui_settings.codex_persist_extended_history.unwrap_or(true),
        ephemeral: gui_settings.codex_ephemeral,
        memories_enabled: gui_settings.codex_memories_enabled.or(Some(true)),
        thread_store_endpoint: gui_settings.codex_thread_store_endpoint.clone(),
        config_overrides,
        cli_overrides: Vec::new(),
        mcp_servers: codex_mcp_server_overrides(config),
        enable_codex_api_key_env: true,
        client_name: Some("remote-code-gui".to_owned()),
        client_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        exec_server_url: env::var("CODEX_EXEC_SERVER_URL").ok(),
        wire_api: if config.provider.protocol == claude_core::ProviderProtocol::Anthropic {
            Some("anthropic_messages".to_string())
        } else {
            None
        },
        upstream_url: if config.provider.protocol == claude_core::ProviderProtocol::Anthropic {
            // The upstream Anthropic URL is the real provider base_url (not the proxy).
            config.provider.base_url.clone()
        } else {
            None
        },
        channel_capacity: Some(1024),
        feedback_capture_enabled: true,
    }
}

fn codex_agent_config_from_options(
    options: &CodexAdapterOptions,
) -> rc_agent_protocol::types::AgentConfig {
    rc_agent_protocol::types::AgentConfig {
        agent_type: ProtocolAgentType::RemoteCodex,
        binary_path: None,
        args: Vec::new(),
        env: Vec::new(),
        working_dir: Some(options.cwd.clone()),
        model: options.model.clone(),
        provider: None,
        api_key: None,
        base_url: None,
    }
}

pub(super) async fn codex_options_snapshot(
    state: &State<'_, AppState>,
) -> std::result::Result<CodexAdapterOptions, String> {
    let mut config = {
        let runtime = state.runtime.lock().await;
        let mut config = runtime.config.clone();
        apply_provider_credentials_from_configs(&mut config.provider, &runtime.provider_configs);
        codex_adapter_options_from_runtime(&config, &runtime.gui_settings)
    };
    if config.cwd.as_os_str().is_empty() {
        config.cwd = env::current_dir().map_err(|error| error.to_string())?;
    }
    Ok(config)
}

fn codex_adapter_key(session_id: Option<String>) -> String {
    session_id
        .and_then(|value| trimmed_option(Some(value)))
        .unwrap_or_else(|| CODEX_GLOBAL_ADAPTER_KEY.to_owned())
}

async fn ensure_codex_adapter(
    state: &State<'_, AppState>,
    key: &str,
) -> std::result::Result<(), String> {
    let options = codex_options_snapshot(state).await?;
    let mut adapters = state.active_codex_adapters.lock().await;
    let needs_create = adapters
        .get(key)
        .map(|adapter| !adapter.is_alive())
        .unwrap_or(true);
    if needs_create {
        let agent_config = codex_agent_config_from_options(&options);
        let mut adapter = CodexInProcessAdapter::start_in_process_with_options(options)
            .await
            .map_err(|error| format!("Failed to start Codex runtime: {error:#}"))?;
        adapter
            .start(&agent_config)
            .await
            .map_err(|error| format!("Failed to initialize Codex adapter: {error:#}"))?;
        adapters.insert(key.to_owned(), adapter);
    }
    Ok(())
}

async fn with_codex_adapter_value<F>(
    state: &State<'_, AppState>,
    session_id: Option<String>,
    operation: F,
) -> std::result::Result<serde_json::Value, String>
where
    F: for<'a> FnOnce(
        &'a mut CodexInProcessAdapter,
    ) -> Pin<
        Box<dyn Future<Output = anyhow::Result<serde_json::Value>> + Send + 'a>,
    >,
{
    let key = codex_adapter_key(session_id);
    ensure_codex_adapter(state, &key).await?;
    let mut adapters = state.active_codex_adapters.lock().await;
    let adapter = adapters
        .get_mut(&key)
        .ok_or_else(|| "Codex adapter was not initialized".to_owned())?;
    operation(adapter)
        .await
        .map_err(|error| format!("{error:#}"))
}

fn parse_codex_mcp_detail(value: Option<&str>) -> Option<McpServerStatusDetail> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value)
            if value.eq_ignore_ascii_case("toolsAndAuthOnly")
                || value.eq_ignore_ascii_case("tools_and_auth_only")
                || value.eq_ignore_ascii_case("tools-auth") =>
        {
            Some(McpServerStatusDetail::ToolsAndAuthOnly)
        }
        Some(value) if value.eq_ignore_ascii_case("full") => Some(McpServerStatusDetail::Full),
        _ => None,
    }
}

fn parse_codex_merge_strategy(value: Option<&str>) -> MergeStrategy {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) if value.eq_ignore_ascii_case("upsert") => MergeStrategy::Upsert,
        _ => MergeStrategy::Replace,
    }
}

fn decode_codex_params<T>(
    params: Option<serde_json::Value>,
    default: impl FnOnce() -> T,
) -> Result<T>
where
    T: DeserializeOwned,
{
    params
        .map(serde_json::from_value)
        .transpose()
        .context("invalid Codex app-server params")
        .map(|value| value.unwrap_or_else(default))
}

fn decode_required_codex_params<T>(params: Option<serde_json::Value>) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(params.unwrap_or(serde_json::Value::Object(Default::default())))
        .context("invalid Codex app-server params")
}

fn parse_codex_plugin_ref(value: String) -> CodexPluginRefRequest {
    let value = value.trim().to_owned();
    if let Some((plugin_name, remote_marketplace_name)) = value.split_once('@') {
        return CodexPluginRefRequest {
            marketplace_path: None,
            remote_marketplace_name: Some(remote_marketplace_name.to_owned()),
            plugin_name: plugin_name.to_owned(),
        };
    }

    CodexPluginRefRequest {
        marketplace_path: None,
        remote_marketplace_name: None,
        plugin_name: value,
    }
}

#[derive(Debug)]
struct GuiPermissionFallbackBroker {
    controller: Arc<RuntimePlanModeController>,
    app: AppHandle,
    pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>,
}

fn permission_request_dto(request_id: String, request: &PermissionRequest) -> PermissionRequestDto {
    PermissionRequestDto {
        request_id,
        tool_name: request.tool_name.clone(),
        tool_use_id: request.tool_use_id.clone().unwrap_or_default(),
        title: request.title.clone().unwrap_or_default(),
        description: request.description.clone().unwrap_or_default(),
        input: request.tool_input.clone(),
        blocked_path: request.blocked_path.clone(),
        permission_suggestions: request.permission_suggestions.clone(),
    }
}

impl GuiPermissionFallbackBroker {
    async fn prompt(&self, request: PermissionRequest) -> PermissionDecision {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_permissions.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        let payload = permission_request_dto(request_id.clone(), &request);

        if self
            .app
            .emit(APP_EVENT_PERMISSION_REQUEST, payload)
            .is_err()
        {
            let mut pending = self.pending_permissions.lock().await;
            pending.remove(&request_id);
            return PermissionDecision::deny("Failed to deliver permission request to GUI.");
        }

        let decision = timeout(Duration::from_secs(PERMISSION_WAIT_SECS), rx).await;
        let decision = match decision {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => PermissionDecision::deny("Permission request channel closed."),
            Err(_) => PermissionDecision::deny(format!(
                "Permission request timed out for {}.",
                request.tool_name
            )),
        };

        let _ = self.app.emit(
            APP_EVENT_PERMISSION_RESOLVED,
            PermissionDecisionDto {
                request_id,
                allowed: decision.allowed,
                message: decision.message.clone(),
                updated_input: decision.updated_input.clone(),
                permission_updates: decision.permission_updates.clone(),
                feedback: decision.feedback.clone(),
                content_blocks: decision.content_blocks.clone(),
            },
        );

        decision
    }
}

#[async_trait]
impl PermissionBroker for GuiPermissionFallbackBroker {
    fn mode(&self) -> Option<PermissionMode> {
        Some(self.controller.current_mode())
    }

    async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
        let mode = self.controller.current_mode();

        if matches!(mode, PermissionMode::BypassPermissions) && request.blocked_path.is_none() {
            return PermissionDecision::allow();
        }

        if matches!(mode, PermissionMode::DontAsk | PermissionMode::AcceptEdits)
            && request.blocked_path.is_none()
            && auto_allows(mode, classify_tool(&request.tool_name))
        {
            return PermissionDecision::allow();
        }

        if matches!(mode, PermissionMode::Plan) {
            return PermissionDecision::deny(
                "Plan mode is active. Only read-only tools and plan-file edits are allowed.",
            );
        }

        self.prompt(request).await
    }

    async fn decide_forced_prompt(&self, request: PermissionRequest) -> PermissionDecision {
        self.prompt(request).await
    }
}

pub(crate) struct GuiRuntimePermissionBroker {
    controller: Arc<RuntimePlanModeController>,
    inner: LayeredPermissionBroker<GuiPermissionFallbackBroker>,
}

impl std::fmt::Debug for GuiRuntimePermissionBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiRuntimePermissionBroker")
            .field("mode", &self.controller.current_mode())
            .finish_non_exhaustive()
    }
}

impl GuiRuntimePermissionBroker {
    pub(crate) fn new(
        config: &RuntimeConfig,
        controller: Arc<RuntimePlanModeController>,
        app: AppHandle,
        pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>,
    ) -> Self {
        let inner = LayeredPermissionBroker::new(
            GuiPermissionFallbackBroker {
                controller: controller.clone(),
                app,
                pending_permissions,
            },
            load_layered_rules(
                &config.cwd,
                &config.paths.profile_dir,
                &config.settings_files,
                &config.cli_settings_files,
            ),
        );
        Self { controller, inner }
    }

    fn decide_plan_mode(&self, request: PermissionRequest) -> PermissionDecision {
        match request.resolved_permission_class() {
            PermissionClass::Read => PermissionDecision::allow(),
            PermissionClass::Edit if self.controller.plan_file_matches_request(&request) => {
                PermissionDecision::allow()
            }
            PermissionClass::Edit => PermissionDecision::deny(
                "Plan mode is active. Only the current plan file may be edited.",
            ),
            _ => PermissionDecision::deny(
                "Plan mode is active. Only read-only tools and plan-file edits are allowed.",
            ),
        }
    }
}

#[async_trait]
impl PermissionBroker for GuiRuntimePermissionBroker {
    async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
        if self.controller.current_mode() == PermissionMode::Plan {
            return self.decide_plan_mode(request);
        }
        self.inner.decide(request).await
    }

    async fn decide_forced_prompt(&self, request: PermissionRequest) -> PermissionDecision {
        self.inner.decide_forced_prompt(request).await
    }

    fn mode(&self) -> Option<PermissionMode> {
        if self.controller.current_mode() == PermissionMode::Plan {
            Some(PermissionMode::Plan)
        } else {
            self.inner.mode()
        }
    }

    fn additional_working_directories(&self) -> Vec<std::path::PathBuf> {
        self.inner.additional_working_directories()
    }

    fn add_session_rule(
        &self,
        action: claude_permissions::RuleAction,
        tool_pattern: String,
    ) -> Result<()> {
        self.inner.add_session_rule(action, tool_pattern)
    }

    fn clear_session_rules(&self) -> Result<usize> {
        self.inner.clear_session_rules()
    }

    fn apply_permission_updates(
        &self,
        updates: &[claude_permissions::PermissionUpdate],
    ) -> Result<usize> {
        self.inner.apply_permission_updates(updates)
    }

    fn audit_records(&self) -> Vec<claude_permissions::PermissionAuditRecord> {
        self.inner.audit_records()
    }

    fn layered_rules(&self) -> Vec<claude_permissions::SourceAwarePermissionRule> {
        self.inner.layered_rules()
    }

    fn matching_rule(
        &self,
        request: &PermissionRequest,
    ) -> Option<claude_permissions::SourceAwarePermissionRule> {
        self.inner.matching_rule(request)
    }

    fn matching_rule_action(
        &self,
        request: &PermissionRequest,
    ) -> Option<claude_permissions::RuleAction> {
        self.inner.matching_rule_action(request)
    }
}

fn as_error<T>(result: Result<T>) -> std::result::Result<T, String> {
    result.map_err(|error| format!("{error:#}"))
}

/// Read the agent_type stored in session metadata.
/// Returns `"remote_claude"` when no agent_type has been set (the default path).
fn get_session_agent_type(store: &SessionStore, session_id: Uuid) -> String {
    store
        .load_transcript(session_id)
        .ok()
        .and_then(|transcript| {
            transcript
                .latest_named_event_payload("agent_type")
                .and_then(|val| {
                    val.get("agent_type")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
        })
        .unwrap_or_else(|| "remote_claude".to_owned())
}

// ---------------------------------------------------------------------------
// In-process Codex prompt execution
// ---------------------------------------------------------------------------

/// Receive the next event from `rx` with a 30-second timeout.
/// If the timeout fires, checks `is_alive` to determine whether the worker
/// crashed (return `Err`) or is just slow (return `Ok(None)` to retry).
async fn recv_with_liveness_check<T: Send>(
    rx: &mut mpsc::Receiver<T>,
    is_alive: impl Fn() -> bool,
) -> std::result::Result<T, String> {
    loop {
        match timeout(Duration::from_secs(30), rx.recv()).await {
            Ok(Some(event)) => return Ok(event),
            Ok(None) => return Err("agent channel closed unexpectedly".to_string()),
            Err(_) => {
                if !is_alive() {
                    return Err("agent worker crashed unexpectedly".to_string());
                }
                // Worker alive but slow — retry.
            }
        }
    }
}

/// Output of the shared event-forwarding loop.
struct StreamLoopResult {
    pub final_text: String,
    pub tool_calls: Vec<crate::dto::ToolCallDto>,
    pub usage_info: rc_agent_protocol::events::UsageInfo,
    pub stop_reason: String,
}

/// Emit a Tauri event and log a warning if emission fails.
fn emit_event(app: &AppHandle, event: &str, payload: impl serde::Serialize + Clone) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(event, error = %e, "Failed to emit GUI event");
    }
}

fn unified_permission_kind(tool_name: &str, input: &serde_json::Value) -> String {
    input
        .get("ask")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(tool_name)
        .to_owned()
}

fn unified_permission_tool_use_id(native_request_id: &str, input: &serde_json::Value) -> String {
    input
        .get("tool_use_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(native_request_id)
        .to_owned()
}

fn unified_permission_description(
    agent_name: &str,
    tool_name: &str,
    input: &serde_json::Value,
) -> String {
    if let Some(reason) = input
        .get("reason")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return reason.to_owned();
    }

    match input.get("ask").and_then(|value| value.as_str()) {
        Some("followup") => format!("{agent_name} 请求补充信息。"),
        Some("completion_result") => format!("{agent_name} 已给出完成结果，等待接受或反馈。"),
        Some("mistake_limit_reached") => {
            format!("{agent_name} 连续遇到工具/格式问题，等待继续或取消。")
        }
        Some("api_req_failed") => format!("{agent_name} API 请求失败，等待是否重试。"),
        Some("auto_approval_max_req_reached") => {
            format!("{agent_name} 自动审批额度已达到上限，等待确认是否继续。")
        }
        _ => format!("工具 {tool_name} 需要授权才能执行。"),
    }
}

/// Shared event-forwarding loop for Codex, Roo, and Claude in-process adapters.
///
/// Reads `UnifiedAgentEvent`s from `rx`, emits corresponding GUI events via
/// the Tauri `AppHandle`, and returns the accumulated result on completion.
///
/// The `agent_specific` closure is called for each event and can return
/// `Some(ControlFlow::Break(_))` to handle agent-specific events (e.g. Codex
/// `codex_token_usage` progress) or override default error handling.
#[allow(clippy::too_many_arguments)]
async fn forward_agent_events(
    app: &AppHandle,
    session_id: &str,
    rx: &mut mpsc::Receiver<rc_agent_protocol::events::UnifiedAgentEvent>,
    is_alive: impl Fn() -> bool + Send + 'static,
    session_store: &claude_session::SessionStore,
    agent_name: &str,
    permission_inserter: impl Fn(String, String, String) + Send, // (gui_request_id, request_id, request_kind)
    permission_title: &str,
    mut agent_specific: impl FnMut(
        &rc_agent_protocol::events::UnifiedAgentEvent,
        &mut String,
        &mut rc_agent_protocol::events::UsageInfo,
    ) -> Option<std::ops::ControlFlow<()>>,
) -> std::result::Result<StreamLoopResult, String> {
    use rc_agent_protocol::events::UnifiedAgentEvent;
    use std::ops::ControlFlow;

    let mut final_text = String::new();
    let mut tool_calls: Vec<crate::dto::ToolCallDto> = Vec::new();
    let mut usage_info = rc_agent_protocol::events::UsageInfo::default();
    let mut stop_reason = String::new();

    'stream: loop {
        let event = match recv_with_liveness_check(rx, &is_alive).await {
            Ok(e) => e,
            Err(msg) => {
                tracing::error!(error = %msg, "{agent_name} streaming loop aborted");
                return Err(msg);
            }
        };

        // Give agent-specific handler first crack at the event.
        if let Some(cf) = agent_specific(&event, &mut final_text, &mut usage_info) {
            match cf {
                ControlFlow::Break(()) => break 'stream,
                ControlFlow::Continue(()) => continue,
            }
        }

        match event {
            UnifiedAgentEvent::MessageDelta { delta, .. } => {
                final_text.push_str(&delta);
                emit_event(
                    app,
                    APP_EVENT_STREAMING_DELTA,
                    crate::dto::StreamingDeltaDto {
                        session_id: session_id.to_string(),
                        delta,
                    },
                );
            }
            UnifiedAgentEvent::ToolCallStarted {
                tool_name,
                tool_input,
                ..
            } => {
                let tool_call_id = Uuid::new_v4().to_string();
                tool_calls.push(crate::dto::ToolCallDto {
                    id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    input: tool_input,
                });
                emit_event(
                    app,
                    APP_EVENT_TOOL_START,
                    crate::dto::ToolProgressDto {
                        tool_call_id,
                        tool_name,
                        message: "started".to_owned(),
                        active_form: None,
                    },
                );
            }
            UnifiedAgentEvent::ToolCallProgress {
                tool_name,
                progress,
                ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_TOOL_PROGRESS,
                    crate::dto::ToolProgressDto {
                        tool_call_id: String::new(),
                        tool_name,
                        message: progress,
                        active_form: None,
                    },
                );
            }
            UnifiedAgentEvent::ToolCallCompleted {
                tool_name, result, ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_TOOL_RESULT,
                    crate::dto::ToolResultDto {
                        tool_call_id: String::new(),
                        tool_name,
                        is_error: false,
                        output: result.to_string(),
                    },
                );
            }
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                let gui_request_id = Uuid::new_v4().to_string();
                let request_kind = unified_permission_kind(&tool_name, &input);
                let tool_use_id = unified_permission_tool_use_id(&request_id, &input);
                let description = unified_permission_description(agent_name, &tool_name, &input);
                permission_inserter(gui_request_id.clone(), request_id, request_kind);
                emit_event(
                    app,
                    APP_EVENT_PERMISSION_REQUEST,
                    crate::dto::PermissionRequestDto {
                        request_id: gui_request_id,
                        tool_name: tool_name.clone(),
                        tool_use_id,
                        title: permission_title.to_owned(),
                        description,
                        input,
                        blocked_path: None,
                        permission_suggestions: vec![],
                    },
                );
            }
            UnifiedAgentEvent::SubtaskStarted {
                task_id,
                description,
                ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_SUBTASK_STARTED,
                    crate::dto::SubtaskStartedDto {
                        session_id: session_id.to_string(),
                        task_id,
                        parent_task_id: None,
                        description,
                        depth: 0,
                    },
                );
            }
            UnifiedAgentEvent::SubtaskProgress {
                task_id, progress, ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_SUBTASK_PROGRESS,
                    crate::dto::SubtaskProgressDto {
                        session_id: session_id.to_string(),
                        task_id,
                        turn: 0,
                        max_turns: 0,
                        summary: progress,
                    },
                );
            }
            UnifiedAgentEvent::SubtaskCompleted {
                task_id, result, ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_SUBTASK_COMPLETED,
                    crate::dto::SubtaskCompletedDto {
                        session_id: session_id.to_string(),
                        task_id,
                        success: true,
                        output_preview: result.to_string(),
                        turns_used: 0,
                    },
                );
            }
            UnifiedAgentEvent::ContextUsage { used, total, .. } => {
                emit_event(
                    app,
                    APP_EVENT_CONTEXT_USAGE,
                    serde_json::json!({
                        "session_id": session_id,
                        "used": used,
                        "total": total,
                    }),
                );
            }
            UnifiedAgentEvent::ContextOverflow { used, total, .. } => {
                emit_event(
                    app,
                    APP_EVENT_CONTEXT_OVERFLOW,
                    serde_json::json!({
                        "session_id": session_id,
                        "used": used,
                        "total": total,
                    }),
                );
            }
            UnifiedAgentEvent::ContextCompacted {
                entries_removed,
                usage_ratio,
                ..
            } => {
                emit_event(
                    app,
                    APP_EVENT_CONTEXT_COMPACTED,
                    serde_json::json!({
                        "session_id": session_id,
                        "entries_removed": entries_removed,
                        "usage_ratio": usage_ratio,
                    }),
                );
            }
            UnifiedAgentEvent::Completed { result, .. } => {
                if !result.response_text.is_empty() {
                    final_text = result.response_text;
                }
                if !result.tool_calls.is_empty() {
                    tool_calls = result
                        .tool_calls
                        .into_iter()
                        .map(|tc| crate::dto::ToolCallDto {
                            id: tc.id,
                            name: tc.name,
                            input: tc.input,
                        })
                        .collect();
                }
                if result.usage.input_tokens > 0
                    || result.usage.output_tokens > 0
                    || result.usage.cache_read > 0
                    || result.usage.cache_write > 0
                {
                    usage_info = result.usage;
                }
                // Persist assistant response to SessionStore for crash recovery.
                if let Ok(sid) = Uuid::parse_str(session_id) {
                    if !final_text.is_empty() {
                        let assistant_entry = ConversationEntry::assistant(final_text.clone());
                        if let Err(error) =
                            session_store.append_conversation_entry(sid, &assistant_entry)
                        {
                            tracing::warn!(%session_id, "Failed to persist {agent_name} assistant message: {error:#}");
                        }
                    }
                } else {
                    tracing::warn!(
                        session_id,
                        "Cannot persist {agent_name} assistant message: invalid UUID"
                    );
                }
                break 'stream;
            }
            UnifiedAgentEvent::Error {
                message,
                recoverable,
                ..
            } => {
                tracing::warn!(error = %message, "{agent_name} error event");
                if !recoverable {
                    return Err(message);
                }
            }
            UnifiedAgentEvent::Stopped => {
                stop_reason = "stopped".to_owned();
                break 'stream;
            }
            UnifiedAgentEvent::Started(_) | UnifiedAgentEvent::Ready => {
                tracing::debug!(event = ?event, "{agent_name} lifecycle event");
            }
            _ => {}
        }
    }

    Ok(StreamLoopResult {
        final_text,
        tool_calls,
        usage_info,
        stop_reason,
    })
}
///
/// Creates or reuses a [`CodexInProcessAdapter`] with isolated storage,
/// sends the user message, and forwards events to the frontend via Tauri emissions.
async fn run_codex_in_process_prompt(
    app: &AppHandle,
    codex_adapters: &Arc<Mutex<HashMap<String, CodexInProcessAdapter>>>,
    pending_codex_permissions: &Arc<Mutex<HashMap<String, CodexPendingPermission>>>,
    session_id: &str,
    prompt: &str,
    options: CodexAdapterOptions,
    session_store: Arc<claude_session::SessionStore>,
) -> std::result::Result<String, String> {
    let working_dir = options.cwd.clone();
    let model = options.model.clone();
    // Ensure the adapter exists for this session.
    {
        let mut adapters = codex_adapters.lock().await;
        if !adapters.contains_key(session_id) {
            tracing::info!(session_id, "Creating new CodexInProcessAdapter");
            let adapter = CodexInProcessAdapter::start_in_process_with_options(options)
                .await
                .map_err(|e| format!("Failed to start Codex in-process runtime: {e}"))?;
            adapters.insert(session_id.to_string(), adapter);
        }
    }

    // Get the adapter and send the message.
    let mut adapters = codex_adapters.lock().await;
    let adapter = adapters
        .get_mut(session_id)
        .ok_or_else(|| "Codex adapter not found".to_string())?;

    // Start the adapter if not yet started.
    if !adapter.is_alive() {
        let agent_config = rc_agent_protocol::types::AgentConfig {
            agent_type: ProtocolAgentType::RemoteCodex,
            binary_path: None,
            args: Vec::new(),
            env: Vec::new(),
            working_dir: Some(working_dir.clone()),
            model,
            provider: None,
            api_key: None,
            base_url: None,
        };
        adapter
            .start(&agent_config)
            .await
            .map_err(|e| format!("CodexInProcessAdapter::start failed: {e}"))?;
    }

    let mut rx = adapter
        .send_message(session_id, prompt)
        .await
        .map_err(|e| format!("CodexInProcessAdapter::send_message failed: {e}"))?;

    // Persist user message to SessionStore for crash recovery.
    if let Ok(sid) = Uuid::parse_str(session_id) {
        let user_entry = ConversationEntry::user(prompt);
        if let Err(error) = session_store.append_conversation_entry(sid, &user_entry) {
            tracing::warn!(%session_id, "Failed to persist Codex user message: {error:#}");
        }
    } else {
        tracing::warn!(
            session_id,
            "Cannot persist Codex user message: invalid UUID"
        );
    }

    // Forward events to the frontend. Drop the lock before streaming.
    drop(adapters);

    let codex_adapters_ref = codex_adapters.clone();
    let sid_for_liveness = session_id.to_string();
    let pending_perms = pending_codex_permissions.clone();
    let sid_for_perm = session_id.to_string();

    let result = forward_agent_events(
        app,
        session_id,
        &mut rx,
        move || {
            codex_adapters_ref.try_lock().is_ok_and(|a| {
                a.get(&sid_for_liveness)
                    .is_some_and(|adapter| adapter.is_alive())
            })
        },
        &session_store,
        "Codex",
        {
            let pending_perms = pending_perms.clone();
            let sid = sid_for_perm.clone();
            move |gui_request_id, request_id, _request_kind| {
                if let Ok(mut pending) = pending_perms.try_lock() {
                    pending.insert(
                        gui_request_id,
                        CodexPendingPermission {
                            session_id: sid.clone(),
                            request_id,
                        },
                    );
                }
            }
        },
        "Codex 请求权限",
        |event, _final_text, usage_info| {
            use rc_agent_protocol::events::UnifiedAgentEvent;
            match event {
                UnifiedAgentEvent::ToolCallProgress {
                    tool_name,
                    progress,
                    ..
                } => {
                    if tool_name == "codex_token_usage"
                        && let Ok(value) = serde_json::from_str::<serde_json::Value>(progress)
                        && let Some(next_usage) = usage_info_from_codex_token_usage(&value)
                    {
                        *usage_info = next_usage;
                    }
                    None
                }
                UnifiedAgentEvent::CodexAppServerNotification { method, params, .. } => {
                    emit_event(
                        app,
                        APP_EVENT_CODEX_APP_SERVER_NOTIFICATION,
                        serde_json::json!({
                            "session_id": session_id,
                            "method": method,
                            "params": params.clone(),
                        }),
                    );
                    emit_event(
                        app,
                        APP_EVENT_TOOL_PROGRESS,
                        ToolProgressDto {
                            tool_call_id: String::new(),
                            tool_name: "codex_app_server_event".to_owned(),
                            message: serde_json::json!({
                                "method": method,
                                "params": params,
                            })
                            .to_string(),
                            active_form: None,
                        },
                    );
                    None
                }
                UnifiedAgentEvent::Error {
                    recoverable: true,
                    message,
                    ..
                } => {
                    emit_event(
                        app,
                        APP_EVENT_CODEX_RECOVERABLE_ERROR,
                        serde_json::json!({
                            "session_id": session_id,
                            "message": message,
                            "timestamp": chrono::Utc::now().timestamp_millis(),
                        }),
                    );
                    None // Handled recoverable, continue
                }
                _ => None,
            }
        },
    )
    .await?;

    let _ = app.emit(
        APP_EVENT_PROMPT_DONE,
        PromptDoneDto {
            session_id: session_id.to_string(),
            is_error: false,
            error: None,
            result: Some(PromptResultDto {
                session_id: session_id.to_string(),
                text: result.final_text.clone(),
                tool_calls: result.tool_calls,
                usage: UsageDto {
                    input_tokens: result.usage_info.input_tokens,
                    output_tokens: result.usage_info.output_tokens,
                    total_tokens: result.usage_info.input_tokens + result.usage_info.output_tokens,
                },
                num_turns: 1,
                stop_reason: result.stop_reason,
            }),
        },
    );

    Ok(result.final_text)
}

// ---------------------------------------------------------------------------
// Roo in-process prompt execution
// ---------------------------------------------------------------------------

/// Execute a prompt via the native Roo in-process adapter.
///
/// This creates a [`RooInProcessAdapter`], starts it, sends the user message,
/// and forwards events to the frontend via Tauri emissions.
#[allow(clippy::too_many_arguments)]
async fn run_roo_in_process_prompt(
    app: &AppHandle,
    roo_adapters: &Arc<Mutex<HashMap<String, RooInProcessAdapter>>>,
    pending_roo_permissions: &Arc<Mutex<HashMap<String, RooPendingPermission>>>,
    session_id: &str,
    prompt: &str,
    working_dir: PathBuf,
    model: Option<String>,
    api_key: Option<String>,
    provider_name: String,
    base_url: Option<String>,
    mcp_servers: HashMap<String, serde_json::Value>,
    roo_storage_path: PathBuf,
    session_store: Arc<claude_session::SessionStore>,
) -> std::result::Result<String, String> {
    // Ensure the adapter exists for this session.
    {
        let mut adapters = roo_adapters.lock().await;
        if !adapters.contains_key(session_id) {
            tracing::info!(session_id, "Creating new RooInProcessAdapter");
            let mut adapter = RooInProcessAdapter::new();
            if !mcp_servers.is_empty() {
                adapter.set_external_mcp_servers(mcp_servers);
            }
            let agent_config = rc_agent_protocol::types::AgentConfig {
                agent_type: ProtocolAgentType::RemoteRoo,
                binary_path: None,
                args: Vec::new(),
                env: vec![
                    (
                        "ROO_TASK_STORAGE_PATH".to_owned(),
                        roo_storage_path.to_string_lossy().to_string(),
                    ),
                    ("ROO_API_CONFIG_NAME".to_owned(), provider_name.clone()),
                ],
                working_dir: Some(working_dir.clone()),
                model: model.clone(),
                provider: Some(provider_name.clone()),
                api_key: api_key.clone(),
                base_url: base_url.clone(),
            };
            adapter
                .start(&agent_config)
                .await
                .map_err(|e| format!("Failed to start Roo in-process runtime: {e}"))?;
            adapters.insert(session_id.to_string(), adapter);
        }
    }

    // Get the adapter and send the message.
    let mut adapters = roo_adapters.lock().await;
    let adapter = adapters
        .get_mut(session_id)
        .ok_or_else(|| "Roo adapter not found".to_string())?;

    let mut rx = adapter
        .send_message(session_id, prompt)
        .await
        .map_err(|e| format!("RooInProcessAdapter::send_message failed: {e}"))?;

    // Persist user message to SessionStore for crash recovery.
    if let Ok(sid) = Uuid::parse_str(session_id) {
        let user_entry = ConversationEntry::user(prompt);
        if let Err(error) = session_store.append_conversation_entry(sid, &user_entry) {
            tracing::warn!(%session_id, "Failed to persist Roo user message: {error:#}");
        }
    } else {
        tracing::warn!(session_id, "Cannot persist Roo user message: invalid UUID");
    }

    // Forward events to the frontend. Drop the lock before streaming.
    drop(adapters);

    let roo_adapters_ref = roo_adapters.clone();
    let sid_for_liveness = session_id.to_string();
    let pending_perms = pending_roo_permissions.clone();
    let sid_for_perm = session_id.to_string();

    let result = forward_agent_events(
        app,
        session_id,
        &mut rx,
        move || {
            roo_adapters_ref.try_lock().is_ok_and(|a| {
                a.get(&sid_for_liveness)
                    .is_some_and(|adapter| adapter.is_alive())
            })
        },
        &session_store,
        "Roo",
        {
            let pending_perms = pending_perms.clone();
            let sid = sid_for_perm.clone();
            move |gui_request_id, request_id, request_kind| {
                if let Ok(mut pending) = pending_perms.try_lock() {
                    pending.insert(
                        gui_request_id,
                        RooPendingPermission {
                            session_id: sid.clone(),
                            request_id,
                            request_kind,
                        },
                    );
                }
            }
        },
        "Roo 请求权限",
        |_event, _final_text, _usage_info| None,
    )
    .await?;

    let _ = app.emit(
        APP_EVENT_PROMPT_DONE,
        PromptDoneDto {
            session_id: session_id.to_string(),
            is_error: false,
            error: None,
            result: Some(PromptResultDto {
                session_id: session_id.to_string(),
                text: result.final_text.clone(),
                tool_calls: result.tool_calls,
                usage: UsageDto {
                    input_tokens: result.usage_info.input_tokens,
                    output_tokens: result.usage_info.output_tokens,
                    total_tokens: result.usage_info.input_tokens + result.usage_info.output_tokens,
                },
                num_turns: 1,
                stop_reason: result.stop_reason,
            }),
        },
    );

    Ok(result.final_text)
}

/// Execute a prompt via the native Claude in-process adapter.
///
/// This creates a [`ClaudeInProcessAdapter`], starts it, sends the user message,
/// and forwards events to the frontend via Tauri emissions.
async fn run_claude_in_process_prompt(
    app: &AppHandle,
    claude_adapters: &Arc<Mutex<HashMap<String, ClaudeInProcessAdapter>>>,
    pending_claude_permissions: &Arc<Mutex<HashMap<String, ClaudePendingPermission>>>,
    session_id: &str,
    prompt: &str,
    runtime_config: claude_config::RuntimeConfig,
    session_store: Arc<claude_session::SessionStore>,
) -> std::result::Result<String, String> {
    // Ensure the adapter exists for this session.
    {
        let mut adapters = claude_adapters.lock().await;
        if !adapters.contains_key(session_id) {
            tracing::info!(
                session_id,
                "Creating new ClaudeInProcessAdapter with RuntimeConfig"
            );
            let mut adapter =
                ClaudeInProcessAdapter::new(runtime_config.clone(), session_store.clone());
            let agent_config = rc_agent_protocol::types::AgentConfig {
                agent_type: ProtocolAgentType::RemoteClaude,
                binary_path: None,
                args: Vec::new(),
                env: Vec::new(),
                working_dir: Some(runtime_config.cwd.clone()),
                model: runtime_config.provider.model.clone(),
                provider: Some(runtime_config.provider.name.clone()),
                api_key: runtime_config.provider.api_key.clone(),
                base_url: runtime_config.provider.base_url.clone(),
            };
            adapter
                .start(&agent_config)
                .await
                .map_err(|e| format!("Failed to start Claude in-process runtime: {e}"))?;
            adapters.insert(session_id.to_string(), adapter);
        }
    }

    // Get the adapter and send the message.
    let mut adapters = claude_adapters.lock().await;
    let adapter = adapters
        .get_mut(session_id)
        .ok_or_else(|| "Claude adapter not found".to_string())?;

    let mut rx = adapter
        .send_message(session_id, prompt)
        .await
        .map_err(|e| format!("ClaudeInProcessAdapter::send_message failed: {e}"))?;

    // Persist user message to SessionStore for crash recovery.
    if let Ok(sid) = Uuid::parse_str(session_id) {
        let user_entry = ConversationEntry::user(prompt);
        if let Err(error) = session_store.append_conversation_entry(sid, &user_entry) {
            tracing::warn!(%session_id, "Failed to persist Claude user message: {error:#}");
        }
    } else {
        tracing::warn!(
            session_id,
            "Cannot persist Claude user message: invalid UUID"
        );
    }

    // Forward events to the frontend. Drop the lock before streaming.
    drop(adapters);

    let claude_adapters_ref = claude_adapters.clone();
    let sid_for_liveness = session_id.to_string();
    let pending_perms = pending_claude_permissions.clone();
    let sid_for_perm = session_id.to_string();

    let result = forward_agent_events(
        app,
        session_id,
        &mut rx,
        move || {
            claude_adapters_ref.try_lock().is_ok_and(|a| {
                a.get(&sid_for_liveness)
                    .is_some_and(|adapter| adapter.is_alive())
            })
        },
        &session_store,
        "Claude",
        {
            let pending_perms = pending_perms.clone();
            let sid = sid_for_perm.clone();
            move |gui_request_id, request_id, _request_kind| {
                if let Ok(mut pending) = pending_perms.try_lock() {
                    pending.insert(
                        gui_request_id,
                        ClaudePendingPermission {
                            session_id: sid.clone(),
                            request_id,
                        },
                    );
                }
            }
        },
        "Claude 请求权限",
        |_event, _final_text, _usage_info| None,
    )
    .await?;

    let _ = app.emit(
        APP_EVENT_PROMPT_DONE,
        PromptDoneDto {
            session_id: session_id.to_string(),
            is_error: false,
            error: None,
            result: Some(PromptResultDto {
                session_id: session_id.to_string(),
                text: result.final_text.clone(),
                tool_calls: result.tool_calls,
                usage: UsageDto {
                    input_tokens: result.usage_info.input_tokens,
                    output_tokens: result.usage_info.output_tokens,
                    total_tokens: result.usage_info.input_tokens + result.usage_info.output_tokens,
                },
                num_turns: 1,
                stop_reason: result.stop_reason,
            }),
        },
    );

    Ok(result.final_text)
}

#[cfg(test)]
mod tests;
