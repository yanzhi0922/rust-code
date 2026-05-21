use std::{
    collections::{BTreeSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use clap::{Args, Subcommand, ValueEnum};
use claude_config::{RuntimeConfig, SettingSource};
use claude_core::{ConversationEntry, ToolCall, ToolResult};
use claude_permissions::{
    PermissionBroker, PermissionDecision, PermissionRequest, PermissionUpdate,
};
use claude_session::SessionStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::Mutex,
    time::{Duration, timeout},
};
use uuid::Uuid;

use crate::session_file_access::handle_session_file_access_post_tool;

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

#[derive(Subcommand, Debug)]
pub enum HooksCommand {
    List(HooksListArgs),
}

#[derive(Args, Debug)]
pub struct HooksListArgs {
    #[arg(long)]
    pub json: bool,

    #[arg(long, value_enum)]
    pub event: Option<HookEventName>,

    #[arg(long)]
    pub include_unsupported: bool,

    #[arg(long = "source")]
    pub sources: Vec<String>,

    #[arg(long = "plugins-dir")]
    pub plugin_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum HookEventName {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    PermissionDenied,
}

impl HookEventName {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::UserPromptSubmit => "user_prompt_submit",
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::PostToolUseFailure => "post_tool_use_failure",
            Self::PermissionRequest => "permission_request",
            Self::PermissionDenied => "permission_denied",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionDenied => "PermissionDenied",
        }
    }

    fn parse_key(raw: &str) -> Option<Self> {
        let normalized = raw
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match normalized.as_str() {
            "sessionstart" => Some(Self::SessionStart),
            "userpromptsubmit" => Some(Self::UserPromptSubmit),
            "pretooluse" => Some(Self::PreToolUse),
            "posttooluse" => Some(Self::PostToolUse),
            "posttoolusefailure" => Some(Self::PostToolUseFailure),
            "permissionrequest" => Some(Self::PermissionRequest),
            "permissiondenied" => Some(Self::PermissionDenied),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HookRecord {
    pub hook_id: String,
    pub event: HookEventName,
    pub hook_type: String,
    pub display: String,
    pub matcher: Option<String>,
    pub source_kind: String,
    pub source_name: String,
    pub source_path: PathBuf,
    pub plugin_name: Option<String>,
    pub shell: Option<String>,
    pub timeout_secs: u64,
    pub once: bool,
    pub supported: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeHookDiscovery {
    pub hooks: Vec<HookRecord>,
    pub warnings: Vec<String>,
}

impl RuntimeHookDiscovery {}

#[derive(Debug, Clone, Serialize)]
pub struct HooksListOutput {
    pub warnings: Vec<String>,
    pub hooks: Vec<HookRecord>,
}

#[derive(Debug, Default)]
pub struct HookRunState {
    consumed_once_hooks: BTreeSet<String>,
    session_start_completed: bool,
}

impl HookRunState {
    pub fn load(store: &SessionStore, session_id: Uuid) -> Result<Self> {
        let transcript = store.load_transcript(session_id)?;
        Ok(Self {
            consumed_once_hooks: transcript.consumed_once_hook_ids(),
            session_start_completed: transcript.has_hook_phase("session_start"),
        })
    }

    fn should_skip_once(&self, hook: &HookRecord) -> bool {
        hook.once && self.consumed_once_hooks.contains(&hook.hook_id)
    }

    fn mark_executed(&mut self, hook: &HookRecord) {
        if hook.once {
            self.consumed_once_hooks.insert(hook.hook_id.clone());
        }
    }

    #[must_use]
    pub fn session_start_completed(&self) -> bool {
        self.session_start_completed
    }

    pub fn mark_session_start_completed(&mut self) {
        self.session_start_completed = true;
    }
}

#[derive(Debug, Clone)]
pub struct PreparedToolCall {
    pub call: ToolCall,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookShell {
    Sh,
    Bash,
    PowerShell,
}

impl HookShell {
    fn parse(raw: Option<&str>) -> Option<Self> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Some(Self::default_for_host());
        };
        match raw.to_ascii_lowercase().as_str() {
            "sh" => Some(Self::Sh),
            "bash" => Some(Self::Bash),
            "powershell" | "powershell.exe" | "pwsh" => Some(Self::PowerShell),
            _ => None,
        }
    }

    fn default_for_host() -> Self {
        if cfg!(windows) {
            Self::PowerShell
        } else {
            Self::Sh
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sh => "sh",
            Self::Bash => "bash",
            Self::PowerShell => "powershell",
        }
    }

    fn program_and_args(self, command: &str) -> (&'static str, Vec<String>) {
        match self {
            Self::Sh => ("sh", vec!["-lc".to_owned(), command.to_owned()]),
            Self::Bash => ("bash", vec!["-lc".to_owned(), command.to_owned()]),
            Self::PowerShell => {
                let program = if cfg!(windows) {
                    "powershell.exe"
                } else {
                    "pwsh"
                };
                (
                    program,
                    vec![
                        "-NoLogo".to_owned(),
                        "-NoProfile".to_owned(),
                        "-Command".to_owned(),
                        command.to_owned(),
                    ],
                )
            }
        }
    }
}

#[derive(Debug)]
struct HookSourceDescriptor {
    source_kind: String,
    source_name: String,
    source_path: PathBuf,
    plugin_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookResponse {
    #[serde(default)]
    r#continue: Option<bool>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    additional_context: Option<String>,
    #[serde(default)]
    updated_input: Option<Value>,
    #[serde(default)]
    hook_specific_output: Option<HookSpecificOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookSpecificOutput {
    #[serde(default)]
    additional_context: Option<String>,
    #[serde(default)]
    updated_input: Option<Value>,
    #[serde(default)]
    permission_decision: Option<PermissionHookBehavior>,
    #[serde(default)]
    permission_decision_reason: Option<String>,
    #[serde(default)]
    decision: Option<PermissionHookDecision>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionHookDecision {
    behavior: PermissionHookBehavior,
    #[serde(default)]
    updated_input: Option<Value>,
    #[serde(default)]
    updated_permissions: Vec<PermissionUpdate>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PermissionHookBehavior {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug)]
struct ExecutedHookOutcome {
    status: &'static str,
    blocked_reason: Option<String>,
    updated_input: Option<Value>,
    permission_decision: Option<PermissionHookDecision>,
    additional_context: Option<String>,
    exit_code: Option<i32>,
    duration_ms: u64,
    stdout_preview: String,
    stderr_preview: String,
}

#[derive(Debug, Default)]
struct HookEffects {
    blocked_reason: Option<String>,
    updated_input: Option<Value>,
    permission_decision: Option<PermissionHookDecision>,
    additional_contexts: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HookExecutionOptions {
    pub persist: bool,
}

impl HookExecutionOptions {
    #[must_use]
    pub fn persistent() -> Self {
        Self { persist: true }
    }

    #[must_use]
    pub fn ephemeral() -> Self {
        Self { persist: false }
    }
}

pub async fn run_hooks(config: &RuntimeConfig, command: HooksCommand) -> Result<()> {
    match command {
        HooksCommand::List(args) => run_hooks_list(config, args).await,
    }
}

pub fn discover_runtime_hooks(
    config: &RuntimeConfig,
    plugin_roots: &[PathBuf],
) -> RuntimeHookDiscovery {
    let mut discovery = RuntimeHookDiscovery::default();
    let mut sources = Vec::new();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);
    let project_sources_enabled = setting_source_enabled(config, SettingSource::Project);

    if user_sources_enabled {
        push_source_if_exists(
            &mut sources,
            "profile",
            "profile hooks",
            config.paths.profile_dir.join("hooks.json"),
            None,
        );
    }

    if project_sources_enabled {
        push_source_if_exists(
            &mut sources,
            "project",
            "project hooks",
            config.cwd.join(".remote-code").join("hooks.json"),
            None,
        );
    }

    for path in &config.settings_files {
        push_settings_source(&mut sources, config, path);
    }

    let mut roots = Vec::new();
    if user_sources_enabled {
        roots.push(config.paths.plugins_dir.clone());
    }
    roots.extend(plugin_roots.iter().cloned());
    dedupe_paths(&mut roots);
    let mut seen_plugins = BTreeSet::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        match claude_plugins::discover_plugins(&root) {
            Ok(plugins) => {
                for plugin in plugins {
                    let manifest_key = plugin.manifest_path.display().to_string();
                    if !seen_plugins.insert(manifest_key) {
                        continue;
                    }
                    if let Some(path) = plugin.hooks_config_path() {
                        sources.push(HookSourceDescriptor {
                            source_kind: "plugin".to_owned(),
                            source_name: plugin.manifest.name.clone(),
                            source_path: path,
                            plugin_name: Some(plugin.manifest.name.clone()),
                        });
                    }
                }
            }
            Err(error) => discovery.warnings.push(format!(
                "Failed to discover plugins in {}: {error}",
                root.display()
            )),
        }
    }

    for source in sources {
        match load_hooks_from_source(&source) {
            Ok(mut hooks) => discovery.hooks.append(&mut hooks),
            Err(error) => discovery.warnings.push(format!(
                "Failed to load hooks from {}: {error}",
                source.source_path.display()
            )),
        }
    }
    discovery
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn push_settings_source(
    sources: &mut Vec<HookSourceDescriptor>,
    config: &RuntimeConfig,
    path: &Path,
) {
    let (source_kind, source_name) = classify_settings_source(config, path);
    push_source_if_exists(sources, source_kind, &source_name, path.to_path_buf(), None);
}

fn classify_settings_source(config: &RuntimeConfig, path: &Path) -> (&'static str, String) {
    if config
        .cli_settings_files
        .iter()
        .any(|candidate| candidate == path)
    {
        return ("explicit", path.display().to_string());
    }

    let legacy = config
        .paths
        .profiles_dir
        .join("legacy-import")
        .join("settings.json");
    if path == legacy {
        return ("legacy-import", "legacy import settings".to_owned());
    }

    let profile = config.paths.profile_dir.join("settings.json");
    if path == profile {
        return ("profile", "profile settings".to_owned());
    }

    let project = config.cwd.join(".remote-code").join("settings.json");
    if path == project {
        return ("project", "project settings".to_owned());
    }

    let local = config.cwd.join(".remote-code").join("settings.local.json");
    if path == local {
        return ("local", "local settings".to_owned());
    }

    ("settings", path.display().to_string())
}

pub fn build_hooks_list_output(
    config: &RuntimeConfig,
    args: &HooksListArgs,
) -> Result<HooksListOutput> {
    let discovery = discover_runtime_hooks(config, &args.plugin_roots);
    let source_filters = args
        .sources
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let hooks = discovery
        .hooks
        .into_iter()
        .filter(|hook| args.include_unsupported || hook.supported)
        .filter(|hook| args.event.is_none_or(|event| hook.event == event))
        .filter(|hook| {
            if source_filters.is_empty() {
                return true;
            }
            let source = format!(
                "{}:{}",
                hook.source_kind.to_ascii_lowercase(),
                hook.source_name.to_ascii_lowercase()
            );
            source_filters.iter().any(|filter| {
                hook.source_kind.eq_ignore_ascii_case(filter)
                    || hook.source_name.eq_ignore_ascii_case(filter)
                    || source.contains(filter)
            })
        })
        .collect();
    Ok(HooksListOutput {
        warnings: discovery.warnings,
        hooks,
    })
}

pub async fn ensure_session_start_hooks(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
) -> Result<()> {
    ensure_session_start_hooks_with_options(
        discovery,
        config,
        store,
        conversation,
        state,
        HookExecutionOptions::persistent(),
    )
    .await
}

pub async fn ensure_session_start_hooks_with_options(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
    options: HookExecutionOptions,
) -> Result<()> {
    if state.session_start_completed() {
        return Ok(());
    }
    let input = json!({
        "event": HookEventName::SessionStart.as_str(),
        "session_id": config.session_id,
        "cwd": config.cwd,
        "provider": {
            "name": config.provider.name,
            "model": config.provider.model,
            "protocol": config.provider.protocol.as_str(),
        },
    });
    let effects = run_event_hooks(
        discovery,
        HookEventName::SessionStart,
        config,
        store,
        state,
        config.cwd.display().to_string(),
        &input,
        true,
        options,
    )
    .await?;
    if let Some(reason) = effects.blocked_reason {
        return Err(anyhow!(reason));
    }
    append_contexts(
        store,
        config.session_id,
        conversation,
        HookEventName::SessionStart,
        &effects.additional_contexts,
        options.persist,
    )?;
    state.mark_session_start_completed();
    if options.persist {
        store.append_named_event(
            config.session_id,
            "hook_phase",
            json!({
                "phase": "session_start",
            }),
        )?;
    }
    Ok(())
}

pub async fn apply_pre_tool_use_hooks(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
    tool_call: &ToolCall,
) -> Result<PreparedToolCall> {
    apply_pre_tool_use_hooks_with_options(
        discovery,
        config,
        store,
        conversation,
        state,
        tool_call,
        HookExecutionOptions::persistent(),
    )
    .await
}

pub async fn apply_pre_tool_use_hooks_with_options(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
    tool_call: &ToolCall,
    options: HookExecutionOptions,
) -> Result<PreparedToolCall> {
    let input = json!({
        "event": HookEventName::PreToolUse.as_str(),
        "session_id": config.session_id,
        "cwd": config.cwd,
        "tool_name": tool_call.name,
        "tool_use_id": tool_call.id,
        "tool_input": tool_call.input,
    });
    let effects = run_event_hooks(
        discovery,
        HookEventName::PreToolUse,
        config,
        store,
        state,
        tool_call.name.clone(),
        &input,
        true,
        options,
    )
    .await?;

    let mut call = tool_call.clone();
    if let Some(updated_input) = effects.updated_input {
        let input_changed = updated_input.is_object() && updated_input != call.input;
        if updated_input.is_object() {
            call.input = updated_input;
        }
        if input_changed && effects.additional_contexts.is_empty() {
            append_contexts(
                store,
                config.session_id,
                conversation,
                HookEventName::PreToolUse,
                &[format!(
                    "A hook adjusted the input for `{}` before execution.",
                    call.name
                )],
                options.persist,
            )?;
        }
    }
    append_contexts(
        store,
        config.session_id,
        conversation,
        HookEventName::PreToolUse,
        &effects.additional_contexts,
        options.persist,
    )?;
    let blocked_reason = effects.blocked_reason.or_else(|| {
        effects
            .permission_decision
            .as_ref()
            .and_then(|decision| match decision.behavior {
                PermissionHookBehavior::Deny => Some(
                    decision
                        .message
                        .clone()
                        .unwrap_or_else(|| "Tool use denied by hook.".to_owned()),
                ),
                PermissionHookBehavior::Allow | PermissionHookBehavior::Ask => None,
            })
    });
    Ok(PreparedToolCall {
        call,
        blocked_reason,
    })
}

pub async fn apply_post_tool_hooks(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
    tool_call: &ToolCall,
    tool_result: &ToolResult,
) -> Result<()> {
    apply_post_tool_hooks_with_options(
        discovery,
        config,
        store,
        conversation,
        state,
        tool_call,
        tool_result,
        HookExecutionOptions::persistent(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_post_tool_hooks_with_options(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    state: &mut HookRunState,
    tool_call: &ToolCall,
    tool_result: &ToolResult,
    options: HookExecutionOptions,
) -> Result<()> {
    let event = if tool_result.is_error {
        HookEventName::PostToolUseFailure
    } else {
        HookEventName::PostToolUse
    };
    let input = json!({
        "event": event.as_str(),
        "session_id": config.session_id,
        "cwd": config.cwd,
        "tool_name": tool_call.name,
        "tool_use_id": tool_call.id,
        "tool_input": tool_call.input,
        "tool_result": {
            "content": tool_result.content,
            "is_error": tool_result.is_error,
        },
    });
    let effects = run_event_hooks(
        discovery,
        event,
        config,
        store,
        state,
        tool_call.name.clone(),
        &input,
        false,
        options,
    )
    .await?;
    append_contexts(
        store,
        config.session_id,
        conversation,
        event,
        &effects.additional_contexts,
        options.persist,
    )?;
    if options.persist && !tool_result.is_error {
        handle_session_file_access_post_tool(config, store, tool_call)?;
    }
    Ok(())
}

pub async fn apply_permission_request_hooks(
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    state: &mut HookRunState,
    request: &PermissionRequest,
    options: HookExecutionOptions,
) -> Result<Option<PermissionDecision>> {
    let input = json!({
        "event": HookEventName::PermissionRequest.as_str(),
        "hookEventName": HookEventName::PermissionRequest.display_name(),
        "session_id": config.session_id,
        "cwd": config.cwd,
        "tool_name": request.tool_name,
        "tool_use_id": request.tool_use_id,
        "tool_input": request.tool_input,
        "permission_request": {
            "class": request.resolved_permission_class(),
            "title": request.title,
            "description": request.description,
            "blocked_path": request.blocked_path,
            "permission_suggestions": request.permission_suggestions,
        },
    });
    let effects = run_event_hooks(
        discovery,
        HookEventName::PermissionRequest,
        config,
        store,
        state,
        request.tool_name.clone(),
        &input,
        true,
        options,
    )
    .await?;

    if let Some(reason) = effects.blocked_reason {
        return Ok(Some(PermissionDecision::deny(reason)));
    }
    let Some(decision) = effects.permission_decision else {
        return Ok(None);
    };
    match decision.behavior {
        PermissionHookBehavior::Allow => {
            let mut permission_decision = PermissionDecision::allow();
            permission_decision.updated_input = decision.updated_input;
            permission_decision.permission_updates = decision.updated_permissions;
            Ok(Some(permission_decision))
        }
        PermissionHookBehavior::Ask => Ok(None),
        PermissionHookBehavior::Deny => Ok(Some(PermissionDecision::deny(
            decision
                .message
                .unwrap_or_else(|| "Permission denied by hook.".to_owned()),
        ))),
    }
}

pub fn wrap_permission_broker_with_hooks(
    broker: Arc<dyn PermissionBroker>,
    discovery: &RuntimeHookDiscovery,
    config: &RuntimeConfig,
) -> Arc<dyn PermissionBroker> {
    if !discovery
        .hooks
        .iter()
        .any(|hook| hook.supported && hook.event == HookEventName::PermissionRequest)
    {
        return broker;
    }
    let store = match SessionStore::open(config.paths.clone()) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!("failed to open hook-aware permission store: {error}");
            return broker;
        }
    };
    let state = HookRunState::load(&store, config.session_id).unwrap_or_default();
    Arc::new(HookAwarePermissionBroker {
        inner: broker,
        discovery: discovery.clone(),
        config: config.clone(),
        store,
        state: Mutex::new(state),
        options: HookExecutionOptions::persistent(),
    })
}

struct HookAwarePermissionBroker {
    inner: Arc<dyn PermissionBroker>,
    discovery: RuntimeHookDiscovery,
    config: RuntimeConfig,
    store: SessionStore,
    state: Mutex<HookRunState>,
    options: HookExecutionOptions,
}

#[async_trait]
impl PermissionBroker for HookAwarePermissionBroker {
    async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
        let hook_result = {
            let mut state = self.state.lock().await;
            apply_permission_request_hooks(
                &self.discovery,
                &self.config,
                &self.store,
                &mut state,
                &request,
                self.options,
            )
            .await
        };
        match hook_result {
            Ok(Some(decision)) => decision,
            Ok(None) => self.inner.decide(request).await,
            Err(error) => PermissionDecision::deny(format!("Permission hook failed: {error}")),
        }
    }

    async fn decide_forced_prompt(&self, request: PermissionRequest) -> PermissionDecision {
        let hook_result = {
            let mut state = self.state.lock().await;
            apply_permission_request_hooks(
                &self.discovery,
                &self.config,
                &self.store,
                &mut state,
                &request,
                self.options,
            )
            .await
        };
        match hook_result {
            Ok(Some(decision)) => decision,
            Ok(None) => self.inner.decide_forced_prompt(request).await,
            Err(error) => PermissionDecision::deny(format!("Permission hook failed: {error}")),
        }
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

    fn apply_permission_updates(&self, updates: &[PermissionUpdate]) -> Result<usize> {
        self.inner.apply_permission_updates(updates)
    }

    fn mode(&self) -> Option<claude_core::PermissionMode> {
        self.inner.mode()
    }

    fn additional_working_directories(&self) -> Vec<PathBuf> {
        self.inner.additional_working_directories()
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
}

async fn run_hooks_list(config: &RuntimeConfig, args: HooksListArgs) -> Result<()> {
    let output = build_hooks_list_output(config, &args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if output.hooks.is_empty() {
        println!("No hooks found.");
        for warning in output.warnings {
            println!("  - {warning}");
        }
        return Ok(());
    }
    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    for hook in &output.hooks {
        println!(
            "{}  type={}  source={}  matcher={}  timeout={}s  once={}  supported={}",
            hook.event.as_str(),
            hook.hook_type,
            format_source(hook),
            hook.matcher.as_deref().unwrap_or("*"),
            hook.timeout_secs,
            if hook.once { "yes" } else { "no" },
            if hook.supported { "yes" } else { "no" }
        );
        println!("  {}", hook.display);
        if let Some(warning) = &hook.warning {
            println!("  warning: {warning}");
        }
    }
    Ok(())
}

fn push_source_if_exists(
    sources: &mut Vec<HookSourceDescriptor>,
    source_kind: &str,
    source_name: &str,
    source_path: PathBuf,
    plugin_name: Option<String>,
) {
    if source_path.exists() {
        sources.push(HookSourceDescriptor {
            source_kind: source_kind.to_owned(),
            source_name: source_name.to_owned(),
            source_path,
            plugin_name,
        });
    }
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(path.display().to_string()));
}

fn load_hooks_from_source(source: &HookSourceDescriptor) -> Result<Vec<HookRecord>> {
    let raw = fs::read_to_string(&source.source_path)
        .with_context(|| format!("failed to read {}", source.source_path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", source.source_path.display()))?;
    let object = value.as_object().ok_or_else(|| {
        anyhow!(
            "{} did not contain a JSON object",
            source.source_path.display()
        )
    })?;
    let hooks_object = object
        .get("hooks")
        .and_then(Value::as_object)
        .unwrap_or(object);

    let mut hooks = Vec::new();
    for (event_key, matchers_value) in hooks_object {
        let Some(event) = HookEventName::parse_key(event_key) else {
            continue;
        };
        let Some(matchers) = matchers_value.as_array() else {
            continue;
        };
        for (matcher_index, matcher_value) in matchers.iter().enumerate() {
            let Some(matcher_object) = matcher_value.as_object() else {
                continue;
            };
            let matcher = matcher_object
                .get("matcher")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let Some(hook_values) = matcher_object.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for (hook_index, hook_value) in hook_values.iter().enumerate() {
                hooks.push(parse_hook_record(
                    source,
                    event,
                    matcher.clone(),
                    matcher_index,
                    hook_index,
                    hook_value,
                ));
            }
        }
    }
    Ok(hooks)
}

fn parse_hook_record(
    source: &HookSourceDescriptor,
    event: HookEventName,
    matcher: Option<String>,
    matcher_index: usize,
    hook_index: usize,
    hook_value: &Value,
) -> HookRecord {
    let object = hook_value.as_object();
    let hook_type = object
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let display = hook_display(&hook_type, object);
    let shell_raw = object
        .and_then(|value| value.get("shell"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let timeout_secs = object
        .and_then(|value| value.get("timeout"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS)
        .max(1);
    let once = object
        .and_then(|value| value.get("once"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_async = object
        .and_then(|value| value.get("async"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || object
            .and_then(|value| value.get("asyncRewake"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let (supported, warning, shell) = if hook_type != "command" {
        (
            false,
            Some(format!(
                "Unsupported hook type `{hook_type}` in {}",
                source.source_path.display()
            )),
            None,
        )
    } else if is_async {
        (
            false,
            Some("Async hook execution is not supported yet in remote-code-rust.".to_owned()),
            shell_raw.clone(),
        )
    } else if HookShell::parse(shell_raw.as_deref()).is_none() {
        (
            false,
            Some(format!(
                "Unsupported hook shell `{}`.",
                shell_raw.as_deref().unwrap_or_default()
            )),
            shell_raw.clone(),
        )
    } else {
        (
            true,
            None,
            Some(
                HookShell::parse(shell_raw.as_deref())
                    .unwrap_or_else(HookShell::default_for_host)
                    .label()
                    .to_owned(),
            ),
        )
    };
    let hook_id = stable_hook_id(&[
        source.source_kind.as_str(),
        source.source_name.as_str(),
        source.source_path.to_string_lossy().as_ref(),
        event.as_str(),
        matcher.as_deref().unwrap_or_default(),
        &matcher_index.to_string(),
        &hook_index.to_string(),
        &hook_type,
        &display,
    ]);
    HookRecord {
        hook_id,
        event,
        hook_type,
        display,
        matcher,
        source_kind: source.source_kind.clone(),
        source_name: source.source_name.clone(),
        source_path: source.source_path.clone(),
        plugin_name: source.plugin_name.clone(),
        shell,
        timeout_secs,
        once,
        supported,
        warning,
    }
}

fn hook_display(hook_type: &str, object: Option<&serde_json::Map<String, Value>>) -> String {
    match hook_type {
        "command" => object
            .and_then(|value| value.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("(missing command)")
            .to_owned(),
        "http" => object
            .and_then(|value| value.get("url"))
            .and_then(Value::as_str)
            .map(|value| format!("POST {value}"))
            .unwrap_or_else(|| "(missing url)".to_owned()),
        "prompt" | "agent" => object
            .and_then(|value| value.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("(missing prompt)")
            .to_owned(),
        _ => hook_type.to_owned(),
    }
}

fn stable_hook_id(parts: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn matches_hook_subject(matcher: Option<&str>, subject: &str) -> bool {
    let Some(matcher) = matcher.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let matcher = matcher.to_ascii_lowercase();
    let subject = subject.to_ascii_lowercase();
    if matcher.contains('*') || matcher.contains('?') {
        wildcard_match(&matcher, &subject)
    } else {
        subject.contains(&matcher)
    }
}

fn wildcard_match(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.as_bytes();
    let candidate = candidate.as_bytes();
    let mut dp = vec![vec![false; candidate.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;
    for index in 0..pattern.len() {
        if pattern[index] == b'*' {
            dp[index + 1][0] = dp[index][0];
        }
    }
    for i in 0..pattern.len() {
        for j in 0..candidate.len() {
            dp[i + 1][j + 1] = match pattern[i] {
                b'*' => dp[i][j + 1] || dp[i + 1][j],
                b'?' => dp[i][j],
                byte => dp[i][j] && byte == candidate[j],
            };
        }
    }
    dp[pattern.len()][candidate.len()]
}

#[allow(clippy::too_many_arguments)]
async fn run_event_hooks(
    discovery: &RuntimeHookDiscovery,
    event: HookEventName,
    config: &RuntimeConfig,
    store: &SessionStore,
    state: &mut HookRunState,
    subject: String,
    input: &Value,
    blocking: bool,
    options: HookExecutionOptions,
) -> Result<HookEffects> {
    let mut effects = HookEffects::default();
    for hook in discovery
        .hooks
        .iter()
        .filter(|hook| hook.supported && hook.event == event)
    {
        if !matches_hook_subject(hook.matcher.as_deref(), &subject) || state.should_skip_once(hook)
        {
            continue;
        }
        let outcome = execute_command_hook(hook, config, input, blocking).await;
        if options.persist {
            store.append_named_event(
                config.session_id,
                "hook_execution",
                json!({
                    "hook_id": hook.hook_id,
                    "event": hook.event.as_str(),
                    "source_kind": hook.source_kind,
                    "source_name": hook.source_name,
                    "plugin_name": hook.plugin_name,
                    "command": hook.display,
                    "status": outcome.status,
                    "blocked_reason": outcome.blocked_reason,
                    "exit_code": outcome.exit_code,
                    "duration_ms": outcome.duration_ms,
                    "stdout_preview": outcome.stdout_preview,
                    "stderr_preview": outcome.stderr_preview,
                    "once": hook.once,
                }),
            )?;
        }
        state.mark_executed(hook);
        if let Some(updated_input) = outcome.updated_input {
            effects.updated_input = Some(updated_input);
        }
        if let Some(permission_decision) = outcome.permission_decision {
            effects.permission_decision = Some(permission_decision);
        }
        if let Some(additional_context) = outcome.additional_context {
            effects.additional_contexts.push(additional_context);
        }
        if let Some(reason) = outcome.blocked_reason {
            effects.blocked_reason = Some(reason);
            break;
        }
    }
    Ok(effects)
}

async fn execute_command_hook(
    hook: &HookRecord,
    config: &RuntimeConfig,
    input: &Value,
    blocking: bool,
) -> ExecutedHookOutcome {
    let Some(shell) = HookShell::parse(hook.shell.as_deref()) else {
        return ExecutedHookOutcome {
            status: "error",
            blocked_reason: blocking
                .then(|| "Hook configuration used an unsupported shell.".to_owned()),
            updated_input: None,
            permission_decision: None,
            additional_context: None,
            exit_code: None,
            duration_ms: 0,
            stdout_preview: String::new(),
            stderr_preview: String::new(),
        };
    };
    let (program, args) = shell.program_and_args(&hook.display);
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&config.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("REMOTE_CODE_HOOK_EVENT", hook.event.display_name())
        .env("REMOTE_CODE_HOOK_ID", &hook.hook_id)
        .env("REMOTE_CODE_HOOK_SOURCE", &hook.source_name)
        .env("REMOTE_CODE_SESSION_ID", config.session_id.to_string())
        .env("REMOTE_CODE_CWD", config.cwd.display().to_string());
    if let Some(plugin_name) = &hook.plugin_name {
        command.env("REMOTE_CODE_PLUGIN_NAME", plugin_name);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ExecutedHookOutcome {
                status: "error",
                blocked_reason: blocking
                    .then(|| format!("Failed to spawn hook `{}`: {error}", hook.display)),
                updated_input: None,
                permission_decision: None,
                additional_context: None,
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_preview: String::new(),
                stderr_preview: error.to_string(),
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Ok(payload) = serde_json::to_vec(input)
    {
        let _ = stdin.write_all(&payload).await;
    }

    let output = timeout(
        Duration::from_secs(hook.timeout_secs.max(1)),
        child.wait_with_output(),
    )
    .await;

    let output = match output {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return ExecutedHookOutcome {
                status: "error",
                blocked_reason: blocking
                    .then(|| format!("Hook `{}` failed to complete: {error}", hook.display)),
                updated_input: None,
                permission_decision: None,
                additional_context: None,
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_preview: String::new(),
                stderr_preview: error.to_string(),
            };
        }
        Err(_) => {
            return ExecutedHookOutcome {
                status: "timeout",
                blocked_reason: blocking.then(|| {
                    format!(
                        "Hook `{}` timed out after {}s.",
                        hook.display, hook.timeout_secs
                    )
                }),
                updated_input: None,
                permission_decision: None,
                additional_context: None,
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_preview: String::new(),
                stderr_preview: String::new(),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let parsed = parse_hook_response(&stdout);
    let updated_input = parsed.as_ref().and_then(normalized_updated_input);
    let permission_decision = parsed
        .as_ref()
        .and_then(normalized_permission_hook_decision);
    let additional_context = parsed.as_ref().and_then(normalized_additional_context);
    let blocked_reason = if parsed.as_ref().is_some_and(hook_response_blocks) {
        Some(
            parsed
                .as_ref()
                .and_then(normalized_stop_reason)
                .unwrap_or_else(|| format!("Hook `{}` blocked the action.", hook.display)),
        )
    } else if blocking && !output.status.success() {
        Some(if !stderr.is_empty() {
            stderr.clone()
        } else if !stdout.is_empty() {
            stdout.clone()
        } else {
            format!(
                "Hook `{}` exited with code {}.",
                hook.display,
                output.status.code().unwrap_or(-1)
            )
        })
    } else {
        None
    };

    ExecutedHookOutcome {
        status: if blocked_reason.is_some() {
            "blocked"
        } else if output.status.success() {
            "ok"
        } else {
            "error"
        },
        blocked_reason,
        updated_input,
        permission_decision,
        additional_context,
        exit_code: output.status.code(),
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_preview: truncate_preview(&stdout, 200),
        stderr_preview: truncate_preview(&stderr, 200),
    }
}

fn parse_hook_response(stdout: &str) -> Option<HookResponse> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok().or_else(|| {
        trimmed
            .lines()
            .last()
            .and_then(|line| serde_json::from_str(line.trim()).ok())
    })
}

fn normalized_updated_input(response: &HookResponse) -> Option<Value> {
    response.updated_input.clone().or_else(|| {
        response
            .hook_specific_output
            .as_ref()
            .and_then(|value| value.updated_input.clone())
    })
}

fn normalized_permission_hook_decision(response: &HookResponse) -> Option<PermissionHookDecision> {
    let hook_specific = response.hook_specific_output.as_ref()?;
    if let Some(decision) = hook_specific.decision.clone() {
        return Some(decision);
    }
    hook_specific
        .permission_decision
        .map(|behavior| PermissionHookDecision {
            behavior,
            updated_input: hook_specific.updated_input.clone(),
            updated_permissions: Vec::new(),
            message: hook_specific.permission_decision_reason.clone(),
        })
}

fn normalized_additional_context(response: &HookResponse) -> Option<String> {
    response
        .additional_context
        .clone()
        .or_else(|| {
            response
                .hook_specific_output
                .as_ref()
                .and_then(|value| value.additional_context.clone())
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalized_stop_reason(response: &HookResponse) -> Option<String> {
    response
        .stop_reason
        .clone()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn hook_response_blocks(response: &HookResponse) -> bool {
    response.r#continue == Some(false)
        || response
            .decision
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("block"))
}

fn append_contexts(
    store: &SessionStore,
    session_id: Uuid,
    conversation: &mut Vec<ConversationEntry>,
    event: HookEventName,
    contexts: &[String],
    persist: bool,
) -> Result<()> {
    for context in contexts {
        let entry =
            ConversationEntry::system(format!("Hook context ({}):\n{}", event.as_str(), context));
        if persist {
            store.append_conversation_entry(session_id, &entry)?;
        }
        conversation.push(entry);
        if persist {
            store.append_named_event(
                session_id,
                "hook_context",
                json!({
                    "event": event.as_str(),
                    "text_preview": truncate_preview(context, 200),
                }),
            )?;
        }
    }
    Ok(())
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    trimmed.chars().take(max_chars).collect::<String>() + "..."
}

fn format_source(hook: &HookRecord) -> String {
    match &hook.plugin_name {
        Some(plugin_name) => format!("{}:{plugin_name}", hook.source_kind),
        None => format!("{}:{}", hook.source_kind, hook.source_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc as StdArc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use tempfile::tempdir;

    struct CountingAllowBroker {
        calls: StdArc<AtomicUsize>,
    }

    #[async_trait]
    impl PermissionBroker for CountingAllowBroker {
        async fn decide(&self, _request: PermissionRequest) -> PermissionDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            PermissionDecision::allow()
        }
    }

    fn json_emitting_hook(json_body: &str) -> (String, String) {
        if cfg!(windows) {
            (
                "powershell".to_owned(),
                format!(
                    "$null = [Console]::In.ReadToEnd(); $json = @'\n{json_body}\n'@; Write-Output $json"
                ),
            )
        } else {
            (
                "sh".to_owned(),
                format!(
                    "cat >/dev/null; printf '%s\\n' '{}'",
                    json_body.replace('\'', "'\\''")
                ),
            )
        }
    }

    fn config_and_store() -> (tempfile::TempDir, RuntimeConfig, SessionStore) {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));
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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));
        let store = SessionStore::open(config.paths.clone())
            .unwrap_or_else(|error| panic!("store open failed: {error}"));
        (tempdir, config, store)
    }

    #[test]
    fn discovers_profile_project_and_plugin_hooks() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));
        fs::write(
            profile.join("hooks.json"),
            r#"{"session_start":[{"hooks":[{"type":"command","command":"echo profile"}]}]}"#,
        )
        .unwrap_or_else(|error| panic!("profile hooks write failed: {error}"));
        let workspace_dir = cwd.join(".remote-code");
        fs::create_dir_all(&workspace_dir)
            .unwrap_or_else(|error| panic!("workspace dir create failed: {error}"));
        fs::write(
            workspace_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"write","hooks":[{"type":"command","command":"echo workspace"}]}]}}"#,
        )
        .unwrap_or_else(|error| panic!("workspace settings write failed: {error}"));
        let plugin_root = profile.join("plugins").join("demo-plugin");
        fs::create_dir_all(plugin_root.join(".codex-plugin"))
            .unwrap_or_else(|error| panic!("plugin dir create failed: {error}"));
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"demo-plugin","version":"0.1.0","hooks":"./hooks.json"}"#,
        )
        .unwrap_or_else(|error| panic!("plugin manifest write failed: {error}"));
        fs::write(
            plugin_root.join("hooks.json"),
            r#"{"post_tool_use_failure":[{"hooks":[{"type":"command","command":"echo plugin"}]}]}"#,
        )
        .unwrap_or_else(|error| panic!("plugin hooks write failed: {error}"));

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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));
        let discovery = discover_runtime_hooks(&config, &[]);
        let names = discovery
            .hooks
            .iter()
            .map(|hook| (hook.event.as_str().to_owned(), hook.source_kind.clone()))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                ("post_tool_use_failure".to_owned(), "plugin".to_owned()),
                ("pre_tool_use".to_owned(), "project".to_owned()),
                ("session_start".to_owned(), "profile".to_owned()),
            ])
        );
    }

    #[test]
    fn discover_runtime_hooks_respects_setting_sources_and_explicit_settings() {
        let (_tempdir, mut config, _store) = config_and_store();
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            r#"{"session_start":[{"hooks":[{"type":"command","command":"echo profile"}]}]}"#,
        )
        .unwrap_or_else(|error| panic!("profile hooks write failed: {error}"));
        let workspace_dir = config.cwd.join(".remote-code");
        fs::create_dir_all(&workspace_dir)
            .unwrap_or_else(|error| panic!("workspace dir create failed: {error}"));
        fs::write(
            workspace_dir.join("hooks.json"),
            r#"{"pre_tool_use":[{"hooks":[{"type":"command","command":"echo project"}]}]}"#,
        )
        .unwrap_or_else(|error| panic!("project hooks write failed: {error}"));
        fs::write(
            workspace_dir.join("settings.local.json"),
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo local"}]}]}}"#,
        )
        .unwrap_or_else(|error| panic!("local settings write failed: {error}"));

        config.allowed_setting_sources = vec![SettingSource::Local];
        config.settings_files = vec![workspace_dir.join("settings.local.json")];
        let discovery = discover_runtime_hooks(&config, &[]);
        let names = discovery
            .hooks
            .iter()
            .map(|hook| (hook.event.as_str().to_owned(), hook.source_kind.clone()))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([("post_tool_use".to_owned(), "local".to_owned())])
        );

        let explicit = workspace_dir.join("extra-settings.json");
        fs::write(
            &explicit,
            r#"{"hooks":{"PermissionDenied":[{"hooks":[{"type":"command","command":"echo explicit"}]}]}}"#,
        )
        .unwrap_or_else(|error| panic!("explicit settings write failed: {error}"));
        config.allowed_setting_sources = vec![SettingSource::Local];
        config.settings_files = vec![explicit.clone()];
        config.cli_settings_files = vec![explicit];
        let discovery = discover_runtime_hooks(&config, &[]);
        let names = discovery
            .hooks
            .iter()
            .map(|hook| (hook.event.as_str().to_owned(), hook.source_kind.clone()))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([("permission_denied".to_owned(), "explicit".to_owned())])
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_updates_input_and_once_state_persists() {
        let (_tempdir, config, store) = config_and_store();
        let (shell, command) =
            json_emitting_hook(r#"{"updatedInput":{"path":"from-hook.txt","content":"patched"}}"#);
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"pre_tool_use":[{{"matcher":"write","hooks":[{{"type":"command","command":"{}","shell":"{}","once":true}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                shell
            ),
        )
        .unwrap_or_else(|error| panic!("hooks write failed: {error}"));

        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                "mock",
                Some("test"),
                Some("hooks"),
            )
            .unwrap_or_else(|error| panic!("ensure session failed: {error}"));
        let discovery = discover_runtime_hooks(&config, &[]);
        let mut state = HookRunState::load(&store, config.session_id)
            .unwrap_or_else(|error| panic!("hook state load failed: {error}"));
        let mut conversation = vec![ConversationEntry::system("system")];

        let prepared = apply_pre_tool_use_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &mut state,
            &ToolCall {
                id: "tool-1".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"path":"original.txt","content":"hello"}),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("pre hook apply failed: {error}"));
        assert_eq!(
            prepared.call.input.get("path").and_then(Value::as_str),
            Some("from-hook.txt")
        );

        let mut state = HookRunState::load(&store, config.session_id)
            .unwrap_or_else(|error| panic!("hook state reload failed: {error}"));
        let prepared = apply_pre_tool_use_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &mut state,
            &ToolCall {
                id: "tool-2".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"path":"second.txt","content":"hello"}),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("second pre hook apply failed: {error}"));
        assert_eq!(
            prepared.call.input.get("path").and_then(Value::as_str),
            Some("second.txt")
        );
    }

    #[tokio::test]
    async fn permission_request_hook_can_deny_before_delegate_broker() {
        let (_tempdir, config, store) = config_and_store();
        let (shell, command) = json_emitting_hook(
            r#"{"hookSpecificOutput":{"decision":{"behavior":"deny","message":"blocked by hook"}}}"#,
        );
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"PermissionRequest":[{{"matcher":"write_file","hooks":[{{"type":"command","command":"{}","shell":"{}"}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                shell
            ),
        )
        .unwrap_or_else(|error| panic!("hooks write failed: {error}"));

        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                "mock",
                Some("test"),
                Some("hooks"),
            )
            .unwrap_or_else(|error| panic!("ensure session failed: {error}"));

        let discovery = discover_runtime_hooks(&config, &[]);
        let calls = StdArc::new(AtomicUsize::new(0));
        let broker = wrap_permission_broker_with_hooks(
            StdArc::new(CountingAllowBroker {
                calls: calls.clone(),
            }),
            &discovery,
            &config,
        );
        let decision = broker
            .decide(PermissionRequest {
                tool_name: "write_file".to_owned(),
                permission_class: None,
                tool_input: json!({"path":"secret.txt","content":"value"}),
                working_directory: Some(config.cwd.display().to_string()),
                tool_use_id: Some("tool-1".to_owned()),
                title: Some("Write file".to_owned()),
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;

        assert!(!decision.allowed);
        assert_eq!(decision.message.as_deref(), Some("blocked by hook"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn hook_parser_accepts_upstream_permission_decision_shape() {
        let parsed = parse_hook_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"blocked upstream","updatedInput":{"command":"git status"}}}"#,
        )
        .expect("parse hook response");

        let decision = normalized_permission_hook_decision(&parsed).expect("permission decision");
        assert_eq!(decision.behavior, PermissionHookBehavior::Deny);
        assert_eq!(decision.message.as_deref(), Some("blocked upstream"));
        assert_eq!(
            decision.updated_input,
            Some(json!({"command": "git status"}))
        );
    }

    #[tokio::test]
    async fn session_start_hook_appends_context_only_once() {
        let (_tempdir, config, store) = config_and_store();
        let (shell, command) =
            json_emitting_hook(r#"{"additionalContext":"Inspect the repo before acting."}"#);
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"SessionStart":[{{"hooks":[{{"type":"command","command":"{}","shell":"{}"}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                shell
            ),
        )
        .unwrap_or_else(|error| panic!("hooks write failed: {error}"));

        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                "mock",
                Some("test"),
                Some("hooks"),
            )
            .unwrap_or_else(|error| panic!("ensure session failed: {error}"));
        let discovery = discover_runtime_hooks(&config, &[]);
        let mut state = HookRunState::load(&store, config.session_id)
            .unwrap_or_else(|error| panic!("hook state load failed: {error}"));
        let mut conversation = vec![ConversationEntry::system("system")];

        ensure_session_start_hooks(&discovery, &config, &store, &mut conversation, &mut state)
            .await
            .unwrap_or_else(|error| panic!("session start hook failed: {error}"));
        assert_eq!(conversation.len(), 2);

        let mut state = HookRunState::load(&store, config.session_id)
            .unwrap_or_else(|error| panic!("hook state reload failed: {error}"));
        ensure_session_start_hooks(&discovery, &config, &store, &mut conversation, &mut state)
            .await
            .unwrap_or_else(|error| panic!("second session start hook failed: {error}"));
        assert_eq!(conversation.len(), 2);
    }
}
