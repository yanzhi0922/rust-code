use std::collections::HashMap;
use std::sync::Arc;

use claude_config::RuntimeConfig;
use claude_permissions::PermissionDecision;
use claude_provider::ProviderClient;
use claude_session::SessionStore;
use rc_claude_adapter::ClaudeInProcessAdapter;
use rc_codex_adapter::CodexInProcessAdapter;
use rc_roo_adapter::RooInProcessAdapter;
use tokio::sync::{Mutex, oneshot};

use crate::dto::*;

pub(crate) const CODEX_GLOBAL_ADAPTER_KEY: &str = "__codex_global__";
pub(crate) const APP_EVENT_PERMISSION_REQUEST: &str = "gui://permission-request";
pub(crate) const APP_EVENT_PERMISSION_RESOLVED: &str = "gui://permission-resolved";
pub(crate) const APP_EVENT_TOOL_START: &str = "gui://tool-start";
pub(crate) const APP_EVENT_TOOL_RESULT: &str = "gui://tool-result";
pub(crate) const APP_EVENT_TOOL_PROGRESS: &str = "gui://tool-progress";
pub(crate) const APP_EVENT_CODEX_APP_SERVER_NOTIFICATION: &str =
    "gui://codex-app-server-notification";
pub(crate) const APP_EVENT_STREAMING_DELTA: &str = "gui://streaming-delta";
pub(crate) const APP_EVENT_PROMPT_DONE: &str = "gui://prompt-done";
pub(crate) const APP_EVENT_SUBTASK_STARTED: &str = "gui://subtask-started";
pub(crate) const APP_EVENT_SUBTASK_PROGRESS: &str = "gui://subtask-progress";
pub(crate) const APP_EVENT_SUBTASK_COMPLETED: &str = "gui://subtask-completed";
pub(crate) const APP_EVENT_BATCH_PROGRESS: &str = "gui://batch-progress";
pub(crate) const APP_EVENT_TASK_SNAPSHOT: &str = "gui://task-snapshot";
pub(crate) const APP_EVENT_CONTEXT_USAGE: &str = "gui://context-usage";
pub(crate) const APP_EVENT_CONTEXT_OVERFLOW: &str = "gui://context-overflow";
pub(crate) const APP_EVENT_CONTEXT_COMPACTED: &str = "gui://context-compacted";
pub(crate) const APP_EVENT_RUNTIME_STATUS: &str = "gui://runtime-status";
pub(crate) const APP_EVENT_CODEX_RECOVERABLE_ERROR: &str = "gui://codex-recoverable-error";
pub(crate) const PROJECTS_FILE_NAME: &str = "gui-projects.json";
pub(crate) const PROVIDERS_FILE_NAME: &str = "gui-providers.json";
pub(crate) const SETTINGS_FILE_NAME: &str = "gui-settings.json";
pub(crate) const DEFAULT_MAX_TURNS: usize = 128;
pub(crate) const PERMISSION_WAIT_SECS: u64 = 60 * 30;
pub(crate) const KEYRING_SERVICE: &str = "remote-code-gui";

pub(crate) fn keyring_store(provider_name: &str, api_key: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, provider_name) {
        let _ = entry.set_password(api_key);
    }
}

pub(crate) fn keyring_retrieve(provider_name: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, provider_name)
        .ok()
        .and_then(|entry| entry.get_password().ok())
}

pub(crate) fn keyring_delete(provider_name: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, provider_name) {
        let _ = entry.delete_credential();
    }
}

pub(crate) struct RuntimeState {
    pub(crate) config: RuntimeConfig,
    pub(crate) provider: Arc<ProviderClient>,
    pub(crate) session_store: Arc<SessionStore>,
    pub(crate) projects: Vec<ProjectEntry>,
    pub(crate) provider_configs: ProviderConfigList,
    pub(crate) gui_settings: GuiSettingsFile,
}

pub(crate) struct AppState {
    pub(crate) runtime: Mutex<RuntimeState>,
    pub(crate) pending_permissions:
        Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>,
    pub(crate) pending_codex_permissions: Arc<Mutex<HashMap<String, CodexPendingPermission>>>,
    pub(crate) pending_roo_permissions: Arc<Mutex<HashMap<String, RooPendingPermission>>>,
    pub(crate) pending_claude_permissions: Arc<Mutex<HashMap<String, ClaudePendingPermission>>>,
    pub(crate) running_prompts: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Codex in-process adapters (session_id → adapter).
    pub(crate) active_codex_adapters: Arc<Mutex<HashMap<String, CodexInProcessAdapter>>>,
    /// Roo in-process adapters (session_id → adapter).
    pub(crate) active_roo_adapters: Arc<Mutex<HashMap<String, RooInProcessAdapter>>>,
    /// Claude in-process adapters (session_id → adapter).
    pub(crate) active_claude_adapters: Arc<Mutex<HashMap<String, ClaudeInProcessAdapter>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexPendingPermission {
    pub(crate) session_id: String,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RooPendingPermission {
    pub(crate) session_id: String,
    pub(crate) request_id: String,
    pub(crate) request_kind: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ClaudePendingPermission {
    pub(crate) session_id: String,
    pub(crate) request_id: String,
}
