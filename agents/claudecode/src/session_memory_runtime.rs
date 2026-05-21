use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use claude_compact::session_memory::SessionMemoryCompactFileContext;
use claude_compact::{
    SessionMemoryCompactConfig, build_post_compact_messages, session_memory_compact,
};
use claude_config::RuntimeConfig;
use claude_core::{ConversationEntry, ConversationRole, PermissionMode};
use claude_permissions::{PermissionBroker, PermissionDecision, PermissionRequest};
use claude_provider::context::TokenEstimator;
use claude_provider::{ConversationBackend, DiscoveredToolScope};
use claude_query_engine::QuerySource;
use claude_runtime_prompt::{runtime_env_defined_falsy, runtime_env_truthy};
use claude_session::SessionStore;
use claude_session::session_memory::{
    SessionMemoryConfig, SessionMemoryState, build_session_memory_update_prompt,
    ensure_session_memory_file,
};
use claude_telemetry::growthbook::{FeatureGate, FeatureValue, GrowthBookClient};
use claude_tools::ToolExecutionContext;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::query_engine_compat::{
    CompatRunOverrides, ForkCacheSafeParams, run_no_persist_forked_query,
};

static SESSION_MEMORY_STATES: OnceLock<
    Mutex<HashMap<Uuid, Arc<parking_lot::Mutex<SessionMemoryRuntimeState>>>>,
> = OnceLock::new();
static SESSION_MEMORY_GROWTHBOOK: OnceLock<GrowthBookClient> = OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct SessionMemoryRuntimeState {
    pub(crate) shared: Arc<parking_lot::Mutex<SessionMemoryState>>,
    pub(crate) last_memory_message_id: Option<String>,
}

impl Default for SessionMemoryRuntimeState {
    fn default() -> Self {
        Self {
            shared: Arc::new(parking_lot::Mutex::new(SessionMemoryState::default())),
            last_memory_message_id: None,
        }
    }
}

pub(crate) async fn session_memory_shared_state_for_session(
    session_id: Uuid,
) -> Arc<parking_lot::Mutex<SessionMemoryState>> {
    session_memory_state_for_session(session_id)
        .await
        .lock()
        .shared
        .clone()
}

fn session_memory_states()
-> &'static Mutex<HashMap<Uuid, Arc<parking_lot::Mutex<SessionMemoryRuntimeState>>>> {
    SESSION_MEMORY_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn growthbook_client() -> &'static GrowthBookClient {
    SESSION_MEMORY_GROWTHBOOK.get_or_init(GrowthBookClient::with_defaults)
}

pub(crate) async fn session_memory_state_for_session(
    session_id: Uuid,
) -> Arc<parking_lot::Mutex<SessionMemoryRuntimeState>> {
    let mut states = session_memory_states().lock().await;
    states
        .entry(session_id)
        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(SessionMemoryRuntimeState::default())))
        .clone()
}

#[derive(Clone, Debug)]
struct SessionMemoryPermissionBroker {
    summary_path: PathBuf,
}

impl SessionMemoryPermissionBroker {
    fn new(summary_path: PathBuf) -> Self {
        Self { summary_path }
    }

    fn is_exact_summary_path(&self, candidate: &Path) -> bool {
        let candidate = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        let summary_path = self
            .summary_path
            .canonicalize()
            .unwrap_or_else(|_| self.summary_path.clone());
        candidate == summary_path
    }
}

#[async_trait::async_trait]
impl PermissionBroker for SessionMemoryPermissionBroker {
    fn mode(&self) -> Option<PermissionMode> {
        Some(PermissionMode::DontAsk)
    }

    async fn decide(&self, request: PermissionRequest) -> PermissionDecision {
        match request.tool_name.as_str() {
            "edit_file" => {
                let candidate = request
                    .tool_input
                    .get("path")
                    .or_else(|| request.tool_input.get("file_path"))
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                if candidate
                    .as_deref()
                    .is_some_and(|path| self.is_exact_summary_path(path))
                {
                    PermissionDecision::allow()
                } else {
                    PermissionDecision::deny(format!(
                        "only Edit on {} is allowed",
                        self.summary_path.display()
                    ))
                }
            }
            _ => PermissionDecision::deny(format!(
                "only Edit on {} is allowed",
                self.summary_path.display()
            )),
        }
    }
}

