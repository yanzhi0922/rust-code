use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use claude_config::RuntimeConfig;
use claude_core::{ConversationEntry, ConversationRole, PermissionMode, SystemMemorySavedMessage};
use claude_permissions::{PermissionBroker, PermissionDecision, PermissionRequest};
use claude_protocol::UsagePayload;
use claude_provider::{ConversationBackend, DiscoveredToolScope};
use claude_query_engine::QuerySource;
use claude_runtime_prompt::{
    RuntimePromptSettings, build_extract_memory_auto_only_prompt,
    build_extract_memory_combined_prompt, format_auto_memory_manifest, runtime_env_defined_falsy,
    runtime_env_truthy, scan_auto_memory_files,
};
use claude_session::SessionStore;
use claude_tools::shell::readonly::{ShellKind, is_read_only_command};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::hooks::HookRunState;
use crate::query_engine_compat::{
    CompatRunOverrides, ForkCacheSafeParams, run_no_persist_forked_query,
};

const EXTRACTION_MAX_TURNS: u32 = 5;
const EXTRACT_MODE_FEATURE: &str = "tengu_passport_quail";
const EXTRACT_NON_INTERACTIVE_FEATURE: &str = "tengu_slate_thimble";
const EXTRACT_THROTTLE_FEATURE: &str = "tengu_bramble_lintel";
const EXTRACT_SKIP_INDEX_FEATURE: &str = "tengu_moth_copse";
const TEAMMEM_FEATURE: &str = "TEAMMEM";
const TEAMMEM_ENABLE_FEATURE: &str = "tengu_herring_clock";
const ENTRYPOINT_NAME: &str = "MEMORY.md";

pub(crate) type AppendSystemMessageFn = Arc<dyn Fn(ConversationEntry) + Send + Sync>;

#[derive(Clone)]
struct PendingExtractionContext {
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: Vec<ConversationEntry>,
    append_system_message: Option<AppendSystemMessageFn>,
    fork_snapshot: Option<ForkCacheSafeParams>,
}

#[derive(Clone, Default)]
struct ExtractMemoriesState {
    last_memory_message_uuid: Option<Uuid>,
    has_logged_gate_failure: bool,
    in_progress: bool,
    turns_since_last_extraction: usize,
    pending_context: Option<PendingExtractionContext>,
}

static EXTRACT_MEMORY_STATE: OnceLock<Mutex<HashMap<Uuid, ExtractMemoriesState>>> = OnceLock::new();
static IN_FLIGHT_EXTRACTIONS: OnceLock<AtomicUsize> = OnceLock::new();

fn extraction_state_map() -> &'static Mutex<HashMap<Uuid, ExtractMemoriesState>> {
    EXTRACT_MEMORY_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn in_flight_extractions() -> &'static AtomicUsize {
    IN_FLIGHT_EXTRACTIONS.get_or_init(|| AtomicUsize::new(0))
}

#[derive(Debug, Clone)]
struct ExtractionRunOutcome {
    memory_paths: Vec<String>,
    team_count: Option<usize>,
    files_written: usize,
    turn_count: u32,
    usage: UsagePayload,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    duration_ms: u64,
}

#[derive(Debug)]
struct ExtractMemoriesPermissionBroker {
    memory_dir: PathBuf,
    team_memory_dir: Option<PathBuf>,
}

impl ExtractMemoriesPermissionBroker {
    fn new(memory_dir: PathBuf, team_memory_dir: Option<PathBuf>) -> Self {
        Self {
            memory_dir,
            team_memory_dir,
        }
    }

    fn is_allowed_write_path(&self, candidate: &Path) -> bool {
        path_within(candidate, &self.memory_dir)
            || self
                .team_memory_dir
                .as_ref()
                .is_some_and(|dir| path_within(candidate, dir))
    }
}

#[async_trait::async_trait]
impl PermissionBroker for ExtractMemoriesPermissionBroker {
    fn mode(&self) -> Option<PermissionMode> {
        Some(PermissionMode::DontAsk)
    }

