use std::collections::{BTreeSet, HashMap, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use claude_config::{RuntimeConfig, SettingSource};
use claude_core::hook_executor::{HookBatchResult, HookExecutor, format_blocking_message};
use claude_core::hook_matcher::match_tool_name;
use claude_core::{
    ConversationEntry, HookDefinition, HookInput, HookMatcherEntry, HookPermissionBehavior,
    ToolCall, ToolResult,
};
use claude_core::{HOOK_EVENTS, HookEventKind, parse_hook_event};
use claude_plugins::discover_plugins;
use claude_session::SessionStore;
use serde_json::{Map, Value, json};

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
struct NamedHook {
    hook_id: String,
    definition: HookDefinition,
}

#[derive(Debug, Clone)]
struct NamedHookMatcher {
    matcher: Option<String>,
    hooks: Vec<NamedHook>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeSessionHookDiscovery {
    hooks: HashMap<HookEventKind, Vec<NamedHookMatcher>>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionHookRunOutcome {
    pub appended_entries: Vec<ConversationEntry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolHookRunOutcome {
    pub appended_entries: Vec<ConversationEntry>,
}

#[derive(Debug, Clone)]
pub struct PreparedToolCall {
    pub call: ToolCall,
    pub blocked_reason: Option<String>,
    pub appended_entries: Vec<ConversationEntry>,
}

#[derive(Debug, Default)]
struct RuntimeSessionHookState {
    consumed_once_hook_ids: BTreeSet<String>,
    session_start_completed: bool,
}

impl RuntimeSessionHookState {
    fn load(store: &SessionStore, session_id: uuid::Uuid) -> Result<Self> {
        let Ok(transcript) = store.load_transcript(session_id) else {
            return Ok(Self::default());
        };
        Ok(Self {
            consumed_once_hook_ids: transcript.consumed_once_hook_ids(),
            session_start_completed: transcript.has_hook_phase("session_start"),
        })
    }
}

pub fn discover_runtime_session_hooks(config: &RuntimeConfig) -> RuntimeSessionHookDiscovery {
    let mut discovery = RuntimeSessionHookDiscovery::default();

    if setting_source_enabled(config, SettingSource::User) {
        let profile_hooks = config.paths.profile_dir.join("hooks.json");
        load_hook_source(&profile_hooks, false, "profile hooks", &mut discovery);
    }

    if setting_source_enabled(config, SettingSource::Project) {
        let project_hooks = config.cwd.join(".remote-code").join("hooks.json");
        load_hook_source(&project_hooks, false, "project hooks", &mut discovery);
    }

    for settings_file in &config.settings_files {
        load_hook_source(
            settings_file,
            true,
            &format!("settings {}", settings_file.display()),
            &mut discovery,
        );
    }

    if setting_source_enabled(config, SettingSource::User) && config.paths.plugins_dir.exists() {
        match discover_plugins(&config.paths.plugins_dir) {
            Ok(plugins) => {
                for plugin in plugins {
                    if let Some(path) = plugin.hooks_config_path() {
                        load_hook_source(
                            &path,
                            false,
                            &format!("plugin {}", plugin.manifest.name),
                            &mut discovery,
                        );
                    }
                }
            }
            Err(error) => discovery
                .warnings
                .push(format!("Failed to discover plugins: {error}")),
        }
    }

    discovery
}

pub async fn ensure_session_start_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<SessionHookRunOutcome> {
    store.ensure_session(
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        config.session_name.as_deref(),
    )?;
    let state = RuntimeSessionHookState::load(store, config.session_id)?;
    if state.session_start_completed {
        return Ok(SessionHookRunOutcome {
            appended_entries: Vec::new(),
            warnings: discovery.warnings.clone(),
        });
    }

    execute_session_lifecycle_hooks(
        discovery,
        HookEventKind::SessionStart,
        config,
        store,
        conversation,
        true,
        true,
    )
    .await
}

pub async fn run_session_end_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
) -> Result<SessionHookRunOutcome> {
    store.ensure_session(
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        config.session_name.as_deref(),
    )?;
    let mut scratch = Vec::new();
    execute_session_lifecycle_hooks(
        discovery,
        HookEventKind::SessionEnd,
        config,
        store,
        &mut scratch,
        false,
        false,
    )
    .await
}

pub async fn apply_pre_tool_use_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    tool_call: &ToolCall,
) -> Result<PreparedToolCall> {
    let state = RuntimeSessionHookState::load(store, config.session_id)?;
    let hooks = matched_named_hooks(
        discovery,
        HookEventKind::PreToolUse,
        Some(tool_call.name.as_str()),
        &state.consumed_once_hook_ids,
    );
    if hooks.is_empty() {
        return Ok(PreparedToolCall {
            call: tool_call.clone(),
            blocked_reason: None,
            appended_entries: Vec::new(),
        });
    }

    let input = HookInput {
        event: HookEventKind::PreToolUse,
        tool_name: Some(tool_call.name.clone()),
        tool_input: Some(tool_call.input.clone()),
        session_id: Some(config.session_id.to_string()),
        cwd: Some(config.cwd.display().to_string()),
        user_prompt: None,
        tool_use_id: Some(tool_call.id.clone()),
        tool_result: None,
    };
    let batch =
        execute_named_hook_batch(&hooks, HookEventKind::PreToolUse, config, store, &input).await?;

    let mut appended_entries = append_hook_context_entries(
        store,
        config.session_id,
        conversation,
        HookEventKind::PreToolUse,
        &batch.aggregated.additional_contexts,
    )?;

    let mut call = tool_call.clone();
    if let Some(updated_input) = batch.aggregated.updated_input.clone()
        && updated_input.is_object()
    {
        let changed = updated_input != call.input;
        call.input = updated_input;
        if changed && appended_entries.is_empty() {
            appended_entries.extend(append_hook_context_entries(
                store,
                config.session_id,
                conversation,
                HookEventKind::PreToolUse,
                &[format!(
                    "A hook adjusted the input for `{}` before execution.",
                    call.name
                )],
            )?);
        }
    }

    let blocked_reason = blocking_reason_for_batch(&batch).or_else(|| {
        (batch.aggregated.permission_behavior == Some(HookPermissionBehavior::Deny)).then(|| {
            batch
                .aggregated
                .permission_decision_reason
                .clone()
                .unwrap_or_else(|| format!("A pre-tool hook denied `{}`.", call.name))
        })
    });

    Ok(PreparedToolCall {
        call,
        blocked_reason,
        appended_entries,
    })
}

pub async fn apply_post_tool_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    tool_call: &ToolCall,
    tool_result: &mut ToolResult,
) -> Result<ToolHookRunOutcome> {
    let state = RuntimeSessionHookState::load(store, config.session_id)?;
    let event = if tool_result.is_error {
        HookEventKind::PostToolUseFailure
    } else {
        HookEventKind::PostToolUse
    };
    let hooks = matched_named_hooks(
        discovery,
        event,
        Some(tool_call.name.as_str()),
        &state.consumed_once_hook_ids,
    );
    if hooks.is_empty() {
        return Ok(ToolHookRunOutcome::default());
    }

    let input = HookInput {
        event,
        tool_name: Some(tool_call.name.clone()),
        tool_input: Some(tool_call.input.clone()),
        session_id: Some(config.session_id.to_string()),
        cwd: Some(config.cwd.display().to_string()),
        user_prompt: None,
        tool_use_id: Some(tool_call.id.clone()),
        tool_result: Some(json!({
            "content": tool_result.content.clone(),
            "is_error": tool_result.is_error,
        })),
    };
    let batch = execute_named_hook_batch(&hooks, event, config, store, &input).await?;
    if let Some(updated_output) = batch.aggregated.updated_mcp_tool_output.as_ref() {
        tool_result.content = match updated_output {
            Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other)?,
        };
    }

    let appended_entries = append_hook_context_entries(
        store,
        config.session_id,
        conversation,
        event,
        &batch.aggregated.additional_contexts,
    )?;
    Ok(ToolHookRunOutcome { appended_entries })
}

