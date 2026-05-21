use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use claude_config::{RuntimeConfig, SettingSource, load_hooks_file, load_settings_hooks};
use claude_core::{CommandHook, ConversationEntry, HookEvent, HookShell, ToolCall};
use claude_tools::{CommandHookExecutionRequest, CommandHookExecutionResult, execute_command_hook};
use serde::Serialize;
use serde_json::Value;

const WORKSPACE_SETTINGS_DIR: &str = ".remote-code";
const WORKSPACE_HOOKS_FILE: &str = "hooks.json";
const WORKSPACE_SETTINGS_FILE: &str = "settings.json";
const WORKSPACE_LOCAL_SETTINGS_FILE: &str = "settings.local.json";
const PROFILE_HOOKS_FILE: &str = "hooks.json";
const PROFILE_SETTINGS_FILE: &str = "settings.json";
const LEGACY_IMPORT_SETTINGS_PATH: &[&str] = &["legacy-import", "settings.json"];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookRecord {
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub command: String,
    pub shell: Option<HookShell>,
    pub timeout_secs: Option<u64>,
    pub once: bool,
    pub origin_kind: String,
    pub origin_name: String,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HookInvocationLog {
    pub hook_id: String,
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub origin_kind: String,
    pub origin_name: String,
    pub config_path: PathBuf,
    pub command: String,
    pub shell: Option<HookShell>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionStartHookSummary {
    pub additional_contexts: Vec<String>,
    pub initial_user_message: Option<String>,
    pub warnings: Vec<String>,
    pub logs: Vec<HookInvocationLog>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreToolUseHookSummary {
    pub tool_call: ToolCall,
    pub decision: HookDecision,
    pub reason: Option<String>,
    pub additional_contexts: Vec<String>,
    pub warnings: Vec<String>,
    pub logs: Vec<HookInvocationLog>,
}

impl PreToolUseHookSummary {
    #[cfg(test)]
    fn from_tool_call(tool_call: ToolCall) -> Self {
        Self {
            tool_call,
            decision: HookDecision::Ask,
            reason: None,
            additional_contexts: Vec::new(),
            warnings: Vec::new(),
            logs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PostToolUseHookSummary {
    pub additional_contexts: Vec<String>,
    pub warnings: Vec<String>,
    pub logs: Vec<HookInvocationLog>,
}

#[derive(Debug, Clone)]
struct RuntimeHookEntry {
    id: String,
    event: HookEvent,
    matcher: Option<String>,
    command: CommandHook,
    origin_kind: &'static str,
    origin_name: String,
    config_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HookRuntime {
    entries: Vec<RuntimeHookEntry>,
    warnings: Vec<String>,
    once_fired: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
struct ParsedHookOutput {
    decision: Option<HookDecision>,
    reason: Option<String>,
    updated_input: Option<Value>,
    additional_contexts: Vec<String>,
    initial_user_message: Option<String>,
}

impl HookRuntime {
    pub(crate) fn discover(config: &RuntimeConfig) -> Self {
        let mut runtime = Self::default();
        let mut seen_paths = BTreeSet::new();
        let user_sources_enabled = setting_source_enabled(config, SettingSource::User);
        let project_sources_enabled = setting_source_enabled(config, SettingSource::Project);

        if user_sources_enabled {
            runtime.load_hook_file_source(
                "profile",
                "profile hooks".to_owned(),
                config.paths.profile_dir.join(PROFILE_HOOKS_FILE),
                &mut seen_paths,
            );
        }
        if project_sources_enabled {
            runtime.load_hook_file_source(
                "project",
                "project hooks".to_owned(),
                config
                    .cwd
                    .join(WORKSPACE_SETTINGS_DIR)
                    .join(WORKSPACE_HOOKS_FILE),
                &mut seen_paths,
            );
        }
        for path in &config.settings_files {
            let (origin_kind, origin_name) = classify_settings_source(config, path);
            runtime.load_settings_source(origin_kind, origin_name, path.clone(), &mut seen_paths);
        }

        if user_sources_enabled && config.paths.plugins_dir.exists() {
            match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
                Ok(plugins) => {
                    for plugin in plugins {
                        if let Some(path) = plugin.hooks_config_path() {
                            runtime.load_hook_file_source(
                                "plugin",
                                plugin.manifest.name.clone(),
                                path,
                                &mut seen_paths,
                            );
                        }
                    }
                }
                Err(error) => runtime.warnings.push(format!(
                    "Failed to discover plugins for hook loading: {error}"
                )),
            }
        }

        runtime.entries.sort_by(|left, right| {
            left.event
                .cmp(&right.event)
                .then_with(|| left.origin_kind.cmp(right.origin_kind))
                .then_with(|| left.origin_name.cmp(&right.origin_name))
                .then_with(|| left.config_path.cmp(&right.config_path))
                .then_with(|| left.matcher.cmp(&right.matcher))
                .then_with(|| left.command.command.cmp(&right.command.command))
        });
        runtime
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn list(&self, event_filter: Option<HookEvent>) -> Vec<HookRecord> {
        self.entries
            .iter()
            .filter(|entry| event_filter.is_none_or(|event| entry.event == event))
            .map(|entry| HookRecord {
                event: entry.event,
                matcher: entry.matcher.clone(),
                command: entry.command.command.clone(),
                shell: entry.command.shell,
                timeout_secs: entry.command.timeout,
                once: entry.command.once,
                origin_kind: entry.origin_kind.to_owned(),
                origin_name: entry.origin_name.clone(),
                config_path: entry.config_path.clone(),
            })
            .collect()
    }

    pub(crate) async fn run_session_start(
        &mut self,
        config: &RuntimeConfig,
        source: &str,
    ) -> SessionStartHookSummary {
        let payload = serde_json::json!({
            "hook_event_name": HookEvent::SessionStart.as_str(),
            "source": source,
            "session_id": config.session_id,
            "cwd": config.cwd.display().to_string(),
            "provider": config.provider.name.clone(),
            "model": config.provider.model.clone(),
            "protocol": config.provider.protocol.as_str(),
        });
        let execution = self
            .execute_matching_hooks(
                HookEvent::SessionStart,
                source,
                None,
                payload,
                config.cwd.clone(),
            )
            .await;

        SessionStartHookSummary {
            additional_contexts: execution.additional_contexts,
            initial_user_message: execution.initial_user_message,
            warnings: execution.warnings,
            logs: execution.logs,
        }
    }

    pub(crate) async fn run_pre_tool_use(
        &mut self,
        config: &RuntimeConfig,
        protocol_name: &str,
        mut tool_call: ToolCall,
    ) -> PreToolUseHookSummary {
        let payload = serde_json::json!({
            "hook_event_name": HookEvent::PreToolUse.as_str(),
            "session_id": config.session_id,
            "cwd": config.cwd.display().to_string(),
            "tool_name": protocol_name,
            "tool_input": tool_call.input.clone(),
            "tool_use_id": tool_call.id,
        });
        let execution = self
            .execute_matching_hooks(
                HookEvent::PreToolUse,
                protocol_name,
                Some(&tool_call.input),
                payload,
                config.cwd.clone(),
            )
            .await;

        if let Some(updated_input) = execution.updated_input {
            tool_call.input = updated_input;
        }

        PreToolUseHookSummary {
            tool_call,
            decision: execution.decision.unwrap_or(HookDecision::Ask),
            reason: execution.reason,
            additional_contexts: execution.additional_contexts,
            warnings: execution.warnings,
            logs: execution.logs,
        }
    }

    pub(crate) async fn run_post_tool_use(
        &mut self,
        config: &RuntimeConfig,
        event: HookEvent,
        protocol_name: &str,
        tool_call: &ToolCall,
        tool_response: &Value,
    ) -> PostToolUseHookSummary {
        let payload = serde_json::json!({
            "hook_event_name": event.as_str(),
            "session_id": config.session_id,
            "cwd": config.cwd.display().to_string(),
            "tool_name": protocol_name,
            "tool_input": tool_call.input.clone(),
            "tool_use_id": tool_call.id,
            "response": tool_response,
        });
        let execution = self
            .execute_matching_hooks(
                event,
                protocol_name,
                Some(&tool_call.input),
                payload,
                config.cwd.clone(),
            )
            .await;

        PostToolUseHookSummary {
            additional_contexts: execution.additional_contexts,
            warnings: execution.warnings,
            logs: execution.logs,
        }
    }

    fn load_settings_source(
        &mut self,
        origin_kind: &'static str,
        origin_name: String,
        path: PathBuf,
        seen_paths: &mut BTreeSet<PathBuf>,
    ) {
        if !path.exists() || !seen_paths.insert(path.clone()) {
            return;
        }
        match load_settings_hooks(&path) {
            Ok(config) => self.extend_entries(origin_kind, origin_name, path, config),
            Err(error) => self.warnings.push(format!(
                "Failed to load hooks from settings {}: {error}",
                path.display()
            )),
        }
    }

    fn load_hook_file_source(
        &mut self,
        origin_kind: &'static str,
        origin_name: String,
        path: PathBuf,
        seen_paths: &mut BTreeSet<PathBuf>,
    ) {
        if !path.exists() || !seen_paths.insert(path.clone()) {
            return;
        }
        match load_hooks_file(&path) {
            Ok(config) => self.extend_entries(origin_kind, origin_name, path, config),
            Err(error) => self.warnings.push(format!(
                "Failed to load hook config {}: {error}",
                path.display()
            )),
        }
    }

    fn extend_entries(
        &mut self,
        origin_kind: &'static str,
        origin_name: String,
        path: PathBuf,
        config: std::collections::BTreeMap<HookEvent, Vec<claude_core::HookMatcher>>,
    ) {
        let mut hook_index = self.entries.len();
        for (event, matchers) in config {
            for matcher in matchers {
                for hook in matcher.hooks {
                    let claude_core::HookCommand::Command(command) = hook;
                    let hook_id = format!(
                        "{}:{}:{}:{}",
                        path.display(),
                        event.as_str(),
                        matcher.matcher.as_deref().unwrap_or("*"),
                        hook_index
                    );
                    hook_index += 1;
                    self.entries.push(RuntimeHookEntry {
                        id: hook_id,
                        event,
                        matcher: matcher.matcher.clone(),
                        command,
                        origin_kind,
                        origin_name: origin_name.clone(),
                        config_path: path.clone(),
                    });
                }
            }
        }
    }

    async fn execute_matching_hooks(
        &mut self,
        event: HookEvent,
        subject: &str,
        tool_input: Option<&Value>,
        payload: Value,
        cwd: PathBuf,
    ) -> ParsedHookBatch {
        let mut batch = ParsedHookBatch::default();
        let matching = self
            .entries
            .iter()
            .filter(|entry| entry.event == event)
            .filter(|entry| !self.once_fired.contains(&entry.id))
            .filter(|entry| matcher_matches(entry.matcher.as_deref(), subject))
            .filter(|entry| {
                condition_matches(entry.command.condition.as_deref(), subject, tool_input)
            })
            .cloned()
            .collect::<Vec<_>>();

        for entry in matching {
            match execute_command_hook(&CommandHookExecutionRequest {
                event,
                command: entry.command.command.clone(),
                cwd: cwd.clone(),
                input: payload.clone(),
                shell: entry.command.shell,
                timeout_secs: entry.command.timeout,
            })
            .await
            {
                Ok(result) => {
                    if entry.command.once {
                        self.once_fired.insert(entry.id.clone());
                    }
                    let parsed = parse_hook_output(event, &result);
                    batch.absorb(parsed);
                    batch.logs.push(HookInvocationLog {
                        hook_id: entry.id.clone(),
                        event,
                        matcher: entry.matcher.clone(),
                        origin_kind: entry.origin_kind.to_owned(),
                        origin_name: entry.origin_name.clone(),
                        config_path: entry.config_path.clone(),
                        command: entry.command.command.clone(),
                        shell: Some(result.shell),
                        exit_code: result.exit_code,
                        stdout: result.stdout.trim().to_owned(),
                        stderr: result.stderr.trim().to_owned(),
                        error: None,
                    });
                }
                Err(error) => {
                    batch.warnings.push(format!(
                        "{} hook `{}` failed: {error}",
                        event.as_str(),
                        entry.command.command
                    ));
                    batch.logs.push(HookInvocationLog {
                        hook_id: entry.id.clone(),
                        event,
                        matcher: entry.matcher.clone(),
                        origin_kind: entry.origin_kind.to_owned(),
                        origin_name: entry.origin_name.clone(),
                        config_path: entry.config_path.clone(),
                        command: entry.command.command.clone(),
                        shell: entry.command.shell,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        batch
    }
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn classify_settings_source(config: &RuntimeConfig, path: &Path) -> (&'static str, String) {
    if config
        .cli_settings_files
        .iter()
        .any(|candidate| candidate == path)
    {
        return ("explicit", path.display().to_string());
    }

    let legacy = LEGACY_IMPORT_SETTINGS_PATH
        .iter()
        .fold(config.paths.profiles_dir.clone(), |path, segment| {
            path.join(segment)
        });
    if path == legacy {
        return ("legacy-import", "legacy import settings".to_owned());
    }

    let profile = config.paths.profile_dir.join(PROFILE_SETTINGS_FILE);
    if path == profile {
        return ("profile", "profile settings".to_owned());
    }

    let project = config
        .cwd
        .join(WORKSPACE_SETTINGS_DIR)
        .join(WORKSPACE_SETTINGS_FILE);
    if path == project {
        return ("project", "project settings".to_owned());
    }

    let local = config
        .cwd
        .join(WORKSPACE_SETTINGS_DIR)
        .join(WORKSPACE_LOCAL_SETTINGS_FILE);
    if path == local {
        return ("local", "local settings".to_owned());
    }

    ("settings", path.display().to_string())
}

#[derive(Debug, Clone, Default)]
struct ParsedHookBatch {
    decision: Option<HookDecision>,
    reason: Option<String>,
    updated_input: Option<Value>,
    additional_contexts: Vec<String>,
    initial_user_message: Option<String>,
    warnings: Vec<String>,
    logs: Vec<HookInvocationLog>,
}

impl ParsedHookBatch {
    fn absorb(&mut self, parsed: ParsedHookOutput) {
        if parsed.decision.is_some() {
            self.decision = parsed.decision;
        }
        if parsed.reason.is_some() {
            self.reason = parsed.reason;
        }
        if parsed.updated_input.is_some() {
            self.updated_input = parsed.updated_input;
        }
        if parsed.initial_user_message.is_some() {
            self.initial_user_message = parsed.initial_user_message;
        }
        self.additional_contexts.extend(parsed.additional_contexts);
    }
}

fn parse_hook_output(event: HookEvent, result: &CommandHookExecutionResult) -> ParsedHookOutput {
    let mut output = ParsedHookOutput::default();
    let stdout = result.stdout.trim();
    let stderr = result.stderr.trim();

    if !stdout.is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(stdout) {
            parse_hook_json(event, &value, &mut output);
        } else if matches!(event, HookEvent::SessionStart) {
            output.additional_contexts.push(stdout.to_owned());
        }
    }

    if matches!(result.exit_code, Some(2)) {
        match event {
            HookEvent::PreToolUse => {
                output.decision = Some(HookDecision::Deny);
                if output.reason.is_none() {
                    output.reason = first_non_empty(&[stderr, stdout]).map(ToOwned::to_owned);
                }
            }
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                if let Some(message) = first_non_empty(&[stderr, stdout]) {
                    output
                        .additional_contexts
                        .push(format!("{} hook: {message}", event.as_str()));
                }
            }
            HookEvent::SessionStart => {}
        }
    }

    output
}

fn parse_hook_json(event: HookEvent, value: &Value, output: &mut ParsedHookOutput) {
    let Some(object) = value.as_object() else {
        return;
    };

    if let Some(reason) = object.get("reason").and_then(Value::as_str) {
        output.reason = Some(reason.to_owned());
    }
    if let Some(system_message) = object.get("systemMessage").and_then(Value::as_str) {
        output.additional_contexts.push(system_message.to_owned());
    }
    if let Some(decision) = object.get("decision").and_then(Value::as_str) {
        output.decision = match decision {
            "approve" => Some(HookDecision::Allow),
            "block" => Some(HookDecision::Deny),
            _ => output.decision,
        };
    }

    let Some(hook_specific) = object.get("hookSpecificOutput").and_then(Value::as_object) else {
        return;
    };
    let hook_event_name = hook_specific
        .get("hookEventName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if hook_event_name != event.as_str() {
        return;
    }

    if let Some(permission_decision) = hook_specific
        .get("permissionDecision")
        .and_then(Value::as_str)
    {
        output.decision = match permission_decision {
            "allow" => Some(HookDecision::Allow),
            "deny" => Some(HookDecision::Deny),
            "ask" => Some(HookDecision::Ask),
            _ => output.decision,
        };
    }
    if let Some(permission_reason) = hook_specific
        .get("permissionDecisionReason")
        .and_then(Value::as_str)
    {
        output.reason = Some(permission_reason.to_owned());
    }
    if let Some(updated_input) = hook_specific.get("updatedInput") {
        output.updated_input = Some(updated_input.clone());
    }
    if let Some(initial_user_message) = hook_specific
        .get("initialUserMessage")
        .and_then(Value::as_str)
    {
        output.initial_user_message = Some(initial_user_message.to_owned());
    }
    extend_additional_contexts(
        &mut output.additional_contexts,
        hook_specific.get("additionalContext"),
    );
}

fn extend_additional_contexts(target: &mut Vec<String>, value: Option<&Value>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                target.push(trimmed.to_owned());
            }
        }
        Value::Array(values) => {
            for value in values {
                if let Some(text) = value.as_str() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        target.push(trimmed.to_owned());
                    }
                }
            }
        }
        _ => {}
    }
}

fn matcher_matches(matcher: Option<&str>, subject: &str) -> bool {
    let Some(matcher) = matcher.map(str::trim) else {
        return true;
    };
    if matcher.is_empty() {
        return true;
    }
    matcher
        .split('|')
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .any(|clause| wildcard_match(clause, subject))
}

fn condition_matches(condition: Option<&str>, subject: &str, tool_input: Option<&Value>) -> bool {
    let Some(condition) = condition.map(str::trim) else {
        return true;
    };
    if condition.is_empty() {
        return true;
    }
    let Some(open_paren) = condition.find('(') else {
        return wildcard_match(condition, subject);
    };
    let Some(close_paren) = condition.rfind(')') else {
        return wildcard_match(condition, subject);
    };
    let tool_name = condition[..open_paren].trim();
    if !wildcard_match(tool_name, subject) {
        return false;
    }
    let pattern = condition[(open_paren + 1)..close_paren].trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let candidate = tool_input
        .and_then(|value| value.get("command").and_then(Value::as_str))
        .or_else(|| tool_input.and_then(|value| value.get("path").and_then(Value::as_str)))
        .unwrap_or_default();
    wildcard_match(pattern, candidate)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return parts[0] == value;
    }

    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let mut cursor = 0usize;

    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 && anchored_start {
            if !value[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
            continue;
        }
        let Some(found) = value[cursor..].find(part) else {
            return false;
        };
        cursor += found + part.len();
    }

    if anchored_end {
        if let Some(last) = parts.iter().rev().find(|part| !part.is_empty()) {
            value.ends_with(last)
        } else {
            true
        }
    } else {
        true
    }
}

fn first_non_empty<'a>(values: &'a [&'a str]) -> Option<&'a str> {
    values.iter().find_map(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

pub(crate) fn append_hook_context_entries(
    entries: &mut Vec<ConversationEntry>,
    contexts: &[String],
    prefix: &str,
) {
    for context in contexts {
        entries.push(ConversationEntry::system(format!("{prefix}{context}")));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HookDecision, HookRuntime, append_hook_context_entries, condition_matches, matcher_matches,
        wildcard_match,
    };
    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use claude_core::{ConversationRole, HookEvent};
    use serde_json::json;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn matcher_supports_exact_pipe_and_glob_clauses() {
        assert!(matcher_matches(Some("Bash|Read*"), "ReadFile"));
        assert!(matcher_matches(Some("Session*"), "SessionStart"));
        assert!(!matcher_matches(Some("Edit|Write"), "Bash"));
    }

    #[test]
    fn wildcard_match_handles_multiple_segments() {
        assert!(wildcard_match("git * --short", "git status --short"));
        assert!(wildcard_match("*.rs", "main.rs"));
        assert!(!wildcard_match("git * --short", "cargo test"));
    }

    #[test]
    fn condition_matches_common_bash_rule_shape() {
        assert!(condition_matches(
            Some("Bash(git status *)"),
            "Bash",
            Some(&json!({"command":"git status --short"}))
        ));
        assert!(!condition_matches(
            Some("Bash(git status *)"),
            "Bash",
            Some(&json!({"command":"cargo test"}))
        ));
    }

    #[tokio::test]
    async fn discovery_loads_settings_and_plugin_hook_files() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile = temp.path().join("profile");
        let plugin_root = profile.join("plugins").join("sample");
        std::fs::create_dir_all(cwd.join(".remote-code")).expect("workspace dir should exist");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin"))
            .expect("plugin dir should exist");

        std::fs::write(
            cwd.join(".remote-code").join("settings.json"),
            r#"{
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{"type": "command", "command": "echo project"}]
                        }
                    ]
                }
            }"#,
        )
        .expect("settings write should work");
        std::fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "sample",
                "version": "0.1.0",
                "hooks": "./hooks.json"
            }"#,
        )
        .expect("plugin write should work");
        std::fs::write(
            plugin_root.join("hooks.json"),
            r#"{
                "SessionStart": [
                    {
                        "matcher": "startup",
                        "hooks": [{"type": "command", "command": "echo plugin"}]
                    }
                ]
            }"#,
        )
        .expect("hook file write should work");

        let config = load_runtime_config(
            Some(cwd.clone()),
            Some(profile.clone()),
            Some(Uuid::nil()),
            claude_core::PermissionMode::BypassPermissions,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            1,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("runtime config should load");

        let runtime = HookRuntime::discover(&config);
        let hooks = runtime.list(None);
        assert_eq!(hooks.len(), 2);
        assert!(hooks.iter().any(|hook| hook.event == HookEvent::PreToolUse));
        assert!(hooks.iter().any(|hook| hook.origin_kind == "plugin"));
    }

    #[tokio::test]
    async fn discovery_respects_setting_sources_and_explicit_settings() {
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile = temp.path().join("profile");
        std::fs::create_dir_all(cwd.join(".remote-code")).expect("workspace dir should exist");
        std::fs::create_dir_all(profile.join("plugins")).expect("plugins dir should exist");

        std::fs::write(
            profile.join("hooks.json"),
            r#"{
                "SessionStart": [
                    {
                        "hooks": [{"type": "command", "command": "echo profile"}]
                    }
                ]
            }"#,
        )
        .expect("profile hooks write should work");
        std::fs::write(
            cwd.join(".remote-code").join("hooks.json"),
            r#"{
                "PreToolUse": [
                    {
                        "hooks": [{"type": "command", "command": "echo project"}]
                    }
                ]
            }"#,
        )
        .expect("project hooks write should work");
        std::fs::write(
            cwd.join(".remote-code").join("settings.local.json"),
            r#"{
                "hooks": {
                    "PostToolUse": [
                        {
                            "hooks": [{"type": "command", "command": "echo local"}]
                        }
                    ]
                }
            }"#,
        )
        .expect("local settings write should work");

        let local_only = load_runtime_config(
            Some(cwd.clone()),
            Some(profile.clone()),
            Some(Uuid::nil()),
            claude_core::PermissionMode::BypassPermissions,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            1,
            ProviderOverrides::default(),
            RuntimeOverrides {
                allowed_setting_sources: Some(vec![SettingSource::Local]),
                ..RuntimeOverrides::default()
            },
        )
        .expect("local-only config should load");
        let hooks = HookRuntime::discover(&local_only).list(None);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::PostToolUse);
        assert_eq!(hooks[0].origin_kind, "local");

        let explicit = temp.path().join("explicit-settings.json");
        std::fs::write(
            &explicit,
            r#"{
                "hooks": {
                    "PostToolUseFailure": [
                        {
                            "hooks": [{"type": "command", "command": "echo explicit"}]
                        }
                    ]
                }
            }"#,
        )
        .expect("explicit settings write should work");
        let explicit_config = load_runtime_config(
            Some(cwd),
            Some(profile),
            Some(Uuid::nil()),
            claude_core::PermissionMode::BypassPermissions,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            1,
            ProviderOverrides::default(),
            RuntimeOverrides {
                settings_files: vec![explicit],
                allowed_setting_sources: Some(vec![SettingSource::Local]),
                ..RuntimeOverrides::default()
            },
        )
        .expect("explicit config should load");
        let hooks = HookRuntime::discover(&explicit_config).list(None);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::PostToolUseFailure);
        assert_eq!(hooks[0].origin_kind, "explicit");
    }

    #[test]
    fn append_hook_context_entries_creates_system_messages() {
        let mut entries = Vec::new();
        append_hook_context_entries(&mut entries, &["context".to_owned()], "Hook: ");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, ConversationRole::System);
        assert_eq!(entries[0].text, "Hook: context");
    }

    #[test]
    fn hook_decision_default_path_is_ask() {
        let config = super::PreToolUseHookSummary::from_tool_call(claude_core::ToolCall {
            id: "tool-1".to_owned(),
            name: "bash_command".to_owned(),
            input: json!({}),
        });
        assert_eq!(config.decision, HookDecision::Ask);
    }
}
