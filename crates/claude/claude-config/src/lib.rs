//! Configuration loading and provider management.
//!
//! Handles discovery of application paths, loading of provider credentials from
//! environment variables and TOML settings files, failover configuration, and
//! legacy profile import.

pub mod env_vars;
pub mod settings_layers;
pub mod tool_filters;

pub use crate::settings_layers::{RuntimeOverrides, SettingSource};

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use directories::BaseDirs;

use claude_core::{
    DEFAULT_PROFILE_DIR_NAME, HookEvent, HookMatcher, InputFormat, LEGACY_PROFILE_DIR_NAME,
    OutputFormat, PermissionMode, ProviderProtocol,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings_layers::{
    ResolvedRuntimeSettings, discover_runtime_settings_sources, load_runtime_settings,
    load_runtime_settings_with_source_hints, resolve_runtime_settings_files,
    setting_source_for_kind,
};
use crate::tool_filters::merge_tool_filters;

/// Runtime version used in User-Agent, billing attribution, and API metadata.
///
/// Can be overridden at runtime via the `CLAUDE_CODE_CLI_VERSION` environment
/// variable to match the official Claude Code CLI version (e.g. "2.1.39").
pub fn runtime_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        std::env::var("CLAUDE_CODE_CLI_VERSION")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
    })
}

/// Compile-time fallback version (CARGO_PKG_VERSION).
pub const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");

const RESERVED_PROVIDER_HEADER_NAMES: &[&str] = &["content-length", "content-type", "host"];

const fn default_provider_max_retries() -> u32 {
    10
}

const fn default_provider_retry_initial_backoff_ms() -> u64 {
    500
}

const fn default_provider_retry_max_backoff_ms() -> u64 {
    5_000
}

const fn default_provider_respect_retry_after() -> bool {
    true
}

/// Well-known application paths derived from the profile directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPaths {
    /// Root profile directory (e.g. `~/.remote-code-rust`).
    pub profile_dir: PathBuf,
    /// SQLite database path for state persistence.
    pub state_db_path: PathBuf,
    /// Directory for session transcripts.
    pub sessions_dir: PathBuf,
    /// Directory for exported artifacts.
    pub artifacts_dir: PathBuf,
    /// Directory for log files.
    pub logs_dir: PathBuf,
    /// Directory for provider profiles.
    pub profiles_dir: PathBuf,
    /// Directory for installed skills.
    pub skills_dir: PathBuf,
    /// Directory for installed plugins.
    pub plugins_dir: PathBuf,
}

impl AppPaths {
    /// # Errors
    /// Returns an error if the user home directory cannot be located.
    pub fn discover(profile_override: Option<PathBuf>) -> Result<Self> {
        let profile_dir = resolve_profile_dir(profile_override, None)?;
        Ok(Self::from_profile_dir(profile_dir))
    }

    /// # Errors
    /// Returns an error if the user home directory cannot be located.
    pub fn discover_for_cwd(profile_override: Option<PathBuf>, cwd: &Path) -> Result<Self> {
        let profile_dir = resolve_profile_dir(profile_override, Some(cwd))?;
        Ok(Self::from_profile_dir(profile_dir))
    }

    fn from_profile_dir(profile_dir: PathBuf) -> Self {
        Self {
            state_db_path: profile_dir.join("state.db"),
            sessions_dir: profile_dir.join("sessions"),
            artifacts_dir: profile_dir.join("artifacts"),
            logs_dir: profile_dir.join("logs"),
            profiles_dir: profile_dir.join("profiles"),
            skills_dir: profile_dir.join("skills"),
            plugins_dir: profile_dir.join("plugins"),
            profile_dir,
        }
    }

    /// # Errors
    /// Returns an error if any directory cannot be created.
    pub fn ensure_exists(&self) -> Result<()> {
        for directory in [
            &self.profile_dir,
            &self.sessions_dir,
            &self.artifacts_dir,
            &self.logs_dir,
            &self.profiles_dir,
            &self.skills_dir,
            &self.plugins_dir,
        ] {
            fs::create_dir_all(directory)
                .with_context(|| format!("failed to create {}", directory.display()))?;
        }
        Ok(())
    }

    /// # Errors
    /// Returns an error if the user home directory cannot be located.
    pub fn legacy_profile_dir() -> Result<PathBuf> {
        let base_dirs =
            BaseDirs::new().ok_or_else(|| anyhow!("failed to locate the user home directory"))?;
        Ok(base_dirs.home_dir().join(LEGACY_PROFILE_DIR_NAME))
    }
}

fn resolve_profile_dir(
    profile_override: Option<PathBuf>,
    cwd_hint: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = profile_override {
        return Ok(path);
    }
    if let Some(cwd) = cwd_hint
        && let Some(project_profile_dir) = discover_project_profile_dir(cwd)
    {
        return Ok(project_profile_dir);
    }
    default_profile_dir()
}

fn default_profile_dir() -> Result<PathBuf> {
    let base_dirs =
        BaseDirs::new().ok_or_else(|| anyhow!("failed to locate the user home directory"))?;
    Ok(base_dirs.home_dir().join(DEFAULT_PROFILE_DIR_NAME))
}

fn discover_project_profile_dir(cwd: &Path) -> Option<PathBuf> {
    let mut current = Some(cwd);
    while let Some(dir) = current {
        let candidate = dir.join(DEFAULT_PROFILE_DIR_NAME);
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

/// CLI / env-var overrides for the active provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderOverrides {
    /// Provider name override.
    pub provider: Option<String>,
    /// Base URL override.
    pub base_url: Option<String>,
    /// API key override.
    pub api_key: Option<String>,
    /// Model identifier override.
    pub model: Option<String>,
    /// Protocol override.
    pub protocol: Option<ProviderProtocol>,
}

/// Full configuration for a single LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider name (e.g. `"openai"`, `"anthropic"`).
    pub name: String,
    /// API base URL (inferred from protocol if not set).
    pub base_url: Option<String>,
    /// API key for authentication.
    pub api_key: Option<String>,
    /// Model identifier to use.
    pub model: Option<String>,
    /// Wire protocol to use.
    pub protocol: ProviderProtocol,
    /// Request timeout in milliseconds.
    pub timeout_ms: u64,
    /// Maximum output tokens per request.
    pub max_output_tokens: u32,
    /// Maximum number of retries on transient failures.
    #[serde(default = "default_provider_max_retries")]
    pub max_retries: u32,
    /// Initial back-off delay in milliseconds for retries.
    #[serde(default = "default_provider_retry_initial_backoff_ms")]
    pub retry_initial_backoff_ms: u64,
    /// Maximum back-off delay in milliseconds for retries.
    #[serde(default = "default_provider_retry_max_backoff_ms")]
    pub retry_max_backoff_ms: u64,
    /// Whether to respect the `Retry-After` header from the provider.
    #[serde(default = "default_provider_respect_retry_after")]
    pub respect_retry_after: bool,
    /// Additional HTTP headers to send with every request.
    #[serde(default)]
    pub request_header_overrides: BTreeMap<String, String>,
    /// Additional API metadata to send with requests when the provider supports it.
    #[serde(default)]
    pub request_metadata: BTreeMap<String, String>,
    /// Token budget for extended thinking/reasoning (Anthropic Claude only).
    /// When set, enables extended thinking with the specified token budget.
    /// Must be less than `max_output_tokens`.
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    /// Sampling temperature (0.0–2.0). When `None`, the provider default is used.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling threshold (0.0–1.0). When `None`, the provider default is used.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Top-K sampling parameter. When `None`, the provider default is used.
    #[serde(default)]
    pub top_k: Option<u32>,
}

/// Configuration for provider failover / load-balancing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// Ordered list of provider configurations to try.
    pub providers: Vec<ProviderConfig>,
    /// Maximum number of providers to attempt before giving up.
    #[serde(default = "default_failover_max_attempts")]
    pub max_failover_attempts: usize,
    /// HTTP status codes that should trigger a failover to the next provider.
    #[serde(default)]
    pub failover_on_status: Vec<u16>,
    /// Whether a timeout error should trigger failover.
    #[serde(default = "default_true")]
    pub failover_on_timeout: bool,
}

const fn default_failover_max_attempts() -> usize {
    3
}

const fn default_true() -> bool {
    true
}

/// Session-scoped active worktree state restored on resume.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveWorktreeSession {
    /// Original working directory before the session entered the worktree.
    pub original_cwd: PathBuf,
    /// Absolute path to the active worktree.
    pub worktree_path: PathBuf,
    /// Logical worktree name/slug.
    pub worktree_name: String,
    /// Temporary worktree branch, when git-backed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// Branch that was active before entering the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_branch: Option<String>,
    /// Baseline HEAD commit used for safe removal checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_head_commit: Option<String>,
    /// Session id that owns the worktree.
    pub session_id: Uuid,
    /// Optional tmux session tied to this worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session_name: Option<String>,
    /// Whether the worktree came from custom hooks instead of git.
    #[serde(default)]
    pub hook_based: bool,
}

/// Top-level runtime configuration assembled from CLI flags, env vars, and settings.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Original working directory before any runtime cwd/worktree switching.
    pub original_cwd: PathBuf,
    /// Active worktree session owned by the current session, if any.
    pub active_worktree_session: Option<ActiveWorktreeSession>,
    /// Active session identifier.
    pub session_id: Uuid,
    /// Permission mode for tool execution.
    pub permission_mode: PermissionMode,
    /// Input format (text or stream-json).
    pub input_format: InputFormat,
    /// Output format (text or stream-json).
    pub output_format: OutputFormat,
    /// Whether to print and exit (non-interactive mode).
    pub print_mode: bool,
    /// Whether verbose logging is enabled.
    pub verbose: bool,
    /// Whether to replay user messages from the previous session.
    pub replay_user_messages: bool,
    /// Whether to include partial streaming messages in output.
    pub include_partial_messages: bool,
    /// Optional JSON Schema for structured output validation.
    pub structured_output_schema: Option<serde_json::Value>,
    /// Explicit MCP config files or directories supplied by the current entrypoint.
    pub mcp_config_paths: Vec<PathBuf>,
    /// Whether runtime MCP discovery should use only explicit MCP configs.
    pub strict_mcp_config: bool,
    /// Maximum number of conversation turns per session.
    pub max_turns: usize,
    /// Optional human-friendly display name for the session.
    pub session_name: Option<String>,
    /// Optional custom system prompt supplied by the current runtime entrypoint.
    pub system_prompt: Option<String>,
    /// Optional prompt suffix appended after the effective system prompt.
    pub append_system_prompt: Option<String>,
    /// Explanation of which settings layers were applied.
    pub setting_sources: Vec<String>,
    /// Enabled user/project/local setting scopes for startup discovery.
    pub allowed_setting_sources: Vec<SettingSource>,
    /// Concrete settings files loaded for this runtime.
    pub settings_files: Vec<PathBuf>,
    /// Settings files explicitly supplied by CLI/runtime overrides.
    pub cli_settings_files: Vec<PathBuf>,
    /// Tool allow-list applied to the current process.
    pub allowed_tools: Vec<String>,
    /// Tool deny-list applied to the current process.
    pub disallowed_tools: Vec<String>,
    /// Requested reasoning effort level, if configured.
    pub effort: Option<String>,
    /// Fallback model used when no explicit primary model is configured.
    pub fallback_model: Option<String>,
    /// Preferred output style name for prompt construction.
    pub output_style: Option<String>,
    /// Preferred response language for prompt construction.
    pub language: Option<String>,
    /// Whether brief mode is active for the current session.
    pub brief_enabled: bool,
    /// Whether proactive/autonomous mode is active for the current session.
    pub proactive_active: bool,
    /// Explanation of where the active auth token came from.
    pub auth_source: Option<String>,
    /// Command configured via `apiKeyHelper`, if any.
    pub api_key_helper: Option<String>,
    /// Settings source that supplied `apiKeyHelper`, if known.
    pub api_key_helper_source: Option<SettingSource>,
    /// Active provider configuration.
    pub provider: ProviderConfig,
    /// Application paths.
    pub paths: AppPaths,
}