fn is_auto_compact_enabled(config: &RuntimeConfig) -> Result<bool> {
    if runtime_env_truthy("DISABLE_COMPACT")
        || runtime_env_truthy("CLAUDE_CODE_DISABLE_COMPACT")
        || runtime_env_truthy("REMOTE_CODE_DISABLE_COMPACT")
        || runtime_env_truthy("DISABLE_AUTO_COMPACT")
        || runtime_env_truthy("CLAUDE_CODE_DISABLE_AUTO_COMPACT")
        || runtime_env_truthy("REMOTE_CODE_DISABLE_AUTO_COMPACT")
    {
        return Ok(false);
    }

    let settings = claude_config::settings_layers::load_runtime_settings(&config.settings_files)?;
    Ok(settings.auto_compact_enabled.unwrap_or(true))
}

fn session_memory_gate_enabled(config: &RuntimeConfig) -> Result<bool> {
    if runtime_env_truthy("CLAUDE_CODE_FEATURE_TENGU_SESSION_MEMORY")
        || runtime_env_truthy("REMOTE_CODE_FEATURE_TENGU_SESSION_MEMORY")
        || runtime_env_truthy("TENGU_SESSION_MEMORY")
    {
        return Ok(true);
    }
    if runtime_env_defined_falsy("CLAUDE_CODE_FEATURE_TENGU_SESSION_MEMORY")
        || runtime_env_defined_falsy("REMOTE_CODE_FEATURE_TENGU_SESSION_MEMORY")
        || runtime_env_defined_falsy("TENGU_SESSION_MEMORY")
    {
        return Ok(false);
    }
    if !is_auto_compact_enabled(config)? {
        return Ok(false);
    }
    if runtime_env_truthy("CLAUDE_CODE_REMOTE") || runtime_env_truthy("REMOTE_CODE_REMOTE") {
        return Ok(false);
    }
    Ok(growthbook_client().is_gate_enabled(FeatureGate::SessionMemory))
}

fn session_memory_dynamic_config() -> SessionMemoryConfig {
    let default = SessionMemoryConfig::default();
    let feature_value = growthbook_client()
        .get_all_features()
        .get("tengu_sm_config")
        .cloned();
    let Some(FeatureValue::Json(Value::Object(object))) = feature_value else {
        return default;
    };

    fn positive_u64(value: Option<&Value>) -> Option<u64> {
        value
            .and_then(Value::as_u64)
            .filter(|candidate| *candidate > 0)
    }

    SessionMemoryConfig {
        minimum_message_tokens_to_init: positive_u64(object.get("minimumMessageTokensToInit"))
            .unwrap_or(default.minimum_message_tokens_to_init),
        minimum_tokens_between_update: positive_u64(object.get("minimumTokensBetweenUpdate"))
            .unwrap_or(default.minimum_tokens_between_update),
        tool_calls_between_updates: positive_u64(object.get("toolCallsBetweenUpdates"))
            .unwrap_or(default.tool_calls_between_updates),
    }
}

fn init_session_memory_config_if_needed(state: &mut SessionMemoryRuntimeState) {
    let mut shared = state.shared.lock();
    if shared.initialized {
        return;
    }
    shared.config = session_memory_dynamic_config();
}

fn has_tool_calls_in_last_assistant_turn(conversation: &[ConversationEntry]) -> bool {
    conversation
        .iter()
        .rev()
        .find(|entry| entry.role == ConversationRole::Assistant)
        .is_some_and(|entry| !entry.tool_calls.is_empty())
}

fn count_tool_calls_since(conversation: &[ConversationEntry], since_uuid: Option<Uuid>) -> u64 {
    let mut count = 0u64;
    let mut found_start = since_uuid.is_none();

    for entry in conversation {
        if !found_start {
            if Some(entry.uuid) == since_uuid {
                found_start = true;
            }
            continue;
        }

        if entry.role == ConversationRole::Assistant {
            count += u64::try_from(entry.tool_calls.len()).unwrap_or(u64::MAX);
        }
    }

    count
}

fn update_last_memory_message_id(
    conversation: &[ConversationEntry],
    state: &mut SessionMemoryRuntimeState,
) {
    if let Some(last_message) = conversation.last() {
        state.last_memory_message_id = Some(last_message.uuid.to_string());
    }
}

fn count_conversation_tokens(conversation: &[ConversationEntry]) -> u64 {
    let estimator = TokenEstimator::new();
    conversation
        .iter()
        .map(|entry| {
            let text_tokens = estimator.estimate(&entry.text);
            let tool_tokens = entry
                .tool_calls
                .iter()
                .map(|tool_call| {
                    estimator
                        .estimate(&tool_call.name)
                        .saturating_add(estimator.estimate(&tool_call.input.to_string()))
                })
                .sum::<u64>();
            text_tokens.saturating_add(tool_tokens)
        })
        .sum()
}