async fn execute_session_lifecycle_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    event: HookEventKind,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    append_messages: bool,
    mark_phase: bool,
) -> Result<SessionHookRunOutcome> {
    let state = RuntimeSessionHookState::load(store, config.session_id)?;
    let hooks = matched_named_hooks(discovery, event, None, &state.consumed_once_hook_ids);

    if hooks.is_empty() {
        if mark_phase {
            store.append_named_event(
                config.session_id,
                "hook_phase",
                json!({ "phase": "session_start" }),
            )?;
        }
        return Ok(SessionHookRunOutcome {
            appended_entries: Vec::new(),
            warnings: discovery.warnings.clone(),
        });
    }

    let input = HookInput {
        event,
        tool_name: None,
        tool_input: None,
        session_id: Some(config.session_id.to_string()),
        cwd: Some(config.cwd.display().to_string()),
        user_prompt: None,
        tool_use_id: None,
        tool_result: None,
    };
    let batch = execute_named_hook_batch(&hooks, event, config, store, &input).await?;

    if batch.is_blocked() {
        let reason = blocking_reason_for_batch(&batch)
            .unwrap_or_else(|| format_blocking_message(&batch.outcomes));
        return Err(anyhow!(reason));
    }

    let mut appended_entries = if append_messages {
        append_hook_context_entries(
            store,
            config.session_id,
            conversation,
            event,
            &batch.aggregated.additional_contexts,
        )?
    } else {
        Vec::new()
    };
    if append_messages
        && let Some(initial_user_message) = batch.aggregated.initial_user_message.as_ref()
    {
        let trimmed = initial_user_message.trim();
        if !trimmed.is_empty() {
            let entry = ConversationEntry::user(trimmed);
            store.append_conversation_entry(config.session_id, &entry)?;
            conversation.push(entry.clone());
            appended_entries.push(entry);
        }
    }

    if mark_phase {
        store.append_named_event(
            config.session_id,
            "hook_phase",
            json!({ "phase": "session_start" }),
        )?;
    }

    Ok(SessionHookRunOutcome {
        appended_entries,
        warnings: discovery.warnings.clone(),
    })
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn load_hook_source(
    path: &Path,
    settings_file: bool,
    label: &str,
    discovery: &mut RuntimeSessionHookDiscovery,
) {
    if !path.exists() {
        return;
    }

    match load_hook_source_value(path, settings_file)
        .and_then(|value| parse_hook_value(path, value))
    {
        Ok(parsed) => merge_hook_map(discovery, parsed, path),
        Err(error) => discovery.warnings.push(format!(
            "Failed to load {label} from {}: {error}",
            path.display()
        )),
    }
}

fn load_hook_source_value(path: &Path, settings_file: bool) -> Result<Value> {
    let root = load_generic_value(path)?;
    if !settings_file {
        return Ok(root);
    }

    Ok(root
        .as_object()
        .and_then(|map| map.get("hooks"))
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new())))
}