/// Result of a diagnostic check (`doctor` command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Whether all checks passed.
    pub ok: bool,
    /// List of issues found.
    pub issues: Vec<String>,
}

#[allow(clippy::fn_params_excessive_bools)]
#[allow(clippy::too_many_arguments)]
/// # Errors
/// Returns an error if configuration loading fails.
pub fn load_runtime_config(
    cwd_override: Option<PathBuf>,
    profile_dir_override: Option<PathBuf>,
    session_id_override: Option<Uuid>,
    permission_mode: PermissionMode,
    input_format: InputFormat,
    output_format: OutputFormat,
    print_mode: bool,
    verbose: bool,
    replay_user_messages: bool,
    include_partial_messages: bool,
    max_turns: usize,
    overrides: ProviderOverrides,
    runtime_overrides: RuntimeOverrides,
) -> Result<RuntimeConfig> {
    let cwd = match cwd_override {
        Some(cwd) => cwd,
        None => env::current_dir().context("failed to discover the current working directory")?,
    };
    let paths = AppPaths::discover_for_cwd(profile_dir_override, &cwd)?;
    paths.ensure_exists()?;
    let allowed_setting_sources = runtime_overrides
        .allowed_setting_sources
        .clone()
        .unwrap_or_else(|| SettingSource::all().to_vec());

    let resolved_settings_files = resolve_runtime_settings_files(
        &cwd,
        &paths.profile_dir,
        &paths.profiles_dir,
        &runtime_overrides.settings_files,
        &allowed_setting_sources,
    );
    let settings = if runtime_overrides.settings_files.is_empty() {
        let source_hints = discover_runtime_settings_sources(
            &cwd,
            &paths.profile_dir,
            &paths.profiles_dir,
            &allowed_setting_sources,
        )
        .into_iter()
        .map(|source| (source.path, setting_source_for_kind(source.kind)))
        .collect::<Vec<_>>();
        load_runtime_settings_with_source_hints(&source_hints)?
    } else {
        load_runtime_settings(&resolved_settings_files)?
    };
    let provider_overrides = overrides.clone();
    let mut provider = load_provider_config(overrides, session_id_override, &settings)?;
    let effort = runtime_overrides
        .effort
        .clone()
        .or_else(crate::env_vars::effort_level)
        .or_else(|| read_env_first(&["REMOTE_CODE_EFFORT"]))
        .or(settings.effort.clone());
    let fallback_model = runtime_overrides
        .fallback_model
        .clone()
        .or_else(|| read_env_first(&["REMOTE_CODE_FALLBACK_MODEL"]))
        .or(settings.fallback_model.clone());
    let output_style = runtime_overrides
        .output_style
        .clone()
        .or(settings.output_style.clone());
    let language = runtime_overrides
        .language
        .clone()
        .or(settings.language.clone());
    let brief_enabled = runtime_overrides
        .brief_enabled
        .unwrap_or_else(|| read_env_truthy(&["REMOTE_CODE_BRIEF", "CLAUDE_CODE_BRIEF"]));
    let proactive_active = runtime_overrides
        .proactive_active
        .unwrap_or_else(|| read_env_truthy(&["REMOTE_CODE_PROACTIVE", "CLAUDE_CODE_PROACTIVE"]));
    if provider.model.is_none() {
        provider.model = fallback_model.clone();
    }
    // Gap 4: CLAUDE_CODE_MAX_OUTPUT_TOKENS overrides the provider default.
    if let Some(max_tokens) = crate::env_vars::max_output_tokens() {
        provider.max_output_tokens = max_tokens;
    }
    if provider.thinking_budget.is_none()
        && let Some(budget) = effort.as_deref().and_then(effort_to_thinking_budget)
    {
        provider.thinking_budget = Some(budget);
    }
    let mut setting_sources = settings.setting_sources.clone();
    setting_sources.extend(cli_setting_sources(&runtime_overrides, &provider_overrides));
    setting_sources.extend(env_setting_sources());
    if setting_sources.is_empty() {
        setting_sources.push("defaults".to_owned());
    }
    let allowed_tools =
        merge_tool_filters(&settings.allowed_tools, &runtime_overrides.allowed_tools);
    let disallowed_tools = merge_tool_filters(
        &settings.disallowed_tools,
        &runtime_overrides.disallowed_tools,
    );
    let session_name = runtime_overrides
        .session_name
        .clone()
        .or(settings.session_name.clone());
    // When the CLI uses the default permission mode (i.e. the user did not
    // explicitly pass --permission-mode), fall back to the settings file value.
    let effective_permission_mode = if permission_mode == PermissionMode::Default {
        settings
            .permission_mode
            .as_deref()
            .and_then(parse_permission_mode_from_settings)
            .unwrap_or(permission_mode)
    } else {
        permission_mode
    };

    Ok(RuntimeConfig {
        cwd: cwd.clone(),
        original_cwd: cwd,
        active_worktree_session: None,
        session_id: session_id_override.unwrap_or_else(Uuid::new_v4),
        permission_mode: effective_permission_mode,
        input_format,
        output_format,
        print_mode,
        verbose,
        replay_user_messages,
        include_partial_messages,
        structured_output_schema: runtime_overrides.structured_output_schema.clone(),
        mcp_config_paths: runtime_overrides.mcp_config_paths.clone(),
        strict_mcp_config: runtime_overrides.strict_mcp_config,
        max_turns: max_turns.max(1),
        session_name,
        system_prompt: runtime_overrides.system_prompt.clone(),
        append_system_prompt: runtime_overrides.append_system_prompt.clone(),
        setting_sources,
        allowed_setting_sources,
        settings_files: resolved_settings_files,
        cli_settings_files: runtime_overrides.settings_files.clone(),
        allowed_tools,
        disallowed_tools,
        effort,
        fallback_model,
        output_style,
        language,
        brief_enabled,
        proactive_active,
        auth_source: resolve_auth_source(&provider_overrides, &settings, &provider),
        api_key_helper: settings.api_key_helper.clone(),
        api_key_helper_source: settings.api_key_helper_source,
        provider,
        paths,
    })
}

/// Re-stamp a runtime config with a new session id and refresh provider-side
/// session metadata derived from that id.
pub fn restamp_runtime_session(config: &mut RuntimeConfig, session_id: Uuid) {
    config.session_id = session_id;
    config.provider.request_header_overrides = build_request_header_overrides(Some(session_id));
    config.provider.request_metadata = build_request_metadata(Some(session_id));
}

/// # Errors
/// Returns an error if provider configuration is invalid.
pub fn load_provider_config(
    overrides: ProviderOverrides,
    session_id: Option<Uuid>,
    settings: &ResolvedRuntimeSettings,
) -> Result<ProviderConfig> {
    let provider_name = overrides
        .provider
        .or_else(|| read_env_first(&["REMOTE_CODE_PROVIDER"]))
        .or_else(|| settings.provider_name.clone())
        .unwrap_or_else(|| "custom".to_owned());
    let base_url = overrides
        .base_url
        .or_else(|| {
            read_env_first(&[
                "REMOTE_CODE_BASE_URL",
                "OPENAI_BASE_URL",
                "ANTHROPIC_BASE_URL",
            ])
        })
        .or_else(|| settings.base_url.clone())
        .or_else(|| default_base_url_for_provider(&provider_name));
    let explicit_protocol = overrides
        .protocol
        .or_else(|| {
            read_env_first(&["REMOTE_CODE_PROTOCOL", "REMOTE_CODE_PROVIDER_PROTOCOL"]).and_then(
                |raw| match raw.to_ascii_lowercase().as_str() {
                    "openai" => Some(ProviderProtocol::OpenAi),
                    "anthropic" => Some(ProviderProtocol::Anthropic),
                    "bedrock" => Some(ProviderProtocol::Bedrock),
                    "vertex" => Some(ProviderProtocol::Vertex),
                    _ => None,
                },
            )
        })
        .or(settings.protocol);
    let protocol = normalize_protocol(base_url.as_deref(), explicit_protocol);
    let normalized_base_url = normalize_base_url(base_url, protocol);
    let timeout_ms = read_env_first(&["REMOTE_CODE_API_TIMEOUT_MS", "API_TIMEOUT_MS"])
        .and_then(|value| value.parse::<u64>().ok())
        .or(settings.timeout_ms)
        .unwrap_or(600_000)
        .max(1_000);
    let max_output_tokens = read_env_first(&["REMOTE_CODE_MAX_OUTPUT_TOKENS"])
        .and_then(|value| value.parse::<u32>().ok())
        .or(settings.max_output_tokens)
        .unwrap_or(4_096)
        .max(256);
    let max_retries = read_env_first(&["REMOTE_CODE_PROVIDER_MAX_RETRIES"])
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default_provider_max_retries());
    let retry_initial_backoff_ms =
        read_env_first(&["REMOTE_CODE_PROVIDER_RETRY_INITIAL_BACKOFF_MS"])
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(default_provider_retry_initial_backoff_ms())
            .max(50);
    let retry_max_backoff_ms = read_env_first(&["REMOTE_CODE_PROVIDER_RETRY_MAX_BACKOFF_MS"])
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_provider_retry_max_backoff_ms())
        .max(retry_initial_backoff_ms);
    let respect_retry_after = read_env_first(&["REMOTE_CODE_PROVIDER_RESPECT_RETRY_AFTER"])
        .map_or(default_provider_respect_retry_after(), |value| {
            !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no")
        });
    let request_header_overrides = build_request_header_overrides(session_id);
    let request_metadata = build_request_metadata(session_id);
    let model = overrides
        .model
        .or_else(|| provider_model_from_env(protocol))
        .or_else(|| default_model_for_provider(provider_name.as_str(), protocol))
        .or_else(|| settings.model.clone());
    let mut provider = ProviderConfig {
        name: provider_name,
        base_url: normalized_base_url,
        api_key: None,
        model,
        protocol,
        timeout_ms,
        max_output_tokens,
        max_retries,
        retry_initial_backoff_ms,
        retry_max_backoff_ms,
        respect_retry_after,
        request_header_overrides,
        request_metadata,
        thinking_budget: settings.thinking_budget,
        temperature: crate::env_vars::temperature(),
        top_p: crate::env_vars::top_p(),
        top_k: crate::env_vars::top_k(),
    };
    let discovered = discover_env_providers();
    let keep_explicit_protocol = explicit_protocol.is_some() || provider.base_url.is_some();
    hydrate_provider_from_discovered(&mut provider, &discovered, keep_explicit_protocol);
    if provider.api_key.is_none() {
        let mut lookup = |keys: &[&str]| read_env_first(keys);
        provider.api_key = provider_api_key_from_lookup(&provider, &mut lookup);
    }
    provider.api_key = overrides
        .api_key
        .or(provider.api_key)
        .or_else(|| settings.api_key.clone());
    Ok(provider)
}