fn should_extract_memory(
    conversation: &[ConversationEntry],
    state: &mut SessionMemoryRuntimeState,
) -> bool {
    let current_token_count = count_conversation_tokens(conversation);
    let mut shared = state.shared.lock();

    if !shared.initialized {
        if !shared.has_met_initialization_threshold(current_token_count) {
            return false;
        }
        shared.mark_initialized();
    }

    let has_met_token_threshold = shared.has_met_update_threshold(current_token_count);
    let last_memory_uuid = state
        .last_memory_message_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok());
    let has_met_tool_call_threshold = count_tool_calls_since(conversation, last_memory_uuid)
        >= shared.config.tool_calls_between_updates;
    let has_tool_calls_in_last_turn = has_tool_calls_in_last_assistant_turn(conversation);

    let should_extract =
        has_met_token_threshold && (has_met_tool_call_threshold || !has_tool_calls_in_last_turn);

    drop(shared);
    if should_extract {
        update_last_memory_message_id(conversation, state);
    }

    should_extract
}

fn update_last_summarized_message_id_if_safe(
    conversation: &[ConversationEntry],
    state: &mut SessionMemoryRuntimeState,
) {
    if has_tool_calls_in_last_assistant_turn(conversation) {
        return;
    }
    if let Some(last_message) = conversation.last() {
        state
            .shared
            .lock()
            .set_last_summarized_message_id(Some(last_message.uuid.to_string()));
    }
}

pub(crate) async fn maybe_spawn_session_memory_update(
    config: &RuntimeConfig,
    _store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    fork_snapshot: Option<ForkCacheSafeParams>,
) {
    let gate_enabled = session_memory_gate_enabled(config).unwrap_or_default();
    if !gate_enabled {
        return;
    }

    let state = session_memory_state_for_session(config.session_id).await;
    let mut guard = state.lock();
    init_session_memory_config_if_needed(&mut guard);
    if !should_extract_memory(conversation, &mut guard) {
        return;
    }
    guard.shared.lock().mark_extraction_started();
    drop(guard);

    let child_config = config.clone();
    let conversation = conversation.to_vec();
    let discovered_tool_scope = discovered_tool_scope.clone();
    let paths = config.paths.clone();
    let state = state.clone();
    let backend = backend.clone();
    let fork_snapshot =
        fork_snapshot.unwrap_or_else(|| ForkCacheSafeParams::from_conversation(&conversation));
    tokio::spawn(async move {
        let store = match SessionStore::open(paths) {
            Ok(store) => store,
            Err(_) => {
                let shared = state.lock().shared.clone();
                shared.lock().mark_extraction_completed();
                return;
            }
        };
        let _ = run_session_memory_update(
            &child_config,
            &store,
            backend,
            discovered_tool_scope,
            &conversation,
            fork_snapshot,
            state,
        )
        .await;
    });
}

pub(crate) async fn try_session_memory_compaction(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    context_manager: &claude_provider::context::ContextWindowManager,
) -> Option<Vec<ConversationEntry>> {
    let gate_enabled = session_memory_gate_enabled(config).ok()?;
    try_session_memory_compaction_with_gate(config, conversation, context_manager, gate_enabled)
        .await
}

async fn try_session_memory_compaction_with_gate(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    context_manager: &claude_provider::context::ContextWindowManager,
    gate_enabled: bool,
) -> Option<Vec<ConversationEntry>> {
    if !gate_enabled {
        return None;
    }

    let shared_state = session_memory_shared_state_for_session(config.session_id).await;
    {
        let mut shared = shared_state.lock();
        if !shared.initialized {
            shared.config = session_memory_dynamic_config();
        }
    }

    let messages = conversation
        .iter()
        .cloned()
        .map(claude_core::Message::from)
        .collect::<Vec<_>>();
    let threshold = context_manager
        .budget_snapshot(conversation)
        .threshold_tokens();
    let compact_config = SessionMemoryCompactConfig::default();
    let file_context = SessionMemoryCompactFileContext {
        runtime_config: config.clone(),
        state: shared_state.clone(),
    };
    let result = session_memory_compact(&messages, &compact_config, Some(&file_context), None)
        .await
        .ok()?;
    let built = build_post_compact_messages(&result);
    let compacted = built
        .into_iter()
        .filter_map(|message| message.as_conversation_entry())
        .collect::<Vec<_>>();

    if context_manager.budget_snapshot(&compacted).estimated_tokens >= threshold {
        return None;
    }

    shared_state.lock().set_last_summarized_message_id(None);

    Some(compacted)
}

struct SessionMemoryFileSetup {
    summary_path: PathBuf,
    current_memory: String,
    read_file_state: claude_tools::FileStateCache,
}

