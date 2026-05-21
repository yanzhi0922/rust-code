use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use claude_config::{
    ActiveWorktreeSession, RuntimeConfig, settings_layers::resolve_runtime_settings_files,
};
use claude_core::{ConversationEntry, ConversationRole, PermissionMode, ProviderProtocol};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::SessionStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProviderContext {
    name: String,
    base_url: Option<String>,
    model: Option<String>,
    protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSessionContext {
    cwd: PathBuf,
    #[serde(default)]
    original_cwd: Option<PathBuf>,
    #[serde(default)]
    active_worktree_session: Option<PersistedActiveWorktreeSession>,
    permission_mode: String,
    #[serde(default)]
    output_style: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    brief_enabled: bool,
    #[serde(default)]
    proactive_active: bool,
    provider: PersistedProviderContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedActiveWorktreeSession {
    original_cwd: PathBuf,
    worktree_path: PathBuf,
    worktree_name: String,
    #[serde(default)]
    worktree_branch: Option<String>,
    #[serde(default)]
    original_branch: Option<String>,
    #[serde(default)]
    original_head_commit: Option<String>,
    session_id: Uuid,
    #[serde(default)]
    tmux_session_name: Option<String>,
    #[serde(default)]
    hook_based: bool,
}

impl From<&ActiveWorktreeSession> for PersistedActiveWorktreeSession {
    fn from(value: &ActiveWorktreeSession) -> Self {
        Self {
            original_cwd: value.original_cwd.clone(),
            worktree_path: value.worktree_path.clone(),
            worktree_name: value.worktree_name.clone(),
            worktree_branch: value.worktree_branch.clone(),
            original_branch: value.original_branch.clone(),
            original_head_commit: value.original_head_commit.clone(),
            session_id: value.session_id,
            tmux_session_name: value.tmux_session_name.clone(),
            hook_based: value.hook_based,
        }
    }
}

impl From<PersistedActiveWorktreeSession> for ActiveWorktreeSession {
    fn from(value: PersistedActiveWorktreeSession) -> Self {
        Self {
            original_cwd: value.original_cwd,
            worktree_path: value.worktree_path,
            worktree_name: value.worktree_name,
            worktree_branch: value.worktree_branch,
            original_branch: value.original_branch,
            original_head_commit: value.original_head_commit,
            session_id: value.session_id,
            tmux_session_name: value.tmux_session_name,
            hook_based: value.hook_based,
        }
    }
}

/// Persist the runtime fields needed to restore a session consistently across
/// CLI, TUI, and resumed/forked sessions.
///
/// # Errors
/// Returns an error if the context cannot be serialized or written.
pub fn persist_runtime_config_session_context(
    store: &SessionStore,
    config: &RuntimeConfig,
) -> Result<()> {
    store.ensure_session(
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        config.session_name.as_deref(),
    )?;
    store.append_named_event(
        config.session_id,
        "session_context",
        serde_json::to_value(PersistedSessionContext {
            cwd: config.cwd.clone(),
            original_cwd: Some(config.original_cwd.clone()),
            active_worktree_session: config
                .active_worktree_session
                .as_ref()
                .map(PersistedActiveWorktreeSession::from),
            permission_mode: config.permission_mode.as_legacy_str().to_owned(),
            output_style: config.output_style.clone(),
            language: config.language.clone(),
            brief_enabled: config.brief_enabled,
            proactive_active: config.proactive_active,
            provider: PersistedProviderContext {
                name: config.provider.name.clone(),
                base_url: config.provider.base_url.clone(),
                model: config.provider.model.clone(),
                protocol: config.provider.protocol.as_str().to_owned(),
            },
        })?,
    )
}

/// Restore persisted runtime fields from session summary and transcript into a
/// [`RuntimeConfig`].
///
/// # Errors
/// Returns an error if the transcript cannot be read or deserialized.
pub fn restore_runtime_config_session_context(
    store: &SessionStore,
    config: &mut RuntimeConfig,
) -> Result<()> {
    if let Ok(summary) = store.get_session_summary(config.session_id) {
        config.session_name = Some(summary.title.clone());
        config.cwd = summary.cwd;
        config.original_cwd = config.cwd.clone();
        config.active_worktree_session = None;
        config.provider.name = summary.provider_name;
        if let Some(model) = summary.model {
            config.provider.model = Some(model);
        }
    }

    let Ok(transcript) = store.load_transcript(config.session_id) else {
        config.original_cwd = config.cwd.clone();
        return Ok(());
    };

    if let Some(persisted) =
        transcript.latest_named_event_as::<PersistedSessionContext>("session_context")?
    {
        config.cwd = persisted.cwd;
        config.original_cwd = persisted.original_cwd.unwrap_or_else(|| config.cwd.clone());
        config.active_worktree_session = persisted.active_worktree_session.map(Into::into);
        if let Some(permission_mode) = parse_permission_mode(&persisted.permission_mode) {
            config.permission_mode = permission_mode;
        }
        config.output_style = persisted.output_style;
        config.language = persisted.language;
        config.brief_enabled = persisted.brief_enabled;
        config.proactive_active = persisted.proactive_active;
        config.provider.name = persisted.provider.name;
        config.provider.base_url = persisted.provider.base_url;
        config.provider.model = persisted.provider.model;
        if let Some(protocol) = parse_provider_protocol(&persisted.provider.protocol) {
            config.provider.protocol = protocol;
        }
    }

    if let Some(plan_mode_state) = transcript.latest_plan_mode_state()? {
        config.permission_mode = plan_mode_state.current_permission_mode;
    }

    config.settings_files = resolve_runtime_settings_files(
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

    Ok(())
}

/// Materialize synthetic error tool results for any interrupted tool batch that
/// has pending calls persisted in resume state but no corresponding tool
/// results in the transcript.
///
/// # Errors
/// Returns an error if transcript state cannot be loaded or the synthetic
/// results cannot be persisted.
pub fn repair_interrupted_tool_batch(
    store: &SessionStore,
    session_id: Uuid,
    conversation: &mut Vec<ConversationEntry>,
) -> Result<()> {
    let Some(resume_state) = store.load_resume_state(session_id)? else {
        return Ok(());
    };
    if resume_state.pending_tool_calls.is_empty() {
        return Ok(());
    }

    let existing_tool_results = conversation
        .iter()
        .filter(|entry| entry.role == ConversationRole::Tool)
        .filter_map(|entry| entry.tool_call_id.as_deref())
        .collect::<HashSet<_>>();

    let interrupted_calls = resume_state
        .pending_tool_calls
        .into_iter()
        .filter(|tool_call| !existing_tool_results.contains(tool_call.id.as_str()))
        .collect::<Vec<_>>();

    if interrupted_calls.is_empty() {
        store.clear_resume_state(session_id)?;
        return Ok(());
    }

    for tool_call in &interrupted_calls {
        let content = format!(
            "Tool execution for `{}` was interrupted before the result was recorded. Retry the tool if you still need it.",
            tool_call.name
        );
        let tool_entry = ConversationEntry::tool(
            tool_call.id.clone(),
            tool_call.name.clone(),
            content.clone(),
            true,
        );
        store.append_conversation_entry(session_id, &tool_entry)?;
        store.append_named_event(
            session_id,
            "tool_result",
            serde_json::json!({
                "tool_name": tool_call.name,
                "tool_use_id": tool_call.id,
                "is_error": true,
                "content_preview": truncate_preview(&content, 160),
                "synthetic": true,
            }),
        )?;
        conversation.push(tool_entry);
    }

    store.append_named_event(
        session_id,
        "resume_repair",
        serde_json::json!({
            "repaired_tool_calls": interrupted_calls.len(),
        }),
    )?;
    store.clear_resume_state(session_id)?;
    Ok(())
}

fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value.trim() {
        "default" => Some(PermissionMode::Default),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" => Some(PermissionMode::DontAsk),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

fn parse_provider_protocol(value: &str) -> Option<ProviderProtocol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai" | "open-ai" => Some(ProviderProtocol::OpenAi),
        "anthropic" => Some(ProviderProtocol::Anthropic),
        "bedrock" => Some(ProviderProtocol::Bedrock),
        "vertex" => Some(ProviderProtocol::Vertex),
        _ => None,
    }
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let truncated = value.chars().take(max_chars).collect::<String>();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{ConversationRole, InputFormat, OutputFormat, PermissionMode, ToolCall};
    use tempfile::tempdir;

    use super::*;
    use crate::resume_state::{PendingToolCall, ResumeState};

    fn test_config() -> (tempfile::TempDir, RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir should succeed");
        let config = load_runtime_config(
            Some(tempdir.path().to_path_buf()),
            Some(tempdir.path().join(".remote-code-rust")),
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
                provider: Some("mock-provider".to_owned()),
                base_url: Some("https://example.invalid/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");
        (tempdir, config)
    }

    #[test]
    fn persisted_runtime_context_round_trips() {
        let (_tempdir, mut config) = test_config();
        let store = SessionStore::open(config.paths.clone()).expect("store should open");
        let expected_cwd = config.cwd.clone();
        let expected_base_url = config.provider.base_url.clone();
        persist_runtime_config_session_context(&store, &config).expect("context should persist");

        config.cwd = PathBuf::from("C:\\different");
        config.original_cwd = PathBuf::from("C:\\different-origin");
        config.permission_mode = PermissionMode::Plan;
        config.provider.name = "different".to_owned();
        config.provider.base_url = Some("https://different.invalid".to_owned());
        config.provider.model = Some("different-model".to_owned());
        config.provider.protocol = ProviderProtocol::OpenAi;

        restore_runtime_config_session_context(&store, &mut config)
            .expect("context should restore");

        assert_eq!(config.cwd, expected_cwd);
        assert_eq!(config.original_cwd, expected_cwd);
        assert_eq!(config.permission_mode, PermissionMode::Default);
        assert_eq!(config.output_style, None);
        assert_eq!(config.language, None);
        assert!(!config.brief_enabled);
        assert!(!config.proactive_active);
        assert_eq!(config.provider.name, "mock-provider");
        assert_eq!(config.provider.base_url, expected_base_url);
        assert_eq!(config.provider.model.as_deref(), Some("mock-model"));
        assert_eq!(config.provider.protocol, ProviderProtocol::Anthropic);
    }

    #[test]
    fn persisted_runtime_context_restores_prompt_and_mode_state() {
        let (_tempdir, mut config) = test_config();
        config.output_style = Some("Explanatory".to_owned());
        config.language = Some("Japanese".to_owned());
        config.brief_enabled = true;
        config.proactive_active = true;
        let store = SessionStore::open(config.paths.clone()).expect("store should open");

        persist_runtime_config_session_context(&store, &config).expect("context should persist");

        config.output_style = None;
        config.language = None;
        config.brief_enabled = false;
        config.proactive_active = false;

        restore_runtime_config_session_context(&store, &mut config)
            .expect("context should restore");

        assert_eq!(config.output_style.as_deref(), Some("Explanatory"));
        assert_eq!(config.language.as_deref(), Some("Japanese"));
        assert!(config.brief_enabled);
        assert!(config.proactive_active);
    }

    #[test]
    fn interrupted_tool_batches_are_repaired_once() {
        let (_tempdir, config) = test_config();
        let store = SessionStore::open(config.paths.clone()).expect("store should open");

        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("repair"),
            )
            .expect("session should exist");

        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls.push(ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: serde_json::json!({"path": "src/lib.rs"}),
        });
        store
            .append_conversation_entry(config.session_id, &assistant)
            .expect("assistant should append");
        store
            .save_resume_state(
                config.session_id,
                &ResumeState::from_pending_calls(vec![PendingToolCall {
                    id: "call-1".to_owned(),
                    name: "replace_in_file".to_owned(),
                    input: serde_json::json!({"path": "src/lib.rs"}),
                }]),
            )
            .expect("resume state should save");

        let mut conversation = store
            .load_conversation(config.session_id)
            .expect("conversation should load");
        repair_interrupted_tool_batch(&store, config.session_id, &mut conversation)
            .expect("repair should succeed");

        let repaired = conversation
            .iter()
            .find(|entry| entry.role == ConversationRole::Tool)
            .expect("synthetic tool result should exist");
        assert_eq!(repaired.tool_call_id.as_deref(), Some("call-1"));
        assert!(repaired.is_error);
        assert!(repaired.text.contains("interrupted"));

        let cleared = store
            .load_resume_state(config.session_id)
            .expect("resume state should load")
            .expect("resume state should exist");
        assert!(cleared.pending_tool_calls.is_empty());

        let previous_len = conversation.len();
        repair_interrupted_tool_batch(&store, config.session_id, &mut conversation)
            .expect("idempotent repair should succeed");
        assert_eq!(conversation.len(), previous_len);
    }
}