fn hydrate_provider_from_discovered(
    provider: &mut ProviderConfig,
    discovered: &[ProviderConfig],
    keep_explicit_protocol: bool,
) {
    let matching = discovered
        .iter()
        .find(|candidate| candidate.name == provider.name)
        .or_else(|| {
            if provider.name == "custom"
                && provider.base_url.is_some()
                && discovered
                    .iter()
                    .any(|candidate| provider_matches_discovered_endpoint(provider, candidate))
            {
                discovered
                    .iter()
                    .find(|candidate| provider_matches_discovered_endpoint(provider, candidate))
            } else {
                None
            }
        })
        .or_else(|| {
            if provider.name == "custom"
                && provider.base_url.is_none()
                && provider.api_key.is_none()
                && provider.model.is_none()
                && discovered.len() == 1
            {
                discovered.first()
            } else {
                None
            }
        });
    let Some(candidate) = matching else {
        return;
    };
    if provider.name == "custom" {
        provider.name = candidate.name.clone();
    }
    if provider.base_url.is_none() {
        provider.base_url = candidate.base_url.clone();
    }
    if provider.api_key.is_none() {
        provider.api_key = candidate.api_key.clone();
    }
    if provider.model.is_none() {
        provider.model = candidate.model.clone();
    }
    if !keep_explicit_protocol {
        provider.protocol = candidate.protocol;
    }
}

fn provider_matches_discovered_endpoint(
    provider: &ProviderConfig,
    candidate: &ProviderConfig,
) -> bool {
    provider.protocol == candidate.protocol
        && provider.base_url.is_some()
        && provider.base_url == candidate.base_url
}

fn default_base_url_for_provider(provider_name: &str) -> Option<String> {
    if provider_name.eq_ignore_ascii_case("anthropic") {
        Some("https://api.anthropic.com".to_owned())
    } else {
        None
    }
}

fn provider_model_from_env(protocol: ProviderProtocol) -> Option<String> {
    read_env_first(&["REMOTE_CODE_MODEL"]).or_else(|| match protocol {
        ProviderProtocol::Anthropic => read_env_first(&[
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
        ]),
        ProviderProtocol::OpenAi => read_env_first(&["OPENAI_MODEL"]),
        _ => None,
    })
}

fn default_model_for_provider(provider_name: &str, protocol: ProviderProtocol) -> Option<String> {
    if provider_name.eq_ignore_ascii_case("anthropic")
        || matches!(protocol, ProviderProtocol::Anthropic)
            && provider_name.eq_ignore_ascii_case("custom")
    {
        Some("claude-sonnet-4-6".to_owned())
    } else {
        None
    }
}

#[must_use]
pub fn validate_provider_config(provider: &ProviderConfig) -> DoctorReport {
    let mut issues = Vec::new();
    if provider.base_url.is_none() {
        issues.push("Missing REMOTE_CODE_BASE_URL (or provider-compatible base URL).".to_owned());
    }
    if provider.model.is_none() {
        issues.push("Missing REMOTE_CODE_MODEL.".to_owned());
    }
    if provider.api_key.is_none() && provider.name != "mock" {
        issues.push(format!(
            "Missing API key. Set {}.",
            expected_provider_auth_inputs(provider)
        ));
    }
    DoctorReport {
        ok: issues.is_empty(),
        issues,
    }
}

#[must_use]
pub fn normalize_protocol(
    base_url: Option<&str>,
    explicit_protocol: Option<ProviderProtocol>,
) -> ProviderProtocol {
    if let Some(protocol) = explicit_protocol {
        return protocol;
    }
    let Some(base_url) = base_url else {
        return ProviderProtocol::OpenAi;
    };
    let normalized = base_url.to_ascii_lowercase();
    if normalized.ends_with("/messages")
        || normalized.contains("/anthropic")
        || normalized.contains("compat=anthropic")
    {
        ProviderProtocol::Anthropic
    } else {
        ProviderProtocol::OpenAi
    }
}

#[must_use]
pub fn normalize_base_url(base_url: Option<String>, protocol: ProviderProtocol) -> Option<String> {
    let raw = base_url?;
    let trimmed = raw.trim().trim_end_matches('/').to_owned();
    let normalized = match protocol {
        ProviderProtocol::Anthropic => {
            if trimmed.ends_with("/messages") {
                trimmed
            } else if trimmed.rsplit('/').next().is_some_and(|segment| {
                segment.starts_with('v') && segment[1..].chars().all(|ch| ch.is_ascii_digit())
            }) {
                format!("{trimmed}/messages")
            } else {
                format!("{trimmed}/v1/messages")
            }
        }
        ProviderProtocol::OpenAi => {
            if trimmed.ends_with("/chat/completions") {
                trimmed
            } else {
                format!("{trimmed}/chat/completions")
            }
        }
        // Bedrock and Vertex use OpenAI-compatible endpoints for now.
        ProviderProtocol::Bedrock | ProviderProtocol::Vertex => trimmed,
    };
    Some(normalized)
}

fn build_request_header_overrides(session_id: Option<Uuid>) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();
    if let Some(raw) = read_env_first(&["ANTHROPIC_CUSTOM_HEADERS", "REMOTE_CODE_CUSTOM_HEADERS"]) {
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((name, value)) = trimmed.split_once(':') else {
                continue;
            };
            let name = name.trim();
            let value = value.trim();
            if !name.is_empty() && !value.is_empty() {
                merged.insert(name.to_owned(), value.to_owned());
            }
        }
    }

    if let Some(raw) = read_env_first(&["REMOTE_CODE_REQUEST_HEADERS_JSON"])
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(object) = value.as_object()
    {
        for (name, raw_value) in object {
            let normalized = match raw_value {
                serde_json::Value::String(value) => Some(value.trim().to_owned()),
                serde_json::Value::Number(value) => Some(value.to_string()),
                serde_json::Value::Bool(value) => Some(value.to_string()),
                _ => None,
            };
            if let Some(value) = normalized
                && !value.is_empty()
            {
                merged.insert(name.trim().to_owned(), value);
            }
        }
    }

    let session = session_id
        .map(|value| value.to_string())
        .unwrap_or_default();
    let mut filtered = BTreeMap::new();
    for (name, value) in merged {
        if RESERVED_PROVIDER_HEADER_NAMES
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name.as_str()))
        {
            continue;
        }
        let resolved = value
            .replace("${REMOTE_CODE_SESSION_ID}", &session)
            .replace("${REMOTE_CODE_VERSION}", runtime_version());
        filtered.insert(name, resolved);
    }
    filtered
}

fn build_request_metadata(session_id: Option<Uuid>) -> BTreeMap<String, String> {
    let device_id = get_or_create_device_id();
    let mut metadata = BTreeMap::from([
        ("device_id".to_owned(), device_id),
        ("account_uuid".to_owned(), String::new()),
    ]);
    if let Some(session_id) = session_id {
        metadata.insert("session_id".to_owned(), session_id.to_string());
    }

    if let Some(raw) = read_env_first(&[
        "REMOTE_CODE_REQUEST_METADATA_JSON",
        "REMOTE_CODE_EXTRA_METADATA_JSON",
    ]) && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(object) = value.as_object()
    {
        for (name, raw_value) in object {
            let normalized = match raw_value {
                serde_json::Value::String(value) => Some(value.trim().to_owned()),
                serde_json::Value::Number(value) => Some(value.to_string()),
                serde_json::Value::Bool(value) => Some(value.to_string()),
                _ => None,
            };
            if let Some(value) = normalized
                && !value.is_empty()
            {
                metadata.insert(name.trim().to_owned(), value);
            }
        }
    }

    metadata
}

/// Get or create a persistent device ID, matching the official CLI's
/// `getOrCreateUserID()` behavior (64-char hex string persisted in config).
fn get_or_create_device_id() -> String {
    let config_path = default_profile_dir().ok().map(|dir| dir.join("device_id"));

    if let Some(ref path) = config_path
        && let Ok(existing) = fs::read_to_string(path)
    {
        let trimmed = existing.trim().to_owned();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }

    // Generate a 64-char hex string (256 bits) matching crypto.randomBytes(32).toString('hex')
    let id = format!(
        "{}{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
    );
    // Take first 64 chars
    let id = &id[..64];

    if let Some(ref path) = config_path {
        let _ = fs::write(path, id);
    }

    id.to_owned()
}

/// Parse a permission mode string from settings files.
///
/// Accepts camelCase (`bypassPermissions`), kebab-case (`bypass-permissions`),
/// and snake_case (`bypass_permissions`) formats.
fn parse_permission_mode_from_settings(mode: &str) -> Option<PermissionMode> {
    match mode {
        "default" | "Default" => Some(PermissionMode::Default),
        "acceptEdits" | "accept-edits" | "accept_edits" => Some(PermissionMode::AcceptEdits),
        "auto" | "Auto" => Some(PermissionMode::Auto),
        "bypassPermissions" | "bypass-permissions" | "bypass_permissions" => {
            Some(PermissionMode::BypassPermissions)
        }
        "dontAsk" | "dont-ask" | "dont_ask" => Some(PermissionMode::DontAsk),
        "plan" | "Plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

fn read_env_first(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
    })
}