fn parse_numbered_read_file_output(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            let Some(prefix) = line.as_bytes().get(0..4) else {
                return line;
            };
            if prefix.iter().all(u8::is_ascii_digit) && line.as_bytes().get(4) == Some(&b' ') {
                line.get(5..).unwrap_or_default()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn setup_session_memory_file(
    config: &RuntimeConfig,
    store: &SessionStore,
) -> Result<SessionMemoryFileSetup> {
    let summary_path = ensure_session_memory_file(config)?;
    let setup_context = ToolExecutionContext::from_runtime_config(config);
    setup_context.read_file_state.delete(&summary_path);
    let current_memory = claude_tools::file_ops::read_file(
        &json!({
            "file_path": summary_path.to_string_lossy().to_string()
        }),
        &setup_context,
    )
    .map(|output| parse_numbered_read_file_output(&output.content))?;
    store.append_named_event(
        config.session_id,
        "tengu_session_memory_file_read",
        json!({
            "content_length": current_memory.len(),
        }),
    )?;
    Ok(SessionMemoryFileSetup {
        summary_path,
        current_memory,
        read_file_state: setup_context.read_file_state.clone_isolated(),
    })
}

fn session_memory_tool_results_dir(config: &RuntimeConfig) -> PathBuf {
    std::env::var_os("CLAUDE_CODE_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("remote-code-session-memory")
        .join(config.session_id.to_string())
}

fn build_session_memory_extraction_prompt(
    config: &RuntimeConfig,
    current_memory: &str,
    summary_path: &Path,
) -> String {
    build_session_memory_update_prompt(config, current_memory, summary_path)
}

async fn run_session_memory_update(
    config: &RuntimeConfig,
    store: &SessionStore,
    backend: Arc<dyn ConversationBackend>,
    discovered_tool_scope: DiscoveredToolScope,
    conversation: &[ConversationEntry],
    fork_snapshot: ForkCacheSafeParams,
    state: Arc<parking_lot::Mutex<SessionMemoryRuntimeState>>,
) -> Result<()> {
    let file_setup = setup_session_memory_file(config, store)?;
    let prompt = build_session_memory_extraction_prompt(
        config,
        &file_setup.current_memory,
        &file_setup.summary_path,
    );
    let broker: Arc<dyn PermissionBroker> = Arc::new(SessionMemoryPermissionBroker::new(
        file_setup.summary_path.clone(),
    ));
    let discovery = crate::hooks::discover_runtime_hooks(config, &[]);
    let mut hook_state = crate::hooks::HookRunState::load(store, config.session_id)?;

    let run_result = run_no_persist_forked_query(
        config,
        store,
        backend,
        discovered_tool_scope,
        broker,
        &discovery,
        &mut hook_state,
        fork_snapshot.with_read_file_state(file_setup.read_file_state.clone()),
        &prompt,
        CompatRunOverrides {
            allowed_tools: Some(vec!["Edit".to_owned()]),
            ..CompatRunOverrides::default()
        },
        QuerySource::SessionMemory,
        None,
        session_memory_tool_results_dir(config),
    )
    .await;

    let outcome = match run_result {
        Ok(outcome) => outcome,
        Err(error) => {
            let shared = state.lock().shared.clone();
            shared.lock().mark_extraction_completed();
            return Err(error);
        }
    };

    let shared = state.lock();
    let shared_guard = shared.shared.lock();
    store.append_named_event(
        config.session_id,
        "tengu_session_memory_extraction",
        json!({
            "input_tokens": outcome.usage.input_tokens,
            "output_tokens": outcome.usage.output_tokens,
            "cache_read_input_tokens": outcome.cache_read_input_tokens,
            "cache_creation_input_tokens": outcome.cache_creation_input_tokens,
            "config_min_message_tokens_to_init": shared_guard.config.minimum_message_tokens_to_init,
            "config_min_tokens_between_update": shared_guard.config.minimum_tokens_between_update,
            "config_tool_calls_between_updates": shared_guard.config.tool_calls_between_updates,
        }),
    )?;
    drop(shared_guard);
    drop(shared);

    let mut state = state.lock();
    state
        .shared
        .lock()
        .record_extraction_token_count(count_conversation_tokens(conversation));
    update_last_summarized_message_id_if_safe(conversation, &mut state);
    state.shared.lock().mark_extraction_completed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SessionMemoryPermissionBroker, session_memory_shared_state_for_session};
    use claude_config::{ProviderOverrides, RuntimeConfig, RuntimeOverrides, load_runtime_config};
    use claude_core::{ConversationEntry, PermissionMode, ProviderProtocol};
    use claude_permissions::{PermissionBroker, PermissionRequest};
    use claude_provider::context::ContextWindowManager;
    use claude_session::session_memory::ensure_session_memory_file;
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::{TempDir, tempdir};

    struct TestEnv {
        _env_guard: MutexGuard<'static, ()>,
        previous_claude_config_dir: Option<OsString>,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            match &self.previous_claude_config_dir {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_runtime() -> (TempDir, RuntimeConfig, TestEnv) {
        let env_guard = env_lock().lock().expect("env lock");
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&profile).expect("profile");
        let previous_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        // Session memory intentionally uses Claude's global config dir.
        // Scope tests to the temp profile so they never touch user state.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &profile) };

        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                base_url: Some("mock://provider".to_owned()),
                api_key: Some("mock".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config");
        let env = TestEnv {
            _env_guard: env_guard,
            previous_claude_config_dir,
        };
        (tempdir, config, env)
    }

    #[tokio::test]
    async fn session_memory_broker_allows_exact_edit_file_path_only() {
        let tempdir = tempdir().expect("tempdir");
        let summary_path = tempdir.path().join("summary.md");
        std::fs::write(&summary_path, "# Session Title\n").expect("write summary");
        let broker = SessionMemoryPermissionBroker::new(summary_path.clone());

        let allow = broker
            .decide(PermissionRequest {
                tool_name: "edit_file".to_owned(),
                permission_class: None,
                tool_input: json!({
                    "path": summary_path,
                    "edits": [{"search": "# Session Title", "replace": "# Session Title"}]
                }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(allow.allowed);

        let allow_file_path = broker
            .decide(PermissionRequest {
                tool_name: "edit_file".to_owned(),
                permission_class: None,
                tool_input: json!({
                    "file_path": summary_path,
                    "edits": [{"search": "# Session Title", "replace": "# Session Title"}]
                }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(allow_file_path.allowed);
    }

    #[tokio::test]
    async fn session_memory_broker_denies_replace_in_file_and_wrong_path() {
        let tempdir = tempdir().expect("tempdir");
        let summary_path = tempdir.path().join("summary.md");
        let other_path = tempdir.path().join("other.md");
        std::fs::write(&summary_path, "# Session Title\n").expect("write summary");
        std::fs::write(&other_path, "# Other\n").expect("write other");
        let broker = SessionMemoryPermissionBroker::new(summary_path.clone());

        let deny_replace = broker
            .decide(PermissionRequest {
                tool_name: "replace_in_file".to_owned(),
                permission_class: None,
                tool_input: json!({
                    "path": summary_path,
                    "search": "a",
                    "replace": "b"
                }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(!deny_replace.allowed);

        let deny_other_path = broker
            .decide(PermissionRequest {
                tool_name: "edit_file".to_owned(),
                permission_class: None,
                tool_input: json!({
                    "path": other_path,
                    "edits": [{"search": "# Other", "replace": "# Other"}]
                }),
                working_directory: None,
                tool_use_id: None,
                title: None,
                description: None,
                blocked_path: None,
                permission_suggestions: Vec::new(),
            })
            .await;
        assert!(!deny_other_path.allowed);
    }

    #[tokio::test]
    async fn try_session_memory_compaction_returns_compacted_conversation_and_clears_boundary() {
        let (_tempdir, config, _env) = test_runtime();
        let summary_path = ensure_session_memory_file(&config).expect("summary file");
        std::fs::write(
            &summary_path,
            "# Session Title\nA real summary\n\n# Current State\nWorking state\n",
        )
        .expect("write summary");
        let shared = session_memory_shared_state_for_session(config.session_id).await;
        {
            let mut guard = shared.lock();
            guard.initialized = true;
            guard.set_last_summarized_message_id(None);
        }
        let conversation = vec![
            ConversationEntry::system("sys"),
            ConversationEntry::user("user ".repeat(300)),
            ConversationEntry::assistant("assistant ".repeat(300)),
            ConversationEntry::user("latest ".repeat(200)),
        ];
        let manager = ContextWindowManager::new(20_000, 100);
        let compacted =
            super::try_session_memory_compaction_with_gate(&config, &conversation, &manager, true)
                .await
                .expect("session memory compaction should succeed");

        assert!(!compacted.is_empty());
        assert!(
            compacted
                .iter()
                .any(|entry| entry.text.contains("A real summary")
                    || entry.text.contains("Current State"))
        );
        let shared = session_memory_shared_state_for_session(config.session_id).await;
        assert!(shared.lock().last_summarized_message_id.is_none());
    }
}