    async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
        match request.tool_name.as_str() {
            "read_file" | "grep" | "glob" | "repl" => PermissionDecision::allow(),
            "bash_command" => {
                let command = request
                    .tool_input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if is_read_only_command(ShellKind::Bash, command) {
                    PermissionDecision::allow()
                } else {
                    PermissionDecision::deny(
                        "Only read-only shell commands are permitted in this context (ls, find, grep, cat, stat, wc, head, tail, and similar)",
                    )
                }
            }
            "write_file" | "edit_file" | "replace_in_file" => {
                let candidate = request
                    .tool_input
                    .get("path")
                    .or_else(|| request.tool_input.get("file_path"))
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                if candidate
                    .as_deref()
                    .is_some_and(|path| self.is_allowed_write_path(path))
                {
                    PermissionDecision::allow()
                } else {
                    PermissionDecision::deny(
                        "only Read, Grep, Glob, read-only Bash, and Edit/Write within the memory directory are allowed",
                    )
                }
            }
            _ => PermissionDecision::deny(
                "only Read, Grep, Glob, read-only Bash, and Edit/Write within the memory directory are allowed",
            ),
        }
    }
}

pub(crate) fn spawn_extract_memories_after_turn(
    config: &RuntimeConfig,
    _store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    append_system_message: Option<AppendSystemMessageFn>,
    fork_snapshot: Option<ForkCacheSafeParams>,
) {
    let config = config.clone();
    let discovered_tool_scope = discovered_tool_scope.clone();
    let conversation = conversation.to_vec();
    let in_flight = in_flight_extractions();
    in_flight.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        let Ok(store) = SessionStore::open(config.paths.clone()) else {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            return;
        };
        let _ = maybe_extract_memories_after_prompt_inner(
            &config,
            &store,
            backend,
            discovered_tool_scope,
            &conversation,
            append_system_message,
            fork_snapshot,
        )
        .await;
        in_flight.fetch_sub(1, Ordering::SeqCst);
    });
}

async fn maybe_extract_memories_after_prompt_inner(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    append_system_message: Option<AppendSystemMessageFn>,
    fork_snapshot: Option<ForkCacheSafeParams>,
) -> Result<()> {
    if !extract_memories_gate_enabled(config).await? {
        return Ok(());
    }

    let session_id = config.session_id;

    {
        let mut states = extraction_state_map().lock().await;
        let state = states.entry(session_id).or_default();
        if state.in_progress {
            state.pending_context = Some(PendingExtractionContext {
                backend,
                discovered_tool_scope,
                conversation: conversation.to_vec(),
                append_system_message,
                fork_snapshot,
            });
            store.append_named_event(
                config.session_id,
                "tengu_extract_memories_coalesced",
                json!({}),
            )?;
            return Ok(());
        }
        state.in_progress = true;
    }

    let mut current = PendingExtractionContext {
        backend,
        discovered_tool_scope,
        conversation: conversation.to_vec(),
        append_system_message,
        fork_snapshot,
    };
    let mut is_trailing_run = false;
    loop {
        let run_result = run_extract_memories_once(
            config,
            store,
            Arc::clone(&current.backend),
            current.discovered_tool_scope.clone(),
            &current.conversation,
            session_id,
            current.append_system_message.clone(),
            current.fork_snapshot.clone(),
            is_trailing_run,
        )
        .await;
        let should_continue = match run_result {
            Ok(should_continue) => should_continue,
            Err(error) => {
                let _ = store.append_named_event(
                    config.session_id,
                    "tengu_extract_memories_error",
                    json!({ "error": error.to_string() }),
                );
                let mut states = extraction_state_map().lock().await;
                let state = states.entry(session_id).or_default();
                state.in_progress = false;
                state.pending_context = None;
                return Ok(());
            }
        };
        let mut states = extraction_state_map().lock().await;
        let state = states.entry(session_id).or_default();
        if should_continue {
            // This branch is reserved for future fork helpers that request a follow-up.
        }
        if let Some(pending_context) = state.pending_context.take() {
            current = pending_context;
            is_trailing_run = true;
            drop(states);
            continue;
        }
        state.in_progress = false;
        break;
    }

    Ok(())
}