fn read_env_truthy(keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        env::var(key).ok().is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

fn cli_setting_sources(
    runtime_overrides: &RuntimeOverrides,
    overrides: &ProviderOverrides,
) -> Vec<String> {
    let mut sources = Vec::new();
    if overrides.provider.is_some() {
        sources.push("cli:provider".to_owned());
    }
    if overrides.base_url.is_some() {
        sources.push("cli:base-url".to_owned());
    }
    if overrides.api_key.is_some() {
        sources.push("cli:api-key".to_owned());
    }
    if overrides.model.is_some() {
        sources.push("cli:model".to_owned());
    }
    if overrides.protocol.is_some() {
        sources.push("cli:protocol".to_owned());
    }
    if runtime_overrides.session_name.is_some() {
        sources.push("cli:name".to_owned());
    }
    if !runtime_overrides.allowed_tools.is_empty() {
        sources.push("cli:allowed-tools".to_owned());
    }
    if !runtime_overrides.disallowed_tools.is_empty() {
        sources.push("cli:disallowed-tools".to_owned());
    }
    if runtime_overrides.structured_output_schema.is_some() {
        sources.push("cli:json-schema".to_owned());
    }
    if !runtime_overrides.mcp_config_paths.is_empty() {
        sources.push("cli:mcp-config".to_owned());
    }
    if runtime_overrides.strict_mcp_config {
        sources.push("cli:strict-mcp-config".to_owned());
    }
    if runtime_overrides.effort.is_some() {
        sources.push("cli:effort".to_owned());
    }
    if runtime_overrides.fallback_model.is_some() {
        sources.push("cli:fallback-model".to_owned());
    }
    if runtime_overrides.output_style.is_some() {
        sources.push("cli:output-style".to_owned());
    }
    if runtime_overrides.language.is_some() {
        sources.push("cli:language".to_owned());
    }
    if runtime_overrides.brief_enabled.is_some() {
        sources.push("cli:brief".to_owned());
    }
    if runtime_overrides.proactive_active.is_some() {
        sources.push("cli:proactive".to_owned());
    }
    if runtime_overrides.allowed_setting_sources.is_some() {
        sources.push("cli:setting-sources".to_owned());
    }
    sources
}

fn env_setting_source_keys() -> &'static [(&'static str, &'static str)] {
    &[
        ("REMOTE_CODE_PROVIDER", "env:REMOTE_CODE_PROVIDER"),
        ("REMOTE_CODE_BASE_URL", "env:REMOTE_CODE_BASE_URL"),
        ("OPENAI_BASE_URL", "env:OPENAI_BASE_URL"),
        ("ANTHROPIC_BASE_URL", "env:ANTHROPIC_BASE_URL"),
        ("REMOTE_CODE_API_KEY", "env:REMOTE_CODE_API_KEY"),
        ("ANTHROPIC_API_KEY", "env:ANTHROPIC_API_KEY"),
        ("OPENAI_API_KEY", "env:OPENAI_API_KEY"),
        ("REMOTE_CODE_MODEL", "env:REMOTE_CODE_MODEL"),
        (
            "REMOTE_CODE_ANTHROPIC_MODEL",
            "env:REMOTE_CODE_ANTHROPIC_MODEL",
        ),
        ("ANTHROPIC_MODEL", "env:ANTHROPIC_MODEL"),
        ("ANTHROPIC_DEFAULT_MODEL", "env:ANTHROPIC_DEFAULT_MODEL"),
        (
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "env:ANTHROPIC_DEFAULT_SONNET_MODEL",
        ),
        ("OPENAI_MODEL", "env:OPENAI_MODEL"),
        ("REMOTE_CODE_PROTOCOL", "env:REMOTE_CODE_PROTOCOL"),
        (
            "REMOTE_CODE_PROVIDER_PROTOCOL",
            "env:REMOTE_CODE_PROVIDER_PROTOCOL",
        ),
        ("REMOTE_CODE_EFFORT", "env:REMOTE_CODE_EFFORT"),
        (
            "REMOTE_CODE_FALLBACK_MODEL",
            "env:REMOTE_CODE_FALLBACK_MODEL",
        ),
        ("REMOTE_CODE_BRIEF", "env:REMOTE_CODE_BRIEF"),
        ("CLAUDE_CODE_BRIEF", "env:CLAUDE_CODE_BRIEF"),
        ("REMOTE_CODE_PROACTIVE", "env:REMOTE_CODE_PROACTIVE"),
        ("CLAUDE_CODE_PROACTIVE", "env:CLAUDE_CODE_PROACTIVE"),
        ("GLM_API_KEY", "env:GLM_API_KEY"),
        ("GLM_CODING_PLAN_API_KEY", "env:GLM_CODING_PLAN_API_KEY"),
        (
            "MINIMAX_TOKEN_PLAN_API_KEY",
            "env:MINIMAX_TOKEN_PLAN_API_KEY",
        ),
        ("MINIMAX_API_KEY", "env:MINIMAX_API_KEY"),
        (
            "MINIMAX_TOKEN_PLAN_BASE_URL",
            "env:MINIMAX_TOKEN_PLAN_BASE_URL",
        ),
        (
            "MINIMAX_ANTHROPIC_BASE_URL",
            "env:MINIMAX_ANTHROPIC_BASE_URL",
        ),
        ("MINIMAX_API_HOST", "env:MINIMAX_API_HOST"),
        ("MINIMAX_TOKEN_PLAN_MODEL", "env:MINIMAX_TOKEN_PLAN_MODEL"),
        (
            "KUAIKAT_CODING_PLAN_API_KEY",
            "env:KUAIKAT_CODING_PLAN_API_KEY",
        ),
        ("KUAIKAT_API_KEY", "env:KUAIKAT_API_KEY"),
        (
            "KUAIKAT_CODING_PLAN_BASE_URL",
            "env:KUAIKAT_CODING_PLAN_BASE_URL",
        ),
        (
            "KUAIKAT_ANTHROPIC_BASE_URL",
            "env:KUAIKAT_ANTHROPIC_BASE_URL",
        ),
        ("KUAIKAT_CODING_PLAN_MODEL", "env:KUAIKAT_CODING_PLAN_MODEL"),
        ("KUAIKAT_MODEL", "env:KUAIKAT_MODEL"),
        ("DEEPSEEK_API_KEY", "env:DEEPSEEK_API_KEY"),
        (
            "DEEPSEEK_CODING_PLAN_API_KEY",
            "env:DEEPSEEK_CODING_PLAN_API_KEY",
        ),
        (
            "DEEPSEEK_ANTHROPIC_BASE_URL",
            "env:DEEPSEEK_ANTHROPIC_BASE_URL",
        ),
        ("DEEPSEEK_MODEL", "env:DEEPSEEK_MODEL"),
        (
            "DEEPSEEK_CODING_PLAN_MODEL",
            "env:DEEPSEEK_CODING_PLAN_MODEL",
        ),
        (
            "MINIMAX_CODING_PLAN_API_KEY",
            "env:MINIMAX_CODING_PLAN_API_KEY",
        ),
        (
            "MINIMAX_CODING_PLAN_BASE_URL",
            "env:MINIMAX_CODING_PLAN_BASE_URL",
        ),
        ("MINIMAX_CODING_PLAN_MODEL", "env:MINIMAX_CODING_PLAN_MODEL"),
        (
            "ALIYUN_CODING_PLAN_API_KEY",
            "env:ALIYUN_CODING_PLAN_API_KEY",
        ),
        ("ALIYUN_CODING_MODEL", "env:ALIYUN_CODING_MODEL"),
        (
            "TENCENT_CODING_PLAN_API_KEY",
            "env:TENCENT_CODING_PLAN_API_KEY",
        ),
        ("TENCENT_CODING_MODEL", "env:TENCENT_CODING_MODEL"),
        (
            "QIANFAN_CODING_PLAN_API_KEY",
            "env:QIANFAN_CODING_PLAN_API_KEY",
        ),
        ("QIANFAN_CODING_MODEL", "env:QIANFAN_CODING_MODEL"),
        ("KIMI_CODING_PLAN_API_KEY", "env:KIMI_CODING_PLAN_API_KEY"),
        ("KIMI_CODING_MODEL", "env:KIMI_CODING_MODEL"),
        (
            "VOLCENGINE_CODING_PLAN_API_KEY",
            "env:VOLCENGINE_CODING_PLAN_API_KEY",
        ),
        (
            "VOLCENGINE_CODING_BASE_URL",
            "env:VOLCENGINE_CODING_BASE_URL",
        ),
        ("VOLCENGINE_CODING_MODEL", "env:VOLCENGINE_CODING_MODEL"),
        ("BEDROCK_MODEL", "env:BEDROCK_MODEL"),
        ("AWS_ACCESS_KEY_ID", "env:AWS_ACCESS_KEY_ID"),
        ("AWS_SECRET_ACCESS_KEY", "env:AWS_SECRET_ACCESS_KEY"),
        ("AWS_REGION", "env:AWS_REGION"),
        (
            "GOOGLE_APPLICATION_CREDENTIALS",
            "env:GOOGLE_APPLICATION_CREDENTIALS",
        ),
        ("VERTEX_PROJECT", "env:VERTEX_PROJECT"),
        ("VERTEX_REGION", "env:VERTEX_REGION"),
        ("VERTEX_MODEL", "env:VERTEX_MODEL"),
    ]
}

fn env_setting_sources() -> Vec<String> {
    env_setting_source_keys()
        .iter()
        .filter_map(|(key, label)| env::var(key).ok().map(|_| (*label).to_owned()))
        .collect()
}

fn resolve_auth_source(
    overrides: &ProviderOverrides,
    settings: &ResolvedRuntimeSettings,
    provider: &ProviderConfig,
) -> Option<String> {
    resolve_auth_source_with_lookup(overrides, settings, provider, &mut |keys| {
        read_env_first(keys)
    })
}

fn resolve_auth_source_with_lookup<F>(
    overrides: &ProviderOverrides,
    settings: &ResolvedRuntimeSettings,
    provider: &ProviderConfig,
    lookup: &mut F,
) -> Option<String>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    if overrides.api_key.is_some() {
        return Some("cli:api-key".to_owned());
    }

    if let Some(settings_api_key) = settings.api_key.as_deref()
        && provider.api_key.as_deref() == Some(settings_api_key)
    {
        return settings.auth_source.clone();
    }

    if settings.api_key_helper.is_some() && provider.api_key.is_none() {
        return Some("apiKeyHelper".to_owned());
    }

    provider_auth_source_from_lookup(provider, lookup).or_else(|| settings.auth_source.clone())
}

fn provider_api_key_from_lookup<F>(provider: &ProviderConfig, lookup: &mut F) -> Option<String>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    provider_auth_candidates(provider)
        .iter()
        .find_map(|(keys, _)| lookup(keys))
}

fn provider_auth_source_from_lookup<F>(provider: &ProviderConfig, lookup: &mut F) -> Option<String>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    provider_auth_candidates(provider)
        .iter()
        .find_map(|(keys, source)| {
            let value = lookup(keys)?;
            if provider.api_key.is_none() || provider.api_key.as_deref() == Some(value.as_str()) {
                Some((*source).to_owned())
            } else {
                None
            }
        })
}

fn provider_auth_candidates(
    provider: &ProviderConfig,
) -> Vec<(&'static [&'static str], &'static str)> {
    let mut candidates = provider_specific_auth_candidates(provider.name.as_str()).to_vec();
    candidates.extend(protocol_auth_candidates(provider.protocol));
    candidates
}