fn load_generic_value(path: &Path) -> Result<Value> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if extension == "json" {
        return serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse JSON file {}", path.display()));
    }

    if extension == "toml" {
        let value: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse TOML file {}", path.display()))?;
        return serde_json::to_value(value)
            .with_context(|| format!("failed to convert TOML file {}", path.display()));
    }

    match toml::from_str::<toml::Value>(&raw) {
        Ok(value) => serde_json::to_value(value)
            .with_context(|| format!("failed to convert TOML file {}", path.display())),
        Err(toml_error) => serde_json::from_str(&raw).map_err(|json_error| {
            anyhow!(
                "failed to parse {} as TOML ({toml_error}) or JSON ({json_error})",
                path.display()
            )
        }),
    }
}

fn parse_hook_value(
    path: &Path,
    value: Value,
) -> Result<HashMap<HookEventKind, Vec<NamedHookMatcher>>> {
    let raw_map: HashMap<String, Vec<HookMatcherEntry>> = serde_json::from_value(value)
        .with_context(|| format!("failed to decode hooks from {}", path.display()))?;
    let mut parsed = HashMap::<HookEventKind, Vec<NamedHookMatcher>>::new();

    for (raw_event_name, matchers) in raw_map {
        let Some(event) = normalize_hook_event_name(&raw_event_name) else {
            continue;
        };
        let named_matchers = matchers
            .into_iter()
            .map(|matcher| NamedHookMatcher {
                matcher: matcher.matcher.clone(),
                hooks: matcher
                    .hooks
                    .into_iter()
                    .map(|hook| NamedHook {
                        hook_id: build_hook_id(path, event, matcher.matcher.as_deref(), &hook),
                        definition: hook,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        parsed.entry(event).or_default().extend(named_matchers);
    }

    Ok(parsed)
}

fn merge_hook_map(
    discovery: &mut RuntimeSessionHookDiscovery,
    parsed: HashMap<HookEventKind, Vec<NamedHookMatcher>>,
    _path: &Path,
) {
    for (event, matchers) in parsed {
        discovery.hooks.entry(event).or_default().extend(matchers);
    }
}

fn normalize_hook_event_name(raw: &str) -> Option<HookEventKind> {
    if let Some(event) = parse_hook_event(raw) {
        return Some(event);
    }

    let normalized = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();

    HOOK_EVENTS.iter().copied().find(|event| {
        event
            .as_str()
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
            == normalized
    })
}

fn build_hook_id(
    path: &Path,
    event: HookEventKind,
    matcher: Option<&str>,
    hook: &HookDefinition,
) -> String {
    let mut hasher = DefaultHasher::new();
    path.display().to_string().hash(&mut hasher);
    event.as_str().hash(&mut hasher);
    matcher.unwrap_or("*").hash(&mut hasher);
    hook.dedup_key().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn matched_named_hooks(
    discovery: &RuntimeSessionHookDiscovery,
    event: HookEventKind,
    subject: Option<&str>,
    consumed_once_hook_ids: &BTreeSet<String>,
) -> Vec<NamedHook> {
    let mut seen = BTreeSet::new();
    let mut hooks = Vec::new();

    for matcher in discovery.hooks.get(&event).into_iter().flatten() {
        if !match_tool_name(subject, matcher.matcher.as_deref()) {
            continue;
        }

        for hook in &matcher.hooks {
            if hook.definition.is_once() && consumed_once_hook_ids.contains(&hook.hook_id) {
                continue;
            }
            let dedup_key = hook.definition.dedup_key();
            if seen.insert(dedup_key) {
                hooks.push(hook.clone());
            }
        }
    }

    hooks
}

async fn execute_named_hook_batch(
    hooks: &[NamedHook],
    event: HookEventKind,
    config: &RuntimeConfig,
    store: &SessionStore,
    input: &HookInput,
) -> Result<HookBatchResult> {
    let executor =
        HookExecutor::new(config.cwd.display().to_string()).with_timeout(DEFAULT_HOOK_TIMEOUT_SECS);
    let hook_defs = hooks
        .iter()
        .map(|hook| hook.definition.clone())
        .collect::<Vec<_>>();
    let batch = executor.execute_hooks(&hook_defs, input).await;

    for (named_hook, outcome) in hooks.iter().zip(batch.outcomes.iter()) {
        store.append_named_event(
            config.session_id,
            "hook_execution",
            json!({
                "hook_id": named_hook.hook_id,
                "event": event.as_str(),
                "matcher": input.tool_name.clone(),
                "once": named_hook.definition.is_once(),
                "success": outcome.success,
                "blocked": outcome.blocked,
                "duration_ms": outcome.duration.as_millis() as u64,
                "exit_code": outcome.output.exit_code,
                "stdout_preview": truncate_preview(&outcome.output.stdout, 200),
                "stderr_preview": truncate_preview(&outcome.output.stderr, 200),
            }),
        )?;
    }

    Ok(batch)
}

fn append_hook_context_entries(
    store: &SessionStore,
    session_id: uuid::Uuid,
    conversation: &mut Vec<ConversationEntry>,
    event: HookEventKind,
    contexts: &[String],
) -> Result<Vec<ConversationEntry>> {
    let mut appended_entries = Vec::new();
    for context in contexts {
        let entry =
            ConversationEntry::system(format!("Hook context ({}):\n{}", event.as_str(), context));
        store.append_conversation_entry(session_id, &entry)?;
        store.append_named_event(
            session_id,
            "hook_context",
            json!({
                "event": event.as_str(),
                "text_preview": truncate_preview(context, 200),
            }),
        )?;
        conversation.push(entry.clone());
        appended_entries.push(entry);
    }
    Ok(appended_entries)
}

fn blocking_reason_for_batch(batch: &HookBatchResult) -> Option<String> {
    if !batch.aggregated.blocking_errors.is_empty() {
        Some(batch.aggregated.blocking_errors.join("\n"))
    } else if batch.is_blocked() {
        Some(format_blocking_message(&batch.outcomes))
    } else {
        None
    }
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    trimmed.chars().take(max_chars).collect::<String>() + "..."
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{
        InputFormat, OutputFormat, PermissionMode, ProviderProtocol, ToolCall, ToolResult,
    };
    use tempfile::tempdir;

    fn json_emitting_command(json_body: &str) -> String {
        if cfg!(windows) {
            format!(
                "$null = [Console]::In.ReadToEnd(); $json = @'\n{json_body}\n'@; Write-Output $json"
            )
        } else {
            format!(
                "cat >/dev/null; printf '%s\\n' '{}'",
                json_body.replace('\'', "'\\''")
            )
        }
    }

    fn test_config_and_store() -> (tempfile::TempDir, RuntimeConfig, SessionStore) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
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
                base_url: Some("https://example.invalid/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config");
        let store = SessionStore::open(config.paths.clone()).expect("store");
        (tempdir, config, store)
    }

    #[tokio::test]
    async fn session_start_hooks_append_context_only_once() {
        let (_tempdir, config, store) = test_config_and_store();
        let command = json_emitting_command(
            r#"{"hook_specific_output":{"hookEventName":"SessionStart","additional_context":"Inspect the repo before acting.","initial_user_message":"Start by reading the current implementation."}}"#,
        );
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"session_start":[{{"hooks":[{{"type":"command","command":"{}","shell":"{}","once":true}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                if cfg!(windows) { "powershell" } else { "bash" }
            ),
        )
        .expect("hooks");

        let discovery = discover_runtime_session_hooks(&config);
        let mut conversation = vec![ConversationEntry::system("system prompt")];

        let first = ensure_session_start_hooks(&discovery, &config, &store, &mut conversation)
            .await
            .expect("first start");
        assert_eq!(first.appended_entries.len(), 2);
        assert_eq!(conversation.len(), 3);

        let second = ensure_session_start_hooks(&discovery, &config, &store, &mut conversation)
            .await
            .expect("second start");
        assert!(second.appended_entries.is_empty());
        assert_eq!(conversation.len(), 3);

        let transcript = store
            .load_transcript(config.session_id)
            .expect("transcript");
        assert!(transcript.has_hook_phase("session_start"));
    }

    #[tokio::test]
    async fn session_end_hooks_load_from_settings_file() {
        let (_tempdir, config, store) = test_config_and_store();
        let settings_dir = config.cwd.join(".remote-code");
        fs::create_dir_all(&settings_dir).expect("settings dir");
        let command = json_emitting_command(r#"{"additional_context":"cleanup"}"#);
        fs::write(
            settings_dir.join("settings.json"),
            format!(
                r#"{{
  "hooks": {{
    "SessionEnd": [
      {{
        "hooks": [
          {{
            "type": "command",
            "command": "{}",
            "shell": "{}"
          }}
        ]
      }}
    ]
  }}
}}"#,
                command
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n"),
                if cfg!(windows) { "powershell" } else { "bash" }
            ),
        )
        .expect("settings");

        let refreshed = RuntimeConfig {
            settings_files: vec![settings_dir.join("settings.json")],
            ..config.clone()
        };
        let discovery = discover_runtime_session_hooks(&refreshed);
        let outcome = run_session_end_hooks(&discovery, &refreshed, &store)
            .await
            .expect("session end");
        assert!(outcome.appended_entries.is_empty());

        let transcript = store
            .load_transcript(refreshed.session_id)
            .expect("transcript");
        let execution = transcript
            .latest_named_event_payload("hook_execution")
            .expect("hook execution payload");
        assert_eq!(execution["event"], "SessionEnd");
    }

    #[tokio::test]
    async fn pre_tool_hooks_can_update_input_and_append_context() {
        let (_tempdir, config, store) = test_config_and_store();
        let command = json_emitting_command(
            r#"{"hook_specific_output":{"hookEventName":"PreToolUse","updated_input":{"command":"echo patched"},"additional_context":"pre context"}} "#.trim(),
        );
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"PreToolUse":[{{"matcher":"bash_command","hooks":[{{"type":"command","command":"{}","shell":"{}"}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                if cfg!(windows) { "powershell" } else { "bash" }
            ),
        )
        .expect("hooks");

        let discovery = discover_runtime_session_hooks(&config);
        let mut conversation = Vec::new();
        let prepared = apply_pre_tool_use_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &ToolCall {
                id: "tool-1".to_owned(),
                name: "bash_command".to_owned(),
                input: json!({"command": "echo original"}),
            },
        )
        .await
        .expect("pre hook should succeed");

        assert_eq!(prepared.call.input["command"], "echo patched");
        assert_eq!(prepared.appended_entries.len(), 1);
        assert!(prepared.blocked_reason.is_none());
        assert!(conversation[0].text.contains("pre context"));
    }

    #[tokio::test]
    async fn pre_tool_hooks_can_deny_execution() {
        let (_tempdir, config, store) = test_config_and_store();
        let command = json_emitting_command(
            r#"{"hook_specific_output":{"hookEventName":"PreToolUse","permission_decision":"deny","permission_decision_reason":"blocked by hook policy"}}"#,
        );
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"PreToolUse":[{{"matcher":"bash_command","hooks":[{{"type":"command","command":"{}","shell":"{}"}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                if cfg!(windows) { "powershell" } else { "bash" }
            ),
        )
        .expect("hooks");

        let discovery = discover_runtime_session_hooks(&config);
        let mut conversation = Vec::new();
        let prepared = apply_pre_tool_use_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &ToolCall {
                id: "tool-2".to_owned(),
                name: "bash_command".to_owned(),
                input: json!({"command": "echo original"}),
            },
        )
        .await
        .expect("pre hook should succeed");

        assert_eq!(
            prepared.blocked_reason.as_deref(),
            Some("blocked by hook policy")
        );
    }

    #[tokio::test]
    async fn post_tool_hooks_can_observe_result_and_rewrite_mcp_output() {
        let (_tempdir, config, store) = test_config_and_store();
        let command = json_emitting_command(
            r#"{"hook_specific_output":{"hookEventName":"PostToolUse","updated_mcp_tool_output":{"value":"patched"},"additional_context":"post context"}}"#,
        );
        fs::write(
            config.paths.profile_dir.join("hooks.json"),
            format!(
                r#"{{"PostToolUse":[{{"matcher":"mcp__mock__resolve-library-id","hooks":[{{"type":"command","command":"{}","shell":"{}"}}]}}]}}"#,
                command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
                if cfg!(windows) { "powershell" } else { "bash" }
            ),
        )
        .expect("hooks");

        let discovery = discover_runtime_session_hooks(&config);
        let mut conversation = Vec::new();
        let mut result = ToolResult {
            content: "raw result".to_owned(),
            is_error: false,
            content_blocks: Vec::new(),
            follow_up_user_blocks: Vec::new(),
        };
        let outcome = apply_post_tool_hooks(
            &discovery,
            &config,
            &store,
            &mut conversation,
            &ToolCall {
                id: "tool-3".to_owned(),
                name: "mcp__mock__resolve-library-id".to_owned(),
                input: json!({"libraryName": "tokio"}),
            },
            &mut result,
        )
        .await
        .expect("post hook should succeed");

        assert!(result.content.contains("patched"));
        assert_eq!(outcome.appended_entries.len(), 1);
        assert!(conversation[0].text.contains("post context"));
    }
}