pub(crate) async fn drain_pending_extractions(timeout: Duration) {
    let started = tokio::time::Instant::now();
    loop {
        if in_flight_extractions().load(Ordering::SeqCst) == 0 {
            break;
        }
        if started.elapsed() >= timeout {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_extract_memories_once(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    session_id: Uuid,
    append_system_message: Option<AppendSystemMessageFn>,
    fork_snapshot: Option<ForkCacheSafeParams>,
    is_trailing_run: bool,
) -> Result<bool> {
    let prompt_settings = RuntimePromptSettings::from_config(config);
    let Some(memory_dir) = prompt_settings
        .auto_memory_read_dir
        .clone()
        .or(prompt_settings.auto_memory_permission_dir.clone())
        .map(PathBuf::from)
    else {
        return Ok(false);
    };

    let team_memory_enabled = runtime_feature_gate_enabled(TEAMMEM_FEATURE, false)
        && runtime_feature_gate_enabled(TEAMMEM_ENABLE_FEATURE, false);
    let team_memory_dir = if team_memory_enabled {
        prompt_settings
            .team_memory_read_dir
            .clone()
            .map(PathBuf::from)
    } else {
        None
    };

    let visible_messages = model_visible_messages(conversation);
    let last_memory_message_uuid = {
        let mut states = extraction_state_map().lock().await;
        states
            .entry(session_id)
            .or_default()
            .last_memory_message_uuid
    };
    let new_message_count =
        count_visible_messages_since(&visible_messages, last_memory_message_uuid.as_ref());
    if new_message_count == 0 {
        return Ok(false);
    }

    if has_memory_writes_since(
        conversation,
        last_memory_message_uuid.as_ref(),
        &memory_dir,
        team_memory_dir.as_deref(),
    ) {
        update_last_visible_cursor(session_id, conversation.last().map(|entry| entry.uuid)).await;
        store.append_named_event(
            config.session_id,
            "tengu_extract_memories_skipped_direct_write",
            json!({ "message_count": new_message_count }),
        )?;
        return Ok(false);
    }

    let throttle = runtime_feature_gate_value_usize(EXTRACT_THROTTLE_FEATURE).unwrap_or(1);
    if !is_trailing_run {
        let mut states = extraction_state_map().lock().await;
        let state = states.entry(session_id).or_default();
        state.turns_since_last_extraction += 1;
        if state.turns_since_last_extraction < throttle {
            return Ok(false);
        }
        state.turns_since_last_extraction = 0;
    } else {
        let mut states = extraction_state_map().lock().await;
        states
            .entry(session_id)
            .or_default()
            .turns_since_last_extraction = 0;
    }

    let manifest = format_auto_memory_manifest(&scan_auto_memory_files(&memory_dir));
    let skip_index = runtime_feature_gate_enabled(EXTRACT_SKIP_INDEX_FEATURE, false);
    let extract_prompt = if team_memory_enabled {
        build_extract_memory_combined_prompt(new_message_count, &manifest, skip_index)
    } else {
        build_extract_memory_auto_only_prompt(new_message_count, &manifest, skip_index)
    };

    let extraction = run_extraction_child(
        config,
        store,
        backend,
        discovered_tool_scope,
        conversation,
        &extract_prompt,
        memory_dir.clone(),
        team_memory_dir.clone(),
        fork_snapshot,
    )
    .await?;

    update_last_visible_cursor(session_id, conversation.last().map(|entry| entry.uuid)).await;

    store.append_named_event(
        config.session_id,
        "tengu_extract_memories_extraction",
        json!({
            "input_tokens": extraction.usage.input_tokens,
            "output_tokens": extraction.usage.output_tokens,
            "cache_read_input_tokens": extraction.cache_read_input_tokens,
            "cache_creation_input_tokens": extraction.cache_creation_input_tokens,
            "message_count": new_message_count,
            "turn_count": extraction.turn_count,
            "files_written": extraction.files_written,
            "memories_saved": extraction.memory_paths.len(),
            "team_memories_saved": extraction.team_count.unwrap_or(0),
            "duration_ms": extraction.duration_ms,
        }),
    )?;

    if extraction.memory_paths.is_empty() {
        return Ok(false);
    }

    if let Some(append_system_message) = append_system_message {
        append_system_message(create_memory_saved_message(
            extraction.memory_paths,
            extraction.team_count,
        ));
    }

    Ok(false)
}

fn create_memory_saved_message(
    written_paths: Vec<String>,
    team_count: Option<usize>,
) -> ConversationEntry {
    let payload = SystemMemorySavedMessage {
        id: Uuid::new_v4().to_string(),
        written_paths,
        team_count,
        timestamp: Utc::now(),
    };
    let mut entry = ConversationEntry::system(
        serde_json::to_string(&payload).unwrap_or_else(|_| String::new()),
    );
    entry.name = Some("memory_saved".to_owned());
    entry
}

#[allow(clippy::too_many_arguments)]
async fn run_extraction_child(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    extract_prompt: &str,
    memory_dir: PathBuf,
    team_memory_dir: Option<PathBuf>,
    fork_snapshot: Option<ForkCacheSafeParams>,
) -> Result<ExtractionRunOutcome> {
    let started = std::time::Instant::now();
    let mut child_config = config.clone();
    child_config.max_turns = usize::try_from(EXTRACTION_MAX_TURNS).unwrap_or(usize::MAX);
    let discovery = crate::hooks::discover_runtime_hooks(&child_config, &[]);
    let temp_root = std::env::var_os("CLAUDE_CODE_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("remote-code-extract-memories")
        .join(config.session_id.to_string());
    fs::create_dir_all(&temp_root)?;
    let tool_results_dir = temp_root.join("tool-results");
    let broker: Arc<dyn PermissionBroker> = Arc::new(ExtractMemoriesPermissionBroker::new(
        memory_dir.clone(),
        team_memory_dir.clone(),
    ));
    let mut hook_state = HookRunState::load(store, config.session_id)?;
    let outcome = run_no_persist_forked_query(
        &child_config,
        store,
        backend,
        discovered_tool_scope,
        broker,
        &discovery,
        &mut hook_state,
        fork_snapshot.unwrap_or_else(|| ForkCacheSafeParams::from_conversation(conversation)),
        extract_prompt,
        CompatRunOverrides::default(),
        QuerySource::ExtractMemories,
        Some(EXTRACTION_MAX_TURNS),
        tool_results_dir,
    )
    .await?;

    let written_paths = extract_written_paths(&outcome.messages);
    let memory_paths = written_paths
        .iter()
        .filter(|path| {
            Path::new(path).file_name().and_then(|name| name.to_str()) != Some(ENTRYPOINT_NAME)
        })
        .cloned()
        .collect::<Vec<_>>();
    let team_count = team_memory_dir.as_ref().map(|team_dir| {
        memory_paths
            .iter()
            .filter(|path| path_within(Path::new(path), team_dir))
            .count()
    });

    Ok(ExtractionRunOutcome {
        memory_paths,
        team_count,
        files_written: written_paths.len(),
        turn_count: outcome.num_turns,
        usage: outcome.usage,
        cache_read_input_tokens: outcome.cache_read_input_tokens,
        cache_creation_input_tokens: outcome.cache_creation_input_tokens,
        duration_ms: outcome
            .duration_ms
            .max(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
    })
}

fn extract_written_paths(conversation: &[ConversationEntry]) -> Vec<String> {
    let mut written = Vec::new();
    let mut seen = HashSet::new();

    for entry in conversation {
        if entry.role != ConversationRole::Assistant {
            continue;
        }
        for tool_call in &entry.tool_calls {
            if !matches!(
                tool_call.name.as_str(),
                "write_file" | "edit_file" | "replace_in_file"
            ) {
                continue;
            }
            let Some(path) = tool_call
                .input
                .get("path")
                .or_else(|| tool_call.input.get("file_path"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if seen.insert(path.to_owned()) {
                written.push(path.to_owned());
            }
        }
    }

    written
}

fn model_visible_messages(conversation: &[ConversationEntry]) -> Vec<&ConversationEntry> {
    conversation
        .iter()
        .filter(|entry| {
            matches!(
                entry.role,
                ConversationRole::User | ConversationRole::Assistant
            )
        })
        .collect()
}

fn count_visible_messages_since(
    visible_messages: &[&ConversationEntry],
    since_uuid: Option<&Uuid>,
) -> usize {
    match since_uuid {
        Some(since_uuid) => {
            let mut found_start = false;
            let mut count = 0usize;
            for entry in visible_messages {
                if !found_start {
                    if &entry.uuid == since_uuid {
                        found_start = true;
                    }
                    continue;
                }
                count += 1;
            }
            if found_start {
                count
            } else {
                visible_messages.len()
            }
        }
        None => visible_messages.len(),
    }
}

fn has_memory_writes_since(
    conversation: &[ConversationEntry],
    since_uuid: Option<&Uuid>,
    memory_dir: &Path,
    team_memory_dir: Option<&Path>,
) -> bool {
    let mut found_start = since_uuid.is_none();
    for entry in conversation {
        if !found_start {
            if Some(&entry.uuid) == since_uuid {
                found_start = true;
            }
            continue;
        }
        if entry.role != ConversationRole::Assistant {
            continue;
        }
        for tool_call in &entry.tool_calls {
            if !matches!(
                tool_call.name.as_str(),
                "write_file" | "edit_file" | "replace_in_file"
            ) {
                continue;
            }
            let Some(path) = tool_call
                .input
                .get("path")
                .or_else(|| tool_call.input.get("file_path"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let path = Path::new(path);
            if path_within(path, memory_dir)
                || team_memory_dir.is_some_and(|team_dir| path_within(path, team_dir))
            {
                return true;
            }
        }
    }
    false
}

fn path_within(candidate: &Path, root: &Path) -> bool {
    let candidate = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if root.as_os_str().is_empty() {
        return false;
    }
    candidate == root || candidate.starts_with(root)
}

async fn update_last_visible_cursor(session_id: Uuid, message_uuid: Option<Uuid>) {
    let mut states = extraction_state_map().lock().await;
    states
        .entry(session_id)
        .or_default()
        .last_memory_message_uuid = message_uuid;
}

async fn extract_memories_gate_enabled(config: &RuntimeConfig) -> Result<bool> {
    if !runtime_feature_gate_enabled(EXTRACT_MODE_FEATURE, false) {
        let mut states = extraction_state_map().lock().await;
        let state = states.entry(config.session_id).or_default();
        if !state.has_logged_gate_failure {
            state.has_logged_gate_failure = true;
        }
        return Ok(false);
    }
    if !is_auto_memory_enabled(config)? {
        return Ok(false);
    }
    if runtime_env_truthy("CLAUDE_CODE_REMOTE") || runtime_env_truthy("REMOTE_CODE_REMOTE") {
        return Ok(false);
    }
    if config.print_mode && !runtime_feature_gate_enabled(EXTRACT_NON_INTERACTIVE_FEATURE, false) {
        return Ok(false);
    }
    Ok(true)
}

fn is_auto_memory_enabled(config: &RuntimeConfig) -> Result<bool> {
    if runtime_env_truthy("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
        || runtime_env_truthy("REMOTE_CODE_DISABLE_AUTO_MEMORY")
    {
        return Ok(false);
    }
    if runtime_env_defined_falsy("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
        || runtime_env_defined_falsy("REMOTE_CODE_DISABLE_AUTO_MEMORY")
    {
        return Ok(true);
    }
    if runtime_env_truthy("CLAUDE_CODE_SIMPLE") || runtime_env_truthy("REMOTE_CODE_SIMPLE") {
        return Ok(false);
    }
    if (runtime_env_truthy("CLAUDE_CODE_REMOTE") || runtime_env_truthy("REMOTE_CODE_REMOTE"))
        && std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR").is_none()
    {
        return Ok(false);
    }
    let settings = claude_config::settings_layers::load_runtime_settings(&config.settings_files)?;
    if let Some(enabled) = settings.auto_memory_enabled {
        return Ok(enabled);
    }
    Ok(true)
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
        if claude_runtime_prompt::runtime_env_truthy(&env_name) {
            return true;
        }
        if runtime_env_defined_falsy(&env_name) {
            return false;
        }
    }
    default
}

fn runtime_feature_gate_value_usize(feature_key: &str) -> Option<usize> {
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
    env_names
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
}

#[cfg(test)]
mod tests {
    use super::{
        ExtractMemoriesPermissionBroker, count_visible_messages_since, extract_written_paths,
        has_memory_writes_since,
    };
    use claude_core::{ConversationEntry, ConversationRole, ToolCall};
    use claude_permissions::{PermissionBroker, PermissionRequest};
    use serde_json::json;
    use std::path::PathBuf;
    use uuid::Uuid;

    #[test]
    fn extract_written_paths_accepts_path_and_file_path_and_dedups() {
        let conversation = vec![ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::Assistant,
            text: String::new(),
            history_text: None,
            content_blocks: Vec::new(),
            tool_calls: vec![
                ToolCall {
                    id: "1".to_owned(),
                    name: "write_file".to_owned(),
                    input: json!({ "path": "C:/mem/user_role.md" }),
                },
                ToolCall {
                    id: "2".to_owned(),
                    name: "edit_file".to_owned(),
                    input: json!({ "file_path": "C:/mem/user_role.md" }),
                },
                ToolCall {
                    id: "3".to_owned(),
                    name: "replace_in_file".to_owned(),
                    input: json!({ "file_path": "C:/mem/project.md" }),
                },
            ],
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        }];

        assert_eq!(
            extract_written_paths(&conversation),
            vec![
                "C:/mem/user_role.md".to_owned(),
                "C:/mem/project.md".to_owned()
            ]
        );
    }

    #[test]
    fn has_memory_writes_since_detects_direct_memory_write() {
        let conversation = vec![
            ConversationEntry::user("hello"),
            ConversationEntry {
                uuid: uuid::Uuid::new_v4(),
                role: ConversationRole::Assistant,
                text: String::new(),
                history_text: None,
                content_blocks: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: "1".to_owned(),
                    name: "write_file".to_owned(),
                    input: json!({ "path": "C:/mem/user_role.md" }),
                }],
                attachments: Vec::new(),
                tool_call_id: None,
                name: None,
                is_error: false,
            },
        ];

        assert!(has_memory_writes_since(
            &conversation,
            Some(&conversation[0].uuid),
            &PathBuf::from("C:/mem"),
            None,
        ));
    }

    #[test]
    fn count_visible_messages_since_falls_back_to_all_when_cursor_missing() {
        let user = ConversationEntry::user("u");
        let assistant = ConversationEntry::assistant("a");
        let visible = vec![&user, &assistant];

        assert_eq!(
            count_visible_messages_since(&visible, Some(&Uuid::new_v4())),
            2
        );
        assert_eq!(count_visible_messages_since(&visible, None), 2);
        assert_eq!(count_visible_messages_since(&visible, Some(&user.uuid)), 1);
    }

    #[tokio::test]
    async fn broker_allows_read_only_bash_and_denies_write_outside_memory() {
        let broker = ExtractMemoriesPermissionBroker::new(PathBuf::from("C:/mem"), None);

        let allow = broker
            .decide(PermissionRequest {
                tool_name: "bash_command".to_owned(),
                permission_class: None,
                tool_input: json!({ "command": "git status" }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(allow.allowed);

        let deny = broker
            .decide(PermissionRequest {
                tool_name: "write_file".to_owned(),
                permission_class: None,
                tool_input: json!({ "path": "C:/other/outside.md" }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(!deny.allowed);
    }
}