fn provider_specific_auth_candidates(
    provider_name: &str,
) -> &'static [(&'static [&'static str], &'static str)] {
    match provider_name {
        "anthropic" => &[
            (&["REMOTE_CODE_API_KEY"], "env:REMOTE_CODE_API_KEY"),
            (&["ANTHROPIC_API_KEY"], "env:ANTHROPIC_API_KEY"),
        ],
        "openai" => &[(&["OPENAI_API_KEY"], "env:OPENAI_API_KEY")],
        "glm" => &[(&["GLM_API_KEY"], "env:GLM_API_KEY")],
        "glm-coding" => &[(&["GLM_CODING_PLAN_API_KEY"], "env:GLM_CODING_PLAN_API_KEY")],
        "minimax-token-plan" => &[
            (
                &["MINIMAX_TOKEN_PLAN_API_KEY"],
                "env:MINIMAX_TOKEN_PLAN_API_KEY",
            ),
            (&["MINIMAX_API_KEY"], "env:MINIMAX_API_KEY"),
        ],
        "kuaikat-coding" => &[
            (
                &["KUAIKAT_CODING_PLAN_API_KEY"],
                "env:KUAIKAT_CODING_PLAN_API_KEY",
            ),
            (&["KUAIKAT_API_KEY"], "env:KUAIKAT_API_KEY"),
        ],
        "deepseek-anthropic" => &[
            (&["DEEPSEEK_API_KEY"], "env:DEEPSEEK_API_KEY"),
            (
                &["DEEPSEEK_CODING_PLAN_API_KEY"],
                "env:DEEPSEEK_CODING_PLAN_API_KEY",
            ),
        ],
        "minimax-coding" => &[(
            &["MINIMAX_CODING_PLAN_API_KEY"],
            "env:MINIMAX_CODING_PLAN_API_KEY",
        )],
        "aliyun-coding" => &[(
            &["ALIYUN_CODING_PLAN_API_KEY"],
            "env:ALIYUN_CODING_PLAN_API_KEY",
        )],
        "tencent-coding" => &[(
            &["TENCENT_CODING_PLAN_API_KEY"],
            "env:TENCENT_CODING_PLAN_API_KEY",
        )],
        "qianfan-coding" => &[(
            &["QIANFAN_CODING_PLAN_API_KEY"],
            "env:QIANFAN_CODING_PLAN_API_KEY",
        )],
        "kimi-coding" => &[(
            &["KIMI_CODING_PLAN_API_KEY"],
            "env:KIMI_CODING_PLAN_API_KEY",
        )],
        "volcengine-coding" => &[(
            &["VOLCENGINE_CODING_PLAN_API_KEY"],
            "env:VOLCENGINE_CODING_PLAN_API_KEY",
        )],
        "bedrock" => &[(&["AWS_ACCESS_KEY_ID"], "env:AWS_ACCESS_KEY_ID")],
        "vertex" => &[
            (
                &["GOOGLE_APPLICATION_CREDENTIALS"],
                "env:GOOGLE_APPLICATION_CREDENTIALS",
            ),
            (&["VERTEX_PROJECT"], "env:VERTEX_PROJECT"),
        ],
        _ => &[],
    }
}

fn protocol_auth_candidates(
    protocol: ProviderProtocol,
) -> &'static [(&'static [&'static str], &'static str)] {
    match protocol {
        ProviderProtocol::Anthropic => &[
            (&["REMOTE_CODE_API_KEY"], "env:REMOTE_CODE_API_KEY"),
            (&["ANTHROPIC_API_KEY"], "env:ANTHROPIC_API_KEY"),
        ],
        ProviderProtocol::OpenAi => &[(&["OPENAI_API_KEY"], "env:OPENAI_API_KEY")],
        _ => &[],
    }
}

fn expected_provider_auth_inputs(provider: &ProviderConfig) -> String {
    let inputs = provider_auth_candidates(provider)
        .iter()
        .filter_map(|(keys, _)| keys.first().copied())
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        return "`--api-key` or provider-specific credentials".to_owned();
    }
    format_auth_inputs(&inputs)
}

fn format_auth_inputs(inputs: &[&str]) -> String {
    let mut unique = Vec::new();
    for input in inputs {
        if !unique.iter().any(|existing: &&str| existing == input) {
            unique.push(*input);
        }
    }
    let rendered = unique
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>();
    match rendered.as_slice() {
        [] => "`--api-key`".to_owned(),
        [only] => format!("{only} or `--api-key`"),
        [first, second] => format!("{first}, {second}, or `--api-key`"),
        _ => {
            let tail = rendered
                .last()
                .cloned()
                .unwrap_or_else(|| "`--api-key`".to_owned());
            format!("{}, or {tail}", rendered[..rendered.len() - 1].join(", "))
        }
    }
}

fn effort_to_thinking_budget(effort: &str) -> Option<u32> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" => Some(2_048),
        "medium" => Some(8_192),
        "high" => Some(16_384),
        _ => None,
    }
}

/// Discover provider configurations from well-known environment variables.
///
/// Checks for the following keys and creates a [`ProviderConfig`] for each one
/// that is present:
///
/// ## Standard API Providers
///
/// | Env var                        | Provider name | Protocol  | Base URL                                              | Model          |
/// |--------------------------------|---------------|-----------|-------------------------------------------------------|----------------|
/// | `GLM_API_KEY`                  | `glm`         | `openai`  | `https://open.bigmodel.cn/api/paas/v4`                | `glm-5.1`      |
/// | `REMOTE_CODE_API_KEY`          | `anthropic`   | `anthropic`| *(default)*                                           | *(default)*    |
/// | `OPENAI_API_KEY`               | `openai`      | `openai`  | *(default)*                                           | *(default)*    |
///
/// Additional overrides for the Anthropic provider:
/// - `REMOTE_CODE_ANTHROPIC_BASE_URL` — custom Anthropic API base URL
/// - `REMOTE_CODE_ANTHROPIC_MODEL` — custom Anthropic model name
/// - `ANTHROPIC_CUSTOM_HEADERS` / `REMOTE_CODE_CUSTOM_HEADERS` — custom HTTP headers (colon-separated `Name: Value` per line)
///
/// ## Coding Plan Providers (Subscription-based AI Coding)
///
/// Providers that support the **Anthropic** wire protocol are configured with
/// it by default so that our Claude Code–style request headers result in
/// priority treatment from the upstream.
///
/// | Env var                          | Provider name         | Protocol    | Base URL                                                              | Model              |
/// |----------------------------------|----------------------|-------------|-----------------------------------------------------------------------|--------------------|
/// | `GLM_CODING_PLAN_API_KEY`        | `glm-coding`         | `anthropic` | `https://open.bigmodel.cn/api/anthropic`                              | `glm-5.1`          |
/// | `ALIYUN_CODING_PLAN_API_KEY`     | `aliyun-coding`      | `anthropic` | `https://coding.dashscope.aliyuncs.com/apps/anthropic`                | `qwen3.6-plus`     |
/// | `TENCENT_CODING_PLAN_API_KEY`    | `tencent-coding`     | `anthropic` | `https://api.lkeap.cloud.tencent.com/coding/anthropic`                | `tc-code-latest`   |
/// | `QIANFAN_CODING_PLAN_API_KEY`    | `qianfan-coding`     | `anthropic` | `https://qianfan.baidubce.com/anthropic/coding`                       | `qianfan-code-latest` |
/// | `MINIMAX_TOKEN_PLAN_API_KEY` / `MINIMAX_API_KEY` | `minimax-token-plan` | `anthropic` | `https://api.minimaxi.com/anthropic`                                  | `minimax-m2.7`     |
/// | `KUAIKAT_CODING_PLAN_API_KEY`    | `kuaikat-coding`     | `anthropic` | `https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy` | `kat-coder-pro-v2` |
/// | `DEEPSEEK_API_KEY`               | `deepseek-anthropic` | `anthropic` | `https://api.deepseek.com/anthropic`                                  | `deepseek-v4-flash` |
/// | `MINIMAX_CODING_PLAN_API_KEY`    | `minimax-coding`     | `openai`    | `https://api.minimax.chat/v1`                                         | `MiniMax-M2.7`     |
/// | `KIMI_CODING_PLAN_API_KEY`       | `kimi-coding`        | `openai`    | `https://api.moonshot.cn/kimi-component/ai_coding`                    | `kimi-k2.5`        |
/// | `VOLCENGINE_CODING_PLAN_API_KEY` | `volcengine-coding`  | `openai`    | `https://ark.cn-beijing.volces.com/api/v3`                            | `doubao-seed-1-5`  |
///
/// The returned list only contains entries for keys that are actually set.
/// This function is intended to be called **before** the main
/// [`load_provider_config`] so that the discovered providers can be merged or
/// offered as fallbacks.
///
/// # Coding Plan Notes
///
/// Coding Plans are subscription-based AI coding services that differ from standard APIs:
/// - They use dedicated endpoints (not standard API endpoints)
/// - They offer fixed monthly quotas instead of per-token billing
/// - They are designed for AI coding tools (Claude Code, Cursor, etc.)
/// - API keys from Coding Plans cannot be used with standard API endpoints
/// - Providers that support the Anthropic protocol receive Claude Code–style
///   request headers for priority treatment
pub fn discover_env_providers() -> Vec<ProviderConfig> {
    let mut providers = Vec::new();

    // ==========================================================================
    // Standard API Providers
    // ==========================================================================

    // GLM / ZhipuAI — OpenAI-compatible endpoint (standard API)
    if let Some(api_key) = read_env_first(&["GLM_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://open.bigmodel.cn/api/paas/v4".to_owned()),
            ProviderProtocol::OpenAi,
        );
        providers.push(ProviderConfig {
            name: "glm".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: Some("glm-5.1".to_owned()),
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Anthropic / first-party Claude API.
    {
        let mut lookup = |keys: &[&str]| read_env_first(keys);
        if let Some(provider) = discover_anthropic_provider(&mut lookup) {
            providers.push(provider);
        }
    }

    // OpenAI
    if let Some(api_key) = read_env_first(&["OPENAI_API_KEY"]) {
        let base_url = normalize_base_url(
            read_env_first(&["OPENAI_BASE_URL"]),
            ProviderProtocol::OpenAi,
        );
        providers.push(ProviderConfig {
            name: "openai".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["OPENAI_MODEL"]),
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // AWS Bedrock — discovered via AWS credentials
    if read_env_first(&["AWS_ACCESS_KEY_ID"]).is_some()
        && read_env_first(&["AWS_SECRET_ACCESS_KEY"]).is_some()
    {
        let region = read_env_first(&["AWS_REGION"]).unwrap_or_else(|| "us-east-1".to_owned());
        let base_url = Some(format!("https://bedrock-runtime.{region}.amazonaws.com"));
        providers.push(ProviderConfig {
            name: "bedrock".to_owned(),
            base_url,
            api_key: None, // Bedrock uses SigV4, not Bearer tokens
            model: read_env_first(&["BEDROCK_MODEL"]),
            protocol: ProviderProtocol::Bedrock,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Google Vertex AI — discovered via GCP credentials or project env
    if read_env_first(&["GOOGLE_APPLICATION_CREDENTIALS"]).is_some()
        || read_env_first(&["VERTEX_PROJECT"]).is_some()
    {
        let project = read_env_first(&["VERTEX_PROJECT"]).unwrap_or_else(|| "default".to_owned());
        let region = read_env_first(&["VERTEX_REGION"]).unwrap_or_else(|| "us-central1".to_owned());
        let base_url = Some(format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/endpoints/openapi"
        ));
        providers.push(ProviderConfig {
            name: "vertex".to_owned(),
            base_url,
            api_key: None, // Vertex uses OAuth2, not Bearer tokens
            model: read_env_first(&["VERTEX_MODEL"]),
            protocol: ProviderProtocol::Vertex,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // ==========================================================================
    // Coding Plan Providers (Subscription-based AI Coding)
    // ==========================================================================
    //
    // Coding Plans are different from standard APIs:
    // - They use dedicated endpoints specific to each provider
    // - They offer fixed monthly quotas (not per-token billing)
    // - They are optimized for AI coding tools (Claude Code, Cursor, etc.)
    // - Coding Plan API keys CANNOT be used with standard API endpoints

    // GLM Coding Plan — Anthropic-compatible endpoint for coding tools
    // Source: https://docs.bigmodel.cn/cn/coding-plan/overview
    if let Some(api_key) = read_env_first(&["GLM_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
            ProviderProtocol::Anthropic,
        );
        providers.push(ProviderConfig {
            name: "glm-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: Some("glm-5.1".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // MiniMax Token Plan — Anthropic-compatible endpoint
    // Current endpoint/provider shape used by the hosted Token Plan product.
    let mut lookup = |keys: &[&str]| read_env_first(keys);
    if let Some(provider) = discover_minimax_token_plan_provider(&mut lookup) {
        providers.push(provider);
    }

    {
        let mut lookup = |keys: &[&str]| read_env_first(keys);
        if let Some(provider) = discover_kuaikat_coding_provider(&mut lookup) {
            providers.push(provider);
        }
    }

    {
        let mut lookup = |keys: &[&str]| read_env_first(keys);
        if let Some(provider) = discover_deepseek_anthropic_provider(&mut lookup) {
            providers.push(provider);
        }
    }

    // MiniMax Token Plan — OpenAI-compatible endpoint
    // Source: https://platform.minimaxi.com/docs/token-plan/intro
    if let Some(api_key) = read_env_first(&["MINIMAX_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            read_env_first(&["MINIMAX_CODING_PLAN_BASE_URL"])
                .or_else(|| Some("https://api.minimax.chat/v1".to_owned())),
            ProviderProtocol::OpenAi,
        );
        providers.push(ProviderConfig {
            name: "minimax-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["MINIMAX_CODING_PLAN_MODEL"])
                .or(Some("MiniMax-M2.7".to_owned())),
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Aliyun Bailian (阿里云百炼) Coding Plan — Anthropic-compatible endpoint
    // Source: https://help.aliyun.com/zh/model-studio/coding-plan
    // Supports: qwen3.6-plus, kimi-k2.5, glm-5, MiniMax-M2.5, qwen3.5-plus, etc.
    if let Some(api_key) = read_env_first(&["ALIYUN_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://coding.dashscope.aliyuncs.com/apps/anthropic".to_owned()),
            ProviderProtocol::Anthropic,
        );
        providers.push(ProviderConfig {
            name: "aliyun-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["ALIYUN_CODING_MODEL"]).or(Some("qwen3.6-plus".to_owned())),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Tencent Cloud Coding Plan — Anthropic-compatible endpoint
    // Source: https://cloud.tencent.com/document/product/1823/130092
    // Supports: tc-code-latest (Auto), hunyuan-2.0-instruct, hunyuan-2.0-thinking,
    //           minimax-m2.5, kimi-k2.5, glm-5
    if let Some(api_key) = read_env_first(&["TENCENT_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://api.lkeap.cloud.tencent.com/coding/anthropic".to_owned()),
            ProviderProtocol::Anthropic,
        );
        providers.push(ProviderConfig {
            name: "tencent-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["TENCENT_CODING_MODEL"]).or(Some("tc-code-latest".to_owned())),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Baidu Qianfan Coding Plan — Anthropic-compatible endpoint
    // Source: https://cloud.baidu.com/doc/qianfan/s/imlg0beiu
    // Supports: kimi-k2.5, deepseek-v3.2, glm-5, minimax-m2.5, ernie-4.5-turbo
    if let Some(api_key) = read_env_first(&["QIANFAN_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://qianfan.baidubce.com/anthropic/coding".to_owned()),
            ProviderProtocol::Anthropic,
        );
        providers.push(ProviderConfig {
            name: "qianfan-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["QIANFAN_CODING_MODEL"])
                .or(Some("qianfan-code-latest".to_owned())),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Kimi / Moonshot Coding Plan — OpenAI-compatible endpoint
    // Source: Kimi Code Plan documentation
    if let Some(api_key) = read_env_first(&["KIMI_CODING_PLAN_API_KEY"]) {
        let base_url = normalize_base_url(
            Some("https://api.moonshot.cn/kimi-component/ai_coding".to_owned()),
            ProviderProtocol::OpenAi,
        );
        providers.push(ProviderConfig {
            name: "kimi-coding".to_owned(),
            base_url,
            api_key: Some(api_key),
            model: read_env_first(&["KIMI_CODING_MODEL"]).or(Some("kimi-k2.5".to_owned())),
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    // Volcano Engine (字节跳动) Coding Plan
    // Source: https://www.volcengine.com/docs/82379/1925114
    if let Some(api_key) = read_env_first(&["VOLCENGINE_CODING_PLAN_API_KEY"]) {
        let base_url = read_env_first(&["VOLCENGINE_CODING_BASE_URL"])
            .or_else(|| Some("https://ark.cn-beijing.volces.com/api/v3".to_owned()));
        providers.push(ProviderConfig {
            name: "volcengine-coding".to_owned(),
            base_url: normalize_base_url(base_url, ProviderProtocol::OpenAi),
            api_key: Some(api_key),
            model: read_env_first(&["VOLCENGINE_CODING_MODEL"])
                .or(Some("doubao-seed-1-5".to_owned())),
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });
    }

    providers
}

fn discover_minimax_token_plan_provider<F>(lookup: &mut F) -> Option<ProviderConfig>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    let api_key = lookup(&["MINIMAX_TOKEN_PLAN_API_KEY", "MINIMAX_API_KEY"])?;
    let base_url = lookup(&["MINIMAX_TOKEN_PLAN_BASE_URL", "MINIMAX_ANTHROPIC_BASE_URL"])
        .or_else(|| {
            lookup(&["MINIMAX_API_HOST"]).map(|host| {
                let trimmed = host.trim().trim_end_matches('/');
                if trimmed.to_ascii_lowercase().contains("/anthropic") {
                    trimmed.to_owned()
                } else {
                    format!("{trimmed}/anthropic")
                }
            })
        })
        .or_else(|| Some("https://api.minimaxi.com/anthropic".to_owned()));
    let base_url = normalize_base_url(base_url, ProviderProtocol::Anthropic);
    Some(ProviderConfig {
        name: "minimax-token-plan".to_owned(),
        base_url,
        api_key: Some(api_key),
        model: lookup(&["MINIMAX_TOKEN_PLAN_MODEL"]).or(Some("minimax-m2.7".to_owned())),
        protocol: ProviderProtocol::Anthropic,
        timeout_ms: 600_000,
        max_output_tokens: 8_192,
        max_retries: default_provider_max_retries(),
        retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
        retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
        respect_retry_after: default_provider_respect_retry_after(),
        request_header_overrides: BTreeMap::new(),
        request_metadata: BTreeMap::new(),
        thinking_budget: None,
        temperature: None,
        top_p: None,
        top_k: None,
    })
}

fn discover_kuaikat_coding_provider<F>(lookup: &mut F) -> Option<ProviderConfig>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    let api_key = lookup(&["KUAIKAT_CODING_PLAN_API_KEY", "KUAIKAT_API_KEY"])?;
    let base_url = normalize_base_url(
        lookup(&["KUAIKAT_CODING_PLAN_BASE_URL", "KUAIKAT_ANTHROPIC_BASE_URL"])
            .or_else(|| {
                Some(
                    "https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy"
                        .to_owned(),
                )
            }),
        ProviderProtocol::Anthropic,
    );
    Some(ProviderConfig {
        name: "kuaikat-coding".to_owned(),
        base_url,
        api_key: Some(api_key),
        model: lookup(&["KUAIKAT_CODING_PLAN_MODEL", "KUAIKAT_MODEL"])
            .or(Some("kat-coder-pro-v2".to_owned())),
        protocol: ProviderProtocol::Anthropic,
        timeout_ms: 600_000,
        max_output_tokens: 8_192,
        max_retries: default_provider_max_retries(),
        retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
        retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
        respect_retry_after: default_provider_respect_retry_after(),
        request_header_overrides: BTreeMap::new(),
        request_metadata: BTreeMap::new(),
        thinking_budget: None,
        temperature: None,
        top_p: None,
        top_k: None,
    })
}

fn discover_deepseek_anthropic_provider<F>(lookup: &mut F) -> Option<ProviderConfig>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    let api_key = lookup(&["DEEPSEEK_API_KEY", "DEEPSEEK_CODING_PLAN_API_KEY"])?;
    let base_url = normalize_base_url(
        lookup(&["DEEPSEEK_ANTHROPIC_BASE_URL"])
            .or_else(|| Some("https://api.deepseek.com/anthropic".to_owned())),
        ProviderProtocol::Anthropic,
    );
    Some(ProviderConfig {
        name: "deepseek-anthropic".to_owned(),
        base_url,
        api_key: Some(api_key),
        model: lookup(&["DEEPSEEK_MODEL", "DEEPSEEK_CODING_PLAN_MODEL"])
            .or(Some("deepseek-v4-flash".to_owned())),
        protocol: ProviderProtocol::Anthropic,
        timeout_ms: 600_000,
        max_output_tokens: 8_192,
        max_retries: default_provider_max_retries(),
        retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
        retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
        respect_retry_after: default_provider_respect_retry_after(),
        request_header_overrides: BTreeMap::new(),
        request_metadata: BTreeMap::new(),
        thinking_budget: None,
        temperature: None,
        top_p: None,
        top_k: None,
    })
}

fn discover_anthropic_provider<F>(lookup: &mut F) -> Option<ProviderConfig>
where
    F: FnMut(&[&str]) -> Option<String>,
{
    let api_key = lookup(&["REMOTE_CODE_API_KEY", "ANTHROPIC_API_KEY"])?;
    let base_url = normalize_base_url(
        lookup(&["REMOTE_CODE_ANTHROPIC_BASE_URL", "ANTHROPIC_BASE_URL"])
            .or_else(|| Some("https://api.anthropic.com".to_owned())),
        ProviderProtocol::Anthropic,
    );
    Some(ProviderConfig {
        name: "anthropic".to_owned(),
        base_url,
        api_key: Some(api_key),
        model: lookup(&[
            "REMOTE_CODE_ANTHROPIC_MODEL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
        ])
        .or(Some("claude-sonnet-4-6".to_owned())),
        protocol: ProviderProtocol::Anthropic,
        timeout_ms: 600_000,
        max_output_tokens: 4_096,
        max_retries: default_provider_max_retries(),
        retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
        retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
        respect_retry_after: default_provider_respect_retry_after(),
        request_header_overrides: BTreeMap::new(),
        request_metadata: BTreeMap::new(),
        thinking_budget: None,
        temperature: None,
        top_p: None,
        top_k: None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyImportSummary {
    pub source_dir: PathBuf,
    pub destination_dir: PathBuf,
    pub copied_files: usize,
    pub skipped_files: usize,
    pub imported_paths: Vec<PathBuf>,
}

/// # Errors
/// Returns an error if the legacy profile cannot be imported.
pub fn import_legacy_profile(
    source_dir: Option<PathBuf>,
    destination: &AppPaths,
) -> Result<LegacyImportSummary> {
    let source_dir = match source_dir {
        Some(path) => path,
        None => AppPaths::legacy_profile_dir()?,
    };
    let destination_dir = destination.profiles_dir.join("legacy-import");
    fs::create_dir_all(&destination_dir)
        .with_context(|| format!("failed to create {}", destination_dir.display()))?;

    let mut copied_files = 0usize;
    let mut skipped_files = 0usize;
    let mut imported_paths = Vec::new();
    for relative in [
        Path::new("feature-flags.json"),
        Path::new("settings.json"),
        Path::new("history.json"),
        Path::new("history.ndjson"),
        Path::new("sessions"),
        Path::new("skills"),
        Path::new("plugins"),
    ] {
        let source_path = source_dir.join(relative);
        if !source_path.exists() {
            continue;
        }
        let target_path = destination_dir.join(relative);
        if source_path.is_dir() {
            copy_directory(
                &source_path,
                &target_path,
                &mut copied_files,
                &mut skipped_files,
            )?;
            imported_paths.push(target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if target_path.exists() {
                skipped_files += 1;
            } else {
                fs::copy(&source_path, &target_path)?;
                copied_files += 1;
                imported_paths.push(target_path);
            }
        }
    }

    Ok(LegacyImportSummary {
        source_dir,
        destination_dir,
        copied_files,
        skipped_files,
        imported_paths,
    })
}

/// # Errors
/// Returns an error if the hooks file cannot be read or parsed.
pub fn load_hooks_file(path: impl AsRef<Path>) -> Result<BTreeMap<HookEvent, Vec<HookMatcher>>> {
    let path = path.as_ref();
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse hooks file {}", path.display()))
}

/// # Errors
/// Returns an error if the settings file cannot be read or parsed.
pub fn load_settings_hooks(
    path: impl AsRef<Path>,
) -> Result<BTreeMap<HookEvent, Vec<HookMatcher>>> {
    let path = path.as_ref();
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse settings file {}", path.display()))?;
    let Some(hooks) = value.get("hooks") else {
        return Ok(BTreeMap::new());
    };
    serde_json::from_value(hooks.clone())
        .with_context(|| format!("failed to decode hooks from settings {}", path.display()))
}

fn copy_directory(
    source: &Path,
    destination: &Path,
    copied_files: &mut usize,
    skipped_files: &mut usize,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        let Ok(relative) = entry.path().strip_prefix(source) else {
            continue;
        };
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if target.exists() {
            *skipped_files += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), &target)?;
        *copied_files += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::fs;

    use super::{
        ProviderConfig, default_provider_max_retries, default_provider_respect_retry_after,
        default_provider_retry_initial_backoff_ms, default_provider_retry_max_backoff_ms,
        discover_anthropic_provider, discover_deepseek_anthropic_provider,
        discover_kuaikat_coding_provider, discover_minimax_token_plan_provider,
        hydrate_provider_from_discovered, load_hooks_file, load_runtime_config,
        load_settings_hooks, normalize_base_url, normalize_protocol,
        resolve_auth_source_with_lookup, validate_provider_config,
    };
    use crate::ProviderOverrides;
    use crate::settings_layers::{ResolvedRuntimeSettings, RuntimeOverrides, SettingSource};
    use claude_core::{HookEvent, InputFormat, OutputFormat, PermissionMode, ProviderProtocol};
    use tempfile::tempdir;

    #[test]
    fn anthropic_base_url_is_normalized() {
        let normalized = normalize_base_url(
            Some("https://example.com/anthropic".to_owned()),
            ProviderProtocol::Anthropic,
        );
        assert_eq!(
            normalized.as_deref(),
            Some("https://example.com/anthropic/v1/messages")
        );
    }

    #[test]
    fn openai_base_url_is_normalized() {
        let normalized = normalize_base_url(
            Some("https://example.com/v1".to_owned()),
            ProviderProtocol::OpenAi,
        );
        assert_eq!(
            normalized.as_deref(),
            Some("https://example.com/v1/chat/completions")
        );
    }

    #[test]
    fn protocol_is_detected_from_base_url() {
        let protocol = normalize_protocol(Some("https://example.com/anthropic"), None);
        assert_eq!(protocol, ProviderProtocol::Anthropic);
    }

    #[test]
    fn loads_hooks_file_using_upstream_shape() {
        let temp = tempdir().expect("tempdir should work");
        let path = temp.path().join("hooks.json");
        fs::write(
            &path,
            r#"{
                "SessionStart": [
                    {
                        "matcher": "startup",
                        "hooks": [{"type": "command", "command": "echo session"}]
                    }
                ]
            }"#,
        )
        .expect("write should work");

        let hooks = load_hooks_file(&path).expect("hooks file should load");
        assert_eq!(hooks.len(), 1);
        assert_eq!(
            hooks
                .get(&HookEvent::SessionStart)
                .expect("session start hook should exist")
                .len(),
            1
        );
    }

    #[test]
    fn loads_hooks_from_settings_file() {
        let temp = tempdir().expect("tempdir should work");
        let path = temp.path().join("settings.json");
        fs::write(
            &path,
            r#"{
                "model": "test-model",
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{"type": "command", "command": "echo before"}]
                        }
                    ]
                }
            }"#,
        )
        .expect("write should work");

        let hooks = load_settings_hooks(&path).expect("settings hooks should load");
        assert_eq!(hooks.len(), 1);
        assert_eq!(
            hooks
                .get(&HookEvent::PreToolUse)
                .expect("pre tool use hook should exist")[0]
                .matcher
                .as_deref(),
            Some("Bash")
        );
    }

    #[test]
    fn settings_without_hooks_returns_empty_map() {
        let temp = tempdir().expect("tempdir should work");
        let path = temp.path().join("settings.json");
        fs::write(&path, r#"{"model": "test-model"}"#).expect("write should work");

        let hooks = load_settings_hooks(&path).expect("settings hooks should load");
        assert_eq!(hooks, BTreeMap::new());
    }

    #[test]
    fn load_runtime_config_auto_discovers_settings_when_cli_empty() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::create_dir_all(profile_dir.join("profiles").join("legacy-import"))
            .expect("legacy settings dir");

        let legacy = profile_dir
            .join("profiles")
            .join("legacy-import")
            .join("settings.json");
        let profile = profile_dir.join("settings.json");
        let project = cwd.join(".remote-code").join("settings.json");
        let local = cwd.join(".remote-code").join("settings.local.json");
        fs::write(&legacy, r#"{"session_name":"legacy"}"#).expect("write legacy");
        fs::write(&profile, r#"{"session_name":"profile"}"#).expect("write profile");
        fs::write(&project, r#"{"session_name":"project"}"#).expect("write project");
        fs::write(
            &local,
            r#"{
                "session_name":"local",
                "provider":{"api_key":"local-secret"}
            }"#,
        )
        .expect("write local");

        let config = load_runtime_config(
            Some(cwd.clone()),
            Some(profile_dir.clone()),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                ..ProviderOverrides::default()
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");

        assert_eq!(config.session_name.as_deref(), Some("local"));
        assert_eq!(config.provider.api_key.as_deref(), Some("local-secret"));
        assert_eq!(
            config.auth_source.as_deref(),
            Some(format!("settings:{}", local.display()).as_str())
        );
        assert_eq!(
            config.settings_files,
            vec![
                legacy.clone(),
                profile.clone(),
                project.clone(),
                local.clone()
            ]
        );
        assert!(config.cli_settings_files.is_empty());
        assert_eq!(
            config
                .setting_sources
                .iter()
                .filter(|source| source.starts_with("settings:"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                format!("settings:{}", legacy.display()),
                format!("settings:{}", profile.display()),
                format!("settings:{}", project.display()),
                format!("settings:{}", local.display()),
            ]
        );
    }

    #[test]
    fn load_runtime_config_explicit_settings_disable_autodiscovery() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::create_dir_all(&profile_dir).expect("profile dir");

        let auto = cwd.join(".remote-code").join("settings.json");
        let explicit = temp.path().join("explicit.json");
        fs::write(&auto, r#"{"session_name":"auto"}"#).expect("write auto");
        fs::write(&explicit, r#"{"session_name":"explicit"}"#).expect("write explicit");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                ..ProviderOverrides::default()
            },
            RuntimeOverrides {
                settings_files: vec![explicit.clone()],
                ..RuntimeOverrides::default()
            },
        )
        .expect("config should load");

        assert_eq!(config.session_name.as_deref(), Some("explicit"));
        assert_eq!(config.settings_files, vec![explicit.clone()]);
        assert_eq!(config.cli_settings_files, vec![explicit.clone()]);
        assert_eq!(
            config
                .setting_sources
                .iter()
                .filter(|source| source.starts_with("settings:"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![format!("settings:{}", explicit.display())]
        );
    }

    #[test]
    fn load_runtime_config_with_no_discovered_settings_keeps_file_list_empty() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(&cwd).expect("workspace dir");

        struct EnvSnapshot(Vec<(&'static str, Option<String>)>);

        impl EnvSnapshot {
            fn clear(names: impl IntoIterator<Item = &'static str>) -> Self {
                let values = names
                    .into_iter()
                    .map(|name| {
                        let value = std::env::var(name).ok();
                        unsafe {
                            std::env::remove_var(name);
                        }
                        (name, value)
                    })
                    .collect();
                Self(values)
            }
        }

        impl Drop for EnvSnapshot {
            fn drop(&mut self) {
                for (name, value) in self.0.drain(..) {
                    match value {
                        Some(value) => unsafe {
                            std::env::set_var(name, value);
                        },
                        None => unsafe {
                            std::env::remove_var(name);
                        },
                    }
                }
            }
        }

        let _env_snapshot = EnvSnapshot::clear(
            super::env_setting_source_keys()
                .iter()
                .map(|(name, _)| *name),
        );

        let config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                ..ProviderOverrides::default()
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");

        assert!(config.settings_files.is_empty());
        assert!(config.cli_settings_files.is_empty());
        assert_eq!(config.setting_sources, vec!["cli:provider".to_owned()]);

        drop(_env_snapshot);
    }

    #[test]
    fn load_runtime_config_preserves_api_key_helper_and_auth_source() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(&cwd).expect("workspace dir");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        let settings = profile_dir.join("settings.json");
        fs::write(
            &settings,
            r#"{
                "apiKeyHelper": "echo helper-key",
                "provider": {
                    "name": "anthropic",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-sonnet-4-5"
                }
            }"#,
        )
        .expect("write settings");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config should load");

        assert_eq!(config.api_key_helper.as_deref(), Some("echo helper-key"));
        assert_eq!(config.provider.api_key, None);
        assert_eq!(config.auth_source.as_deref(), Some("apiKeyHelper"));
    }

    #[test]
    fn load_runtime_config_can_limit_discovery_to_local_sources() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::create_dir_all(profile_dir.join("profiles").join("legacy-import"))
            .expect("legacy settings dir");

        let legacy = profile_dir
            .join("profiles")
            .join("legacy-import")
            .join("settings.json");
        let profile = profile_dir.join("settings.json");
        let project = cwd.join(".remote-code").join("settings.json");
        let local = cwd.join(".remote-code").join("settings.local.json");
        fs::write(&legacy, r#"{"session_name":"legacy"}"#).expect("write legacy");
        fs::write(&profile, r#"{"session_name":"profile"}"#).expect("write profile");
        fs::write(&project, r#"{"session_name":"project"}"#).expect("write project");
        fs::write(&local, r#"{"session_name":"local"}"#).expect("write local");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                ..ProviderOverrides::default()
            },
            RuntimeOverrides {
                allowed_setting_sources: Some(vec![SettingSource::Local]),
                ..RuntimeOverrides::default()
            },
        )
        .expect("config should load");

        assert_eq!(config.allowed_setting_sources, vec![SettingSource::Local]);
        assert_eq!(config.settings_files, vec![local.clone()]);
        assert!(config.cli_settings_files.is_empty());
        assert_eq!(config.session_name.as_deref(), Some("local"));
        assert_eq!(
            config
                .setting_sources
                .iter()
                .filter(|source| source.starts_with("settings:"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![format!("settings:{}", local.display())]
        );
    }

    #[test]
    fn load_runtime_config_prefers_ancestor_project_profile_dir_when_present() {
        let temp = tempdir().expect("tempdir should work");
        let project_root = temp.path().join("workspace");
        let cwd = project_root.join("tasks").join("nested");
        let profile_dir = project_root.join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile_dir).expect("profile");
        fs::write(
            profile_dir.join("settings.json"),
            r#"{"session_name":"workspace-profile"}"#,
        )
        .expect("write profile settings");

        let config = load_runtime_config(
            Some(cwd),
            None,
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                ..ProviderOverrides::default()
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");

        assert_eq!(config.paths.profile_dir, profile_dir);
        assert_eq!(config.session_name.as_deref(), Some("workspace-profile"));
    }

    #[test]
    fn resolve_auth_source_prefers_settings_when_provider_env_was_not_used() {
        let provider = ProviderConfig {
            name: "glm-coding".to_owned(),
            base_url: Some("https://open.bigmodel.cn/api/anthropic/v1/messages".to_owned()),
            api_key: Some("settings-secret".to_owned()),
            model: Some("glm-5.1".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let settings = ResolvedRuntimeSettings {
            api_key: Some("settings-secret".to_owned()),
            auth_source: Some("settings:/tmp/profile/settings.json".to_owned()),
            ..ResolvedRuntimeSettings::default()
        };
        let env_values = HashMap::from([("GLM_CODING_PLAN_API_KEY", "env-secret".to_owned())]);

        let auth_source = resolve_auth_source_with_lookup(
            &ProviderOverrides::default(),
            &settings,
            &provider,
            &mut |keys| keys.iter().find_map(|key| env_values.get(*key).cloned()),
        );

        assert_eq!(
            auth_source.as_deref(),
            Some("settings:/tmp/profile/settings.json")
        );
    }

    #[test]
    fn resolve_auth_source_uses_anthropic_env_for_custom_anthropic_provider() {
        let provider = ProviderConfig {
            name: "custom".to_owned(),
            base_url: Some("https://example.com/anthropic/v1/messages".to_owned()),
            api_key: Some("env-secret".to_owned()),
            model: Some("custom-model".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let settings = ResolvedRuntimeSettings::default();
        let env_values = HashMap::from([("ANTHROPIC_API_KEY", "env-secret".to_owned())]);

        let auth_source = resolve_auth_source_with_lookup(
            &ProviderOverrides::default(),
            &settings,
            &provider,
            &mut |keys| keys.iter().find_map(|key| env_values.get(*key).cloned()),
        );

        assert_eq!(auth_source.as_deref(), Some("env:ANTHROPIC_API_KEY"));
    }

    #[test]
    fn resolve_auth_source_prefers_existing_key_over_api_key_helper() {
        let provider = ProviderConfig {
            name: "anthropic".to_owned(),
            base_url: Some("https://api.anthropic.com/v1/messages".to_owned()),
            api_key: Some("env-secret".to_owned()),
            model: Some("claude-sonnet-4-5".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let settings = ResolvedRuntimeSettings {
            api_key_helper: Some("echo helper".to_owned()),
            ..ResolvedRuntimeSettings::default()
        };
        let env_values = HashMap::from([("ANTHROPIC_API_KEY", "env-secret".to_owned())]);

        let auth_source = resolve_auth_source_with_lookup(
            &ProviderOverrides::default(),
            &settings,
            &provider,
            &mut |keys| keys.iter().find_map(|key| env_values.get(*key).cloned()),
        );

        assert_eq!(auth_source.as_deref(), Some("env:ANTHROPIC_API_KEY"));
    }

    #[test]
    fn anthropic_env_provider_uses_reference_env_aliases_and_defaults() {
        let values = HashMap::from([
            ("ANTHROPIC_API_KEY", "env-secret".to_owned()),
            ("ANTHROPIC_DEFAULT_MODEL", "claude-sonnet-4-6".to_owned()),
        ]);
        let provider = discover_anthropic_provider(&mut |keys| {
            keys.iter().find_map(|key| values.get(*key).cloned())
        })
        .expect("anthropic provider should be discovered");

        assert_eq!(provider.name, "anthropic");
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.anthropic.com/v1/messages")
        );
        assert_eq!(provider.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn minimax_token_plan_env_provider_is_discovered_as_anthropic() {
        let values = HashMap::from([
            ("MINIMAX_API_KEY", "secret".to_owned()),
            ("MINIMAX_API_HOST", "https://api.minimaxi.com".to_owned()),
            ("MINIMAX_TOKEN_PLAN_MODEL", "minimax-m2.7".to_owned()),
        ]);
        let provider = discover_minimax_token_plan_provider(&mut |keys| {
            keys.iter().find_map(|key| values.get(*key).cloned())
        })
        .expect("minimax token plan provider should be discovered");
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.minimaxi.com/anthropic/v1/messages")
        );
        assert_eq!(provider.model.as_deref(), Some("minimax-m2.7"));
    }

    #[test]
    fn minimax_token_plan_explicit_anthropic_base_url_is_preserved() {
        let values = HashMap::from([
            ("MINIMAX_TOKEN_PLAN_API_KEY", "secret".to_owned()),
            (
                "MINIMAX_ANTHROPIC_BASE_URL",
                "https://api.minimaxi.com/anthropic".to_owned(),
            ),
        ]);
        let provider = discover_minimax_token_plan_provider(&mut |keys| {
            keys.iter().find_map(|key| values.get(*key).cloned())
        })
        .expect("minimax token plan provider should be discovered");

        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.minimaxi.com/anthropic/v1/messages")
        );
    }

    #[test]
    fn kuaikat_coding_env_provider_is_discovered_as_anthropic() {
        let values = HashMap::from([("KUAIKAT_CODING_PLAN_API_KEY", "secret".to_owned())]);
        let provider = discover_kuaikat_coding_provider(&mut |keys| {
            keys.iter().find_map(|key| values.get(*key).cloned())
        })
        .expect("kuaikat coding provider should be discovered");

        assert_eq!(provider.name, "kuaikat-coding");
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
        assert_eq!(
            provider.base_url.as_deref(),
            Some(
                "https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy/v1/messages"
            )
        );
        assert_eq!(provider.model.as_deref(), Some("kat-coder-pro-v2"));
    }

    #[test]
    fn deepseek_anthropic_env_provider_is_discovered() {
        let values = HashMap::from([("DEEPSEEK_API_KEY", "secret".to_owned())]);
        let provider = discover_deepseek_anthropic_provider(&mut |keys| {
            keys.iter().find_map(|key| values.get(*key).cloned())
        })
        .expect("deepseek anthropic provider should be discovered");

        assert_eq!(provider.name, "deepseek-anthropic");
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic/v1/messages")
        );
        assert_eq!(provider.model.as_deref(), Some("deepseek-v4-flash"));
    }

    #[test]
    fn discovered_provider_hydrates_named_runtime_provider() {
        let mut provider = ProviderConfig {
            name: "glm-coding".to_owned(),
            base_url: None,
            api_key: None,
            model: None,
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let discovered = vec![ProviderConfig {
            name: "glm-coding".to_owned(),
            base_url: Some("https://open.bigmodel.cn/api/anthropic/v1/messages".to_owned()),
            api_key: Some("secret".to_owned()),
            model: Some("glm-5.1".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }];

        hydrate_provider_from_discovered(&mut provider, &discovered, false);

        assert_eq!(provider.name, "glm-coding");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://open.bigmodel.cn/api/anthropic/v1/messages")
        );
        assert_eq!(provider.api_key.as_deref(), Some("secret"));
        assert_eq!(provider.model.as_deref(), Some("glm-5.1"));
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
    }

    #[test]
    fn discovered_provider_can_replace_empty_custom_provider_when_only_one_exists() {
        let mut provider = ProviderConfig {
            name: "custom".to_owned(),
            base_url: None,
            api_key: None,
            model: None,
            protocol: ProviderProtocol::OpenAi,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let discovered = vec![ProviderConfig {
            name: "minimax-token-plan".to_owned(),
            base_url: Some("https://api.minimaxi.com/anthropic/v1/messages".to_owned()),
            api_key: Some("secret".to_owned()),
            model: Some("minimax-m2.7".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }];

        hydrate_provider_from_discovered(&mut provider, &discovered, false);

        assert_eq!(provider.name, "minimax-token-plan");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.minimaxi.com/anthropic/v1/messages")
        );
        assert_eq!(provider.api_key.as_deref(), Some("secret"));
        assert_eq!(provider.model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
    }

    #[test]
    fn discovered_provider_can_hydrate_custom_provider_from_matching_endpoint() {
        let mut provider = ProviderConfig {
            name: "custom".to_owned(),
            base_url: Some("https://api.minimaxi.com/anthropic/v1/messages".to_owned()),
            api_key: None,
            model: Some("minimax-m2.7".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        };
        let discovered = vec![ProviderConfig {
            name: "minimax-token-plan".to_owned(),
            base_url: Some("https://api.minimaxi.com/anthropic/v1/messages".to_owned()),
            api_key: Some("secret".to_owned()),
            model: Some("minimax-m2.7".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 8_192,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }];

        hydrate_provider_from_discovered(&mut provider, &discovered, true);

        assert_eq!(provider.name, "minimax-token-plan");
        assert_eq!(provider.api_key.as_deref(), Some("secret"));
        assert_eq!(provider.model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(provider.protocol, ProviderProtocol::Anthropic);
    }

    #[test]
    fn validate_provider_config_reports_provider_aware_auth_hints() {
        let report = validate_provider_config(&ProviderConfig {
            name: "custom".to_owned(),
            base_url: Some("https://example.com/anthropic/v1/messages".to_owned()),
            api_key: None,
            model: Some("custom-model".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 600_000,
            max_output_tokens: 4_096,
            max_retries: default_provider_max_retries(),
            retry_initial_backoff_ms: default_provider_retry_initial_backoff_ms(),
            retry_max_backoff_ms: default_provider_retry_max_backoff_ms(),
            respect_retry_after: default_provider_respect_retry_after(),
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        });

        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("ANTHROPIC_API_KEY") && issue.contains("--api-key"))
        );
    }
}
