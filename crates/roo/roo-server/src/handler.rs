//! JSON-RPC request handler.
//!
//! Source: `src/core/webview/webviewMessageHandler.ts` — handles all WebviewMessage types
//! Source: `packages/types/src/ipc.ts` — TaskCommand handling
//!
//! This module implements the handler for each JSON-RPC method, mapping them
//! to the corresponding TypeScript webviewMessageHandler operations.
//!
//! R10-A: Updated to use TaskLifecycle and AskSayHandler from R9-C.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tracing::{debug, error, info, instrument, warn};

use roo_app::App;
use roo_config::paths;
use roo_jsonrpc::types::Message;
use roo_task::TaskManager;
use roo_task::ask_say::AskResponse;
use roo_task::events::TaskEvent;
use roo_task::task_lifecycle::TaskLifecycle;

use crate::error::{ServerError, ServerResult};

// ---------------------------------------------------------------------------
// JSON-RPC Method Names
// ---------------------------------------------------------------------------

/// Standard JSON-RPC lifecycle methods.
pub mod methods {
    /// Initialize the server.
    pub const INITIALIZE: &str = "initialize";
    /// Shutdown the server.
    pub const SHUTDOWN: &str = "shutdown";
    /// Ping (keep-alive).
    pub const PING: &str = "ping";

    // ── Task commands (from TaskCommandName in ipc.ts) ──
    pub const TASK_START: &str = "task/start";
    pub const TASK_CANCEL: &str = "task/cancel";
    pub const TASK_CLOSE: &str = "task/close";
    pub const TASK_RESUME: &str = "task/resume";
    pub const TASK_SEND_MESSAGE: &str = "task/sendMessage";
    pub const TASK_GET_COMMANDS: &str = "task/getCommands";
    pub const TASK_GET_MODES: &str = "task/getModes";
    pub const TASK_GET_MODELS: &str = "task/getModels";
    pub const TASK_DELETE_QUEUED_MESSAGE: &str = "task/deleteQueuedMessage";

    // ── State commands ──
    pub const STATE_GET: &str = "state/get";
    pub const STATE_SET_MODE: &str = "state/setMode";
    pub const SYSTEM_PROMPT_BUILD: &str = "systemPrompt/build";
    pub const HISTORY_GET: &str = "history/get";
    pub const HISTORY_DELETE: &str = "history/delete";
    pub const HISTORY_EXPORT: &str = "history/export";
    pub const TODO_UPDATE: &str = "todo/update";
    pub const ASK_RESPONSE: &str = "ask/response";
    pub const TERMINAL_OPERATION: &str = "terminal/operation";
    pub const TASK_CONDENSE: &str = "task/condense";
    pub const TASK_CLEAR: &str = "task/clear";
    pub const TASK_CANCEL_AUTO_APPROVAL: &str = "task/cancelAutoApproval";
    pub const TASK_GET_AGGREGATED_COSTS: &str = "task/getAggregatedCosts";
    pub const TASK_SHOW_WITH_ID: &str = "task/showWithId";
    pub const CHECKPOINT_DIFF: &str = "checkpoint/diff";
    pub const CHECKPOINT_RESTORE: &str = "checkpoint/restore";
    pub const PROMPT_ENHANCE: &str = "prompt/enhance";
    pub const SEARCH_FILES: &str = "search/files";
    pub const FILE_READ: &str = "file/read";
    pub const GIT_SEARCH_COMMITS: &str = "git/searchCommits";
    pub const MCP_LIST_SERVERS: &str = "mcp/listServers";
    pub const MCP_RESTART_SERVER: &str = "mcp/restartServer";
    pub const MCP_TOGGLE_SERVER: &str = "mcp/toggleServer";
    pub const MCP_USE_TOOL: &str = "mcp/useTool";
    pub const MCP_ACCESS_RESOURCE: &str = "mcp/accessResource";
    pub const MCP_DELETE_SERVER: &str = "mcp/deleteServer";
    pub const MCP_UPDATE_TIMEOUT: &str = "mcp/updateTimeout";
    pub const MCP_REFRESH_ALL: &str = "mcp/refreshAll";
    pub const MCP_TOGGLE_TOOL_ALWAYS_ALLOW: &str = "mcp/toggleToolAlwaysAllow";
    pub const MCP_TOGGLE_TOOL_ENABLED_FOR_PROMPT: &str = "mcp/toggleToolEnabledForPrompt";

    // ── Settings commands ──
    pub const SETTINGS_UPDATE: &str = "settings/update";
    pub const SETTINGS_SAVE_API_CONFIG: &str = "settings/saveApiConfig";
    pub const SETTINGS_LOAD_API_CONFIG: &str = "settings/loadApiConfig";
    pub const SETTINGS_LOAD_API_CONFIG_BY_ID: &str = "settings/loadApiConfigById";
    pub const SETTINGS_DELETE_API_CONFIG: &str = "settings/deleteApiConfig";
    pub const SETTINGS_LIST_API_CONFIGS: &str = "settings/listApiConfigs";
    pub const SETTINGS_UPSERT_API_CONFIG: &str = "settings/upsertApiConfig";
    pub const SETTINGS_RENAME_API_CONFIG: &str = "settings/renameApiConfig";
    pub const SETTINGS_CUSTOM_INSTRUCTIONS: &str = "settings/customInstructions";
    pub const SETTINGS_UPDATE_PROMPT: &str = "settings/updatePrompt";
    pub const SETTINGS_COPY_SYSTEM_PROMPT: &str = "settings/copySystemPrompt";
    pub const SETTINGS_RESET_STATE: &str = "settings/resetState";
    pub const SETTINGS_IMPORT_SETTINGS: &str = "settings/importSettings";
    pub const SETTINGS_EXPORT_SETTINGS: &str = "settings/exportSettings";
    pub const SETTINGS_LOCK_API_CONFIG: &str = "settings/lockApiConfig";
    pub const SETTINGS_TOGGLE_API_CONFIG_PIN: &str = "settings/toggleApiConfigPin";
    pub const SETTINGS_ENHANCEMENT_API_CONFIG_ID: &str = "settings/enhancementApiConfigId";
    pub const SETTINGS_AUTO_APPROVAL_ENABLED: &str = "settings/autoApprovalEnabled";
    pub const SETTINGS_DEBUG_SETTING: &str = "settings/debugSetting";
    pub const SETTINGS_ALLOWED_COMMANDS: &str = "settings/allowedCommands";
    pub const SETTINGS_DENIED_COMMANDS: &str = "settings/deniedCommands";
    pub const SETTINGS_CONDENSING_PROMPT: &str = "settings/condensingPrompt";
    pub const SETTINGS_SET_API_CONFIG_PASSWORD: &str = "settings/setApiConfigPassword";
    pub const SETTINGS_HAS_OPENED_MODE_SELECTOR: &str = "settings/hasOpenedModeSelector";
    pub const SETTINGS_TASK_SYNC_ENABLED: &str = "settings/taskSyncEnabled";
    pub const SETTINGS_UPDATE_SETTINGS: &str = "settings/updateSettings";
    pub const SETTINGS_UPDATE_VSCODE_SETTING: &str = "settings/updateVSCodeSetting";
    pub const SETTINGS_GET_VSCODE_SETTING: &str = "settings/getVSCodeSetting";

    // ── Skills commands ──
    pub const SKILLS_LIST: &str = "skills/list";
    pub const SKILLS_CREATE: &str = "skills/create";
    pub const SKILLS_DELETE: &str = "skills/delete";
    pub const SKILLS_MOVE: &str = "skills/move";
    pub const SKILLS_UPDATE_MODES: &str = "skills/updateModes";
    pub const SKILL_OPEN_FILE: &str = "skill/openFile";

    // ── Mode commands ──
    pub const MODE_UPDATE_CUSTOM: &str = "mode/updateCustom";
    pub const MODE_DELETE_CUSTOM: &str = "mode/deleteCustom";
    pub const MODE_EXPORT: &str = "mode/export";
    pub const MODE_IMPORT: &str = "mode/import";
    pub const MODE_SWITCH: &str = "mode/switch";
    pub const MODE_CHECK_RULES: &str = "mode/checkRulesDirectory";
    pub const MODE_OPEN_SETTINGS: &str = "mode/openSettings";
    pub const MODE_SET_OPENAI_CUSTOM_MODEL_INFO: &str = "mode/setOpenAiCustomModelInfo";

    // ── Message commands ──
    pub const MESSAGE_DELETE: &str = "message/delete";
    pub const MESSAGE_EDIT: &str = "message/edit";
    pub const MESSAGE_QUEUE: &str = "message/queue";
    pub const MESSAGE_DELETE_CONFIRM: &str = "message/deleteConfirm";
    pub const MESSAGE_EDIT_CONFIRM: &str = "message/editConfirm";
    pub const MESSAGE_EDIT_QUEUED: &str = "message/editQueued";
    pub const MESSAGE_REMOVE_QUEUED: &str = "message/removeQueued";
    pub const MESSAGE_SUBMIT_EDITED: &str = "message/submitEdited";

    // ── History commands (additional) ──
    pub const HISTORY_DELETE_MULTIPLE: &str = "history/deleteMultiple";
    pub const HISTORY_SHARE_TASK: &str = "history/shareTask";

    // ── Tools commands ──
    pub const TOOLS_REFRESH_CUSTOM: &str = "tools/refreshCustom";

    // ── Telemetry commands ──
    pub const TELEMETRY_SET_SETTING: &str = "telemetry/setSetting";

    // ── Marketplace commands ──
    /// Source: TS `webviewMessageHandler.ts` — `installMarketplaceItem`
    pub const MARKETPLACE_INSTALL: &str = "marketplace/install";
    /// Source: TS `webviewMessageHandler.ts` — `removeInstalledMarketplaceItem`
    pub const MARKETPLACE_REMOVE: &str = "marketplace/remove";
    /// Source: TS `webviewMessageHandler.ts` — `installMarketplaceItemWithParameters`
    pub const MARKETPLACE_INSTALL_WITH_PARAMS: &str = "marketplace/installWithParams";
    /// Source: TS `webviewMessageHandler.ts` — `fetchMarketplaceData`
    pub const MARKETPLACE_FETCH_DATA: &str = "marketplace/fetchData";
    /// Source: TS `webviewMessageHandler.ts` — `filterMarketplaceItems`
    pub const MARKETPLACE_FILTER_ITEMS: &str = "marketplace/filterItems";
    /// Source: TS `webviewMessageHandler.ts` — `marketplaceButtonClicked`
    pub const MARKETPLACE_BUTTON_CLICKED: &str = "marketplace/buttonClicked";
    /// Source: TS `webviewMessageHandler.ts` — `cancelMarketplaceInstall`
    pub const MARKETPLACE_CANCEL_INSTALL: &str = "marketplace/cancelInstall";

    // ── Worktree commands ──
    /// Source: TS `webviewMessageHandler.ts` — `listWorktrees`
    pub const WORKTREE_LIST: &str = "worktree/list";
    /// Source: TS `webviewMessageHandler.ts` — `createWorktree`
    pub const WORKTREE_CREATE: &str = "worktree/create";
    /// Source: TS `webviewMessageHandler.ts` — `deleteWorktree`
    pub const WORKTREE_DELETE: &str = "worktree/delete";
    /// Source: TS `webviewMessageHandler.ts` — `switchWorktree`
    pub const WORKTREE_SWITCH: &str = "worktree/switch";
    /// Source: TS `webviewMessageHandler.ts` — `getAvailableBranches`
    pub const WORKTREE_GET_BRANCHES: &str = "worktree/getBranches";
    /// Source: TS `webviewMessageHandler.ts` — `getWorktreeDefaults`
    pub const WORKTREE_GET_DEFAULTS: &str = "worktree/getDefaults";
    /// Source: TS `webviewMessageHandler.ts` — `getWorktreeIncludeStatus`
    pub const WORKTREE_GET_INCLUDE_STATUS: &str = "worktree/getIncludeStatus";
    /// Source: TS `webviewMessageHandler.ts` — `checkBranchWorktreeInclude`
    pub const WORKTREE_CHECK_BRANCH_INCLUDE: &str = "worktree/checkBranchInclude";
    /// Source: TS `webviewMessageHandler.ts` — `createWorktreeInclude`
    pub const WORKTREE_CREATE_INCLUDE: &str = "worktree/createInclude";
    /// Source: TS `webviewMessageHandler.ts` — `checkoutBranch`
    pub const WORKTREE_CHECKOUT_BRANCH: &str = "worktree/checkoutBranch";
    /// Source: TS `webviewMessageHandler.ts` — `browseForWorktreePath`
    pub const WORKTREE_BROWSE_PATH: &str = "worktree/browsePath";

    // ── TTS commands ──
    /// Source: TS `webviewMessageHandler.ts` — `playTts`
    pub const TTS_PLAY: &str = "tts/play";
    /// Source: TS `webviewMessageHandler.ts` — `stopTts`
    pub const TTS_STOP: &str = "tts/stop";
    /// Source: TS `webviewMessageHandler.ts` — `ttsEnabled`
    pub const TTS_ENABLED: &str = "tts/enabled";
    /// Source: TS `webviewMessageHandler.ts` — `ttsSpeed`
    pub const TTS_SPEED: &str = "tts/speed";

    // ── Image commands ──
    /// Source: TS `webviewMessageHandler.ts` — `saveImage`
    pub const IMAGE_SAVE: &str = "image/save";
    /// Source: TS `webviewMessageHandler.ts` — `openImage`
    pub const IMAGE_OPEN: &str = "image/open";

    // ── Model request commands ──
    /// Source: TS `webviewMessageHandler.ts` — `flushRouterModels`
    pub const MODELS_FLUSH_ROUTER: &str = "models/flushRouter";
    /// Source: TS `webviewMessageHandler.ts` — `requestRouterModels`
    pub const MODELS_REQUEST_ROUTER: &str = "models/requestRouter";
    /// Source: TS `webviewMessageHandler.ts` — `requestOpenAiModels`
    pub const MODELS_REQUEST_OPENAI: &str = "models/requestOpenAi";
    /// Source: TS `webviewMessageHandler.ts` — `requestOllamaModels`
    pub const MODELS_REQUEST_OLLAMA: &str = "models/requestOllama";
    /// Source: TS `webviewMessageHandler.ts` — `requestLmStudioModels`
    pub const MODELS_REQUEST_LMSTUDIO: &str = "models/requestLmStudio";
    /// Source: TS `webviewMessageHandler.ts` — `requestRooModels`
    pub const MODELS_REQUEST_ROO: &str = "models/requestRoo";
    /// Source: TS `webviewMessageHandler.ts` — `requestRooCreditBalance`
    pub const MODELS_REQUEST_ROO_CREDIT: &str = "models/requestRooCredit";
    /// Source: TS `webviewMessageHandler.ts` — `requestVsCodeLmModels`
    pub const MODELS_REQUEST_VSCODELM: &str = "models/requestVsCodeLm";

    // ── Mention commands ──
    /// Source: TS `webviewMessageHandler.ts` — `openMention`
    pub const MENTION_OPEN: &str = "mention/open";
    /// Source: TS `webviewMessageHandler.ts` — `resolveMentions` (internal)
    pub const MENTION_RESOLVE: &str = "mention/resolve";

    // ── Command (slash commands) ──
    /// Source: TS `webviewMessageHandler.ts` — `requestCommands`
    pub const COMMAND_REQUEST: &str = "command/request";
    /// Source: TS `webviewMessageHandler.ts` — `openCommandFile`
    pub const COMMAND_OPEN_FILE: &str = "command/openFile";
    /// Source: TS `webviewMessageHandler.ts` — `deleteCommand`
    pub const COMMAND_DELETE: &str = "command/delete";
    /// Source: TS `webviewMessageHandler.ts` — `createCommand`
    pub const COMMAND_CREATE: &str = "command/create";

    // ── UI / VS Code-specific commands (stubs for headless) ──
    /// Source: TS `webviewMessageHandler.ts` — `webviewDidLaunch`
    pub const WEBVIEW_DID_LAUNCH: &str = "webview/didLaunch";
    /// Source: TS `webviewMessageHandler.ts` — `didShowAnnouncement`
    pub const ANNOUNCEMENT_DID_SHOW: &str = "announcement/didShow";
    /// Source: TS `webviewMessageHandler.ts` — `selectImages`
    pub const IMAGES_SELECT: &str = "images/select";
    /// Source: TS `webviewMessageHandler.ts` — `draggedImages`
    pub const IMAGES_DRAGGED: &str = "images/dragged";
    /// Source: TS `webviewMessageHandler.ts` — `playSound`
    pub const PLAY_SOUND: &str = "sound/play";
    /// Source: TS `webviewMessageHandler.ts` — `openFile`
    pub const FILE_OPEN: &str = "file/open";
    /// Source: TS `webviewMessageHandler.ts` — `openExternal`
    pub const EXTERNAL_OPEN: &str = "external/open";
    /// Source: TS `webviewMessageHandler.ts` — `openKeyboardShortcuts`
    pub const OPEN_KEYBOARD_SHORTCUTS: &str = "ui/openKeyboardShortcuts";
    /// Source: TS `webviewMessageHandler.ts` — `openMcpSettings`
    pub const OPEN_MCP_SETTINGS: &str = "mcp/openSettings";
    /// Source: TS `webviewMessageHandler.ts` — `openProjectMcpSettings`
    pub const OPEN_PROJECT_MCP_SETTINGS: &str = "mcp/openProjectSettings";
    /// Source: TS `webviewMessageHandler.ts` — `focusPanelRequest`
    pub const FOCUS_PANEL: &str = "ui/focusPanel";
    /// Source: TS `webviewMessageHandler.ts` — `switchTab`
    pub const TAB_SWITCH: &str = "ui/switchTab";
    /// Source: TS `webviewMessageHandler.ts` — `insertTextIntoTextarea`
    pub const INSERT_TEXT: &str = "ui/insertText";
    /// Source: TS `webviewMessageHandler.ts` — `openMarkdownPreview`
    pub const MARKDOWN_PREVIEW: &str = "ui/markdownPreview";

    // ── Cloud commands ──
    /// Source: TS `webviewMessageHandler.ts` — `rooCloudSignIn`
    pub const CLOUD_SIGN_IN: &str = "cloud/signIn";
    /// Source: TS `webviewMessageHandler.ts` — `rooCloudSignOut`
    pub const CLOUD_SIGN_OUT: &str = "cloud/signOut";
    /// Source: TS `webviewMessageHandler.ts` — `rooCloudManualUrl`
    pub const CLOUD_MANUAL_URL: &str = "cloud/manualUrl";
    /// Source: TS `webviewMessageHandler.ts` — `cloudButtonClicked`
    pub const CLOUD_BUTTON_CLICKED: &str = "cloud/buttonClicked";
    /// Source: TS `webviewMessageHandler.ts` — `clearCloudAuthSkipModel`
    pub const CLOUD_CLEAR_SKIP_MODEL: &str = "cloud/clearSkipModel";
    /// Source: TS `webviewMessageHandler.ts` — `switchOrganization`
    pub const CLOUD_SWITCH_ORG: &str = "cloud/switchOrganization";
    /// Source: TS `webviewMessageHandler.ts` — `openAiCodexSignIn`
    pub const CODEX_SIGN_IN: &str = "codex/signIn";
    /// Source: TS `webviewMessageHandler.ts` — `openAiCodexSignOut`
    pub const CODEX_SIGN_OUT: &str = "codex/signOut";
    /// Source: TS `webviewMessageHandler.ts` — `requestOpenAiCodexRateLimits`
    pub const CODEX_REQUEST_RATE_LIMITS: &str = "codex/requestRateLimits";

    // ── Codebase Index commands ──
    /// Source: TS `webviewMessageHandler.ts` — `codebaseIndexEnabled`
    pub const INDEX_ENABLED: &str = "index/enabled";
    /// Source: TS `webviewMessageHandler.ts` — `requestIndexingStatus`
    pub const INDEX_REQUEST_STATUS: &str = "index/requestStatus";
    /// Source: TS `webviewMessageHandler.ts` — `startIndexing`
    pub const INDEX_START: &str = "index/start";
    /// Source: TS `webviewMessageHandler.ts` — `stopIndexing`
    pub const INDEX_STOP: &str = "index/stop";
    /// Source: TS `webviewMessageHandler.ts` — `clearIndexData`
    pub const INDEX_CLEAR: &str = "index/clear";
    /// Source: TS `webviewMessageHandler.ts` — `toggleWorkspaceIndexing`
    pub const INDEX_TOGGLE_WORKSPACE: &str = "index/toggleWorkspace";
    /// Source: TS `webviewMessageHandler.ts` — `setAutoEnableDefault`
    pub const INDEX_SET_AUTO_ENABLE: &str = "index/setAutoEnable";
    /// Source: TS `webviewMessageHandler.ts` — `saveCodeIndexSettingsAtomic`
    pub const INDEX_SAVE_SETTINGS: &str = "index/saveSettings";
    /// Source: TS `webviewMessageHandler.ts` — `requestCodeIndexSecretStatus`
    pub const INDEX_REQUEST_SECRET_STATUS: &str = "index/requestSecretStatus";

    // ── Upsell commands ──
    /// Source: TS `webviewMessageHandler.ts` — `dismissUpsell`
    pub const UPSELL_DISMISS: &str = "upsell/dismiss";
    /// Source: TS `webviewMessageHandler.ts` — `getDismissedUpsells`
    pub const UPSELL_GET_DISMISSED: &str = "upsell/getDismissed";

    // ── Debug commands ──
    /// Source: TS `webviewMessageHandler.ts` — `openDebugApiHistory`
    pub const DEBUG_API_HISTORY: &str = "debug/apiHistory";
    /// Source: TS `webviewMessageHandler.ts` — `openDebugUiHistory`
    pub const DEBUG_UI_HISTORY: &str = "debug/uiHistory";
    /// Source: TS `webviewMessageHandler.ts` — `downloadErrorDiagnostics`
    pub const DEBUG_DOWNLOAD_DIAGNOSTICS: &str = "debug/downloadDiagnostics";

    // ── Other commands ──
    /// Source: TS `webviewMessageHandler.ts` — `showMdmAuthRequiredNotification`
    pub const MDM_AUTH_NOTIFICATION: &str = "mdm/authNotification";
    /// Source: TS `webviewMessageHandler.ts` — `imageGenerationSettings`
    pub const IMAGE_GENERATION_SETTINGS: &str = "imageGeneration/settings";

    // ── Notification method (server → client) ──
    /// Method name for task event notifications sent from server to client.
    pub const NOTIFICATION_TASK_EVENT: &str = "notification/taskEvent";
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handles JSON-RPC requests by dispatching to the appropriate App method.
///
/// Source: `src/core/webview/webviewMessageHandler.ts` — `webviewMessageHandler` function
///
/// R10-A: Now uses [`TaskLifecycle`] for all task operations and forwards
/// [`TaskEvent`]s to the client as JSON-RPC notifications.
pub struct Handler {
    app: Arc<tokio::sync::RwLock<App>>,
    task_manager: Arc<TaskManager>,
    /// Pending JSON-RPC notifications to be sent to the client.
    /// Event listeners push notifications here; the server polls them.
    pending_notifications: Arc<Mutex<Vec<Message>>>,
}

impl Handler {
    /// Create a new handler wrapping the given App.
    pub fn new(app: App) -> Self {
        Self {
            app: Arc::new(tokio::sync::RwLock::new(app)),
            task_manager: Arc::new(TaskManager::new()),
            pending_notifications: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a handler from an already-wrapped App.
    pub fn from_arc(app: Arc<tokio::sync::RwLock<App>>) -> Self {
        Self {
            app,
            task_manager: Arc::new(TaskManager::new()),
            pending_notifications: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // ── Settings persistence helpers ──────────────────────────────────────

    /// Returns the path to the roo-settings JSON file.
    ///
    /// Uses `{global_storage_path}/settings/roo-settings.json` when available,
    /// otherwise falls back to `~/.roo/settings/roo-settings.json`.
    fn roo_settings_path(global_storage_path: &str) -> std::path::PathBuf {
        let base = if global_storage_path.is_empty() {
            paths::get_global_roo_directory()
        } else {
            std::path::PathBuf::from(global_storage_path)
        };
        base.join("settings").join("roo-settings.json")
    }

    /// Read the current settings from disk. Returns an empty JSON object if
    /// the file does not exist yet.
    async fn read_roo_settings(global_storage_path: &str) -> Value {
        let path = Self::roo_settings_path(global_storage_path);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                warn!(path = %path.display(), error = %e, "Failed to parse settings file, using defaults");
                json!({})
            }),
            Err(_) => json!({}),
        }
    }

    /// Write settings back to disk using the safe atomic write from roo-config.
    async fn write_roo_settings(global_storage_path: &str, settings: &Value) -> Result<(), String> {
        let path = Self::roo_settings_path(global_storage_path);
        roo_config::safe_write_json(
            &path,
            settings,
            Some(roo_config::safe_write_json::SafeWriteJsonOptions { pretty_print: true }),
        )
        .await
        .map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to write settings file");
            format!("Failed to write settings: {}", e)
        })
    }

    /// Dispatch a JSON-RPC request message to the appropriate handler.
    #[instrument(skip(self, request), fields(method = %request.method.as_deref().unwrap_or("unknown")))]
    pub async fn handle(&self, request: &Message) -> Message {
        let id = match &request.id {
            Some(id) => id.clone(),
            None => return Message::response(Value::Null, json!(null)),
        };

        let method = match &request.method {
            Some(m) => m.as_str(),
            None => {
                return Message::error_response(
                    id,
                    roo_jsonrpc::types::error_codes::INVALID_REQUEST,
                    "Missing method field",
                );
            }
        };

        let params = request.params.clone().unwrap_or(Value::Null);
        debug!(method = method, "Handling request");

        let result = match method {
            methods::INITIALIZE => self.handle_initialize(params).await,
            methods::SHUTDOWN => self.handle_shutdown(params).await,
            methods::PING => self.handle_ping(params).await,
            methods::TASK_START => self.handle_task_start(params).await,
            methods::TASK_CANCEL => self.handle_task_cancel(params).await,
            methods::TASK_CLOSE => self.handle_task_close(params).await,
            methods::TASK_RESUME => self.handle_task_resume(params).await,
            methods::TASK_SEND_MESSAGE => self.handle_task_send_message(params).await,
            methods::TASK_GET_COMMANDS => self.handle_task_get_commands(params).await,
            methods::TASK_GET_MODES => self.handle_task_get_modes(params).await,
            methods::TASK_GET_MODELS => self.handle_task_get_models(params).await,
            methods::TASK_DELETE_QUEUED_MESSAGE => {
                self.handle_task_delete_queued_message(params).await
            }
            methods::TASK_CONDENSE => self.handle_task_condense(params).await,
            methods::TASK_CLEAR => self.handle_task_clear(params).await,
            methods::TASK_CANCEL_AUTO_APPROVAL => {
                self.handle_task_cancel_auto_approval(params).await
            }
            methods::TASK_GET_AGGREGATED_COSTS => {
                self.handle_task_get_aggregated_costs(params).await
            }
            methods::TASK_SHOW_WITH_ID => self.handle_task_show_with_id(params).await,
            methods::STATE_GET => self.handle_state_get(params).await,
            methods::STATE_SET_MODE => self.handle_state_set_mode(params).await,
            methods::SYSTEM_PROMPT_BUILD => self.handle_system_prompt_build(params).await,
            methods::HISTORY_GET => self.handle_history_get(params).await,
            methods::HISTORY_DELETE => self.handle_history_delete(params).await,
            methods::HISTORY_DELETE_MULTIPLE => self.handle_history_delete_multiple(params).await,
            methods::HISTORY_EXPORT => self.handle_history_export(params).await,
            methods::TODO_UPDATE => self.handle_todo_update(params).await,
            methods::ASK_RESPONSE => self.handle_ask_response(params).await,
            methods::TERMINAL_OPERATION => self.handle_terminal_operation(params).await,
            methods::CHECKPOINT_DIFF => self.handle_checkpoint_diff(params).await,
            methods::CHECKPOINT_RESTORE => self.handle_checkpoint_restore(params).await,
            methods::PROMPT_ENHANCE => self.handle_prompt_enhance(params).await,
            methods::SEARCH_FILES => self.handle_search_files(params).await,
            methods::FILE_READ => self.handle_file_read(params).await,
            methods::GIT_SEARCH_COMMITS => self.handle_git_search_commits(params).await,
            methods::MCP_LIST_SERVERS => self.handle_mcp_list_servers(params).await,
            methods::MCP_RESTART_SERVER => self.handle_mcp_restart_server(params).await,
            methods::MCP_TOGGLE_SERVER => self.handle_mcp_toggle_server(params).await,
            methods::MCP_USE_TOOL => self.handle_mcp_use_tool(params).await,
            methods::MCP_ACCESS_RESOURCE => self.handle_mcp_access_resource(params).await,
            methods::MCP_DELETE_SERVER => self.handle_mcp_delete_server(params).await,
            methods::MCP_UPDATE_TIMEOUT => self.handle_mcp_update_timeout(params).await,
            methods::MCP_REFRESH_ALL => self.handle_mcp_refresh_all(params).await,
            methods::MCP_TOGGLE_TOOL_ALWAYS_ALLOW => {
                self.handle_mcp_toggle_tool_always_allow(params).await
            }
            methods::MCP_TOGGLE_TOOL_ENABLED_FOR_PROMPT => {
                self.handle_mcp_toggle_tool_enabled_for_prompt(params).await
            }
            methods::SETTINGS_UPDATE => self.handle_settings_update(params).await,
            methods::SETTINGS_SAVE_API_CONFIG => self.handle_settings_save_api_config(params).await,
            methods::SETTINGS_LOAD_API_CONFIG => self.handle_settings_load_api_config(params).await,
            methods::SETTINGS_LOAD_API_CONFIG_BY_ID => {
                self.handle_settings_load_api_config_by_id(params).await
            }
            methods::SETTINGS_DELETE_API_CONFIG => {
                self.handle_settings_delete_api_config(params).await
            }
            methods::SETTINGS_LIST_API_CONFIGS => {
                self.handle_settings_list_api_configs(params).await
            }
            methods::SETTINGS_UPSERT_API_CONFIG => {
                self.handle_settings_upsert_api_config(params).await
            }
            methods::SETTINGS_RENAME_API_CONFIG => {
                self.handle_settings_rename_api_config(params).await
            }
            methods::SETTINGS_CUSTOM_INSTRUCTIONS => {
                self.handle_settings_custom_instructions(params).await
            }
            methods::SETTINGS_UPDATE_PROMPT => self.handle_settings_update_prompt(params).await,
            methods::SETTINGS_COPY_SYSTEM_PROMPT => {
                self.handle_settings_copy_system_prompt(params).await
            }
            methods::SETTINGS_RESET_STATE => self.handle_settings_reset_state(params).await,
            methods::SETTINGS_IMPORT_SETTINGS => self.handle_settings_import_settings(params).await,
            methods::SETTINGS_EXPORT_SETTINGS => self.handle_settings_export_settings(params).await,
            methods::SETTINGS_LOCK_API_CONFIG => self.handle_settings_lock_api_config(params).await,
            methods::SETTINGS_TOGGLE_API_CONFIG_PIN => {
                self.handle_settings_toggle_api_config_pin(params).await
            }
            methods::SETTINGS_ENHANCEMENT_API_CONFIG_ID => {
                self.handle_settings_enhancement_api_config_id(params).await
            }
            methods::SETTINGS_AUTO_APPROVAL_ENABLED => {
                self.handle_settings_auto_approval_enabled(params).await
            }
            methods::SETTINGS_DEBUG_SETTING => self.handle_settings_debug_setting(params).await,
            methods::SETTINGS_ALLOWED_COMMANDS => {
                self.handle_settings_allowed_commands(params).await
            }
            methods::SKILLS_LIST => self.handle_skills_list(params).await,
            methods::SKILLS_CREATE => self.handle_skills_create(params).await,
            methods::SKILLS_DELETE => self.handle_skills_delete(params).await,
            methods::SKILLS_MOVE => self.handle_skills_move(params).await,
            methods::SKILLS_UPDATE_MODES => self.handle_skills_update_modes(params).await,
            methods::SKILL_OPEN_FILE => self.handle_skill_open_file(params).await,
            methods::MODE_UPDATE_CUSTOM => self.handle_mode_update_custom(params).await,
            methods::MODE_DELETE_CUSTOM => self.handle_mode_delete_custom(params).await,
            methods::MODE_EXPORT => self.handle_mode_export(params).await,
            methods::MODE_IMPORT => self.handle_mode_import(params).await,
            methods::MODE_SWITCH => self.handle_mode_switch(params).await,
            methods::MODE_CHECK_RULES => self.handle_mode_check_rules(params).await,
            methods::MODE_OPEN_SETTINGS => self.handle_mode_open_settings(params).await,
            methods::MODE_SET_OPENAI_CUSTOM_MODEL_INFO => {
                self.handle_mode_set_openai_custom_model_info(params).await
            }
            methods::MESSAGE_DELETE => self.handle_message_delete(params).await,
            methods::MESSAGE_EDIT => self.handle_message_edit(params).await,
            methods::MESSAGE_QUEUE => self.handle_message_queue(params).await,
            methods::MESSAGE_DELETE_CONFIRM => self.handle_message_delete_confirm(params).await,
            methods::MESSAGE_EDIT_CONFIRM => self.handle_message_edit_confirm(params).await,
            methods::MESSAGE_EDIT_QUEUED => self.handle_message_edit_queued(params).await,
            methods::MESSAGE_REMOVE_QUEUED => self.handle_message_remove_queued(params).await,
            methods::MESSAGE_SUBMIT_EDITED => self.handle_message_submit_edited(params).await,
            methods::TOOLS_REFRESH_CUSTOM => self.handle_tools_refresh_custom(params).await,
            methods::TELEMETRY_SET_SETTING => self.handle_telemetry_set_setting(params).await,
            // ── Marketplace ──
            methods::MARKETPLACE_INSTALL => self.handle_marketplace_install(params).await,
            methods::MARKETPLACE_REMOVE => self.handle_marketplace_remove(params).await,
            methods::MARKETPLACE_INSTALL_WITH_PARAMS => {
                self.handle_marketplace_install_with_params(params).await
            }
            methods::MARKETPLACE_FETCH_DATA => self.handle_marketplace_fetch_data(params).await,
            methods::MARKETPLACE_FILTER_ITEMS => self.handle_marketplace_filter_items(params).await,
            methods::MARKETPLACE_BUTTON_CLICKED => {
                self.handle_marketplace_button_clicked(params).await
            }
            methods::MARKETPLACE_CANCEL_INSTALL => {
                self.handle_marketplace_cancel_install(params).await
            }
            // ── Worktree ──
            methods::WORKTREE_LIST => self.handle_worktree_list(params).await,
            methods::WORKTREE_CREATE => self.handle_worktree_create(params).await,
            methods::WORKTREE_DELETE => self.handle_worktree_delete(params).await,
            methods::WORKTREE_SWITCH => self.handle_worktree_switch(params).await,
            methods::WORKTREE_GET_BRANCHES => self.handle_worktree_get_branches(params).await,
            methods::WORKTREE_GET_DEFAULTS => self.handle_worktree_get_defaults(params).await,
            methods::WORKTREE_GET_INCLUDE_STATUS => {
                self.handle_worktree_get_include_status(params).await
            }
            methods::WORKTREE_CHECK_BRANCH_INCLUDE => {
                self.handle_worktree_check_branch_include(params).await
            }
            methods::WORKTREE_CREATE_INCLUDE => self.handle_worktree_create_include(params).await,
            methods::WORKTREE_CHECKOUT_BRANCH => self.handle_worktree_checkout_branch(params).await,
            methods::WORKTREE_BROWSE_PATH => self.handle_worktree_browse_path(params).await,
            // ── TTS ──
            methods::TTS_PLAY => self.handle_tts_play(params).await,
            methods::TTS_STOP => self.handle_tts_stop(params).await,
            methods::TTS_ENABLED => self.handle_tts_enabled(params).await,
            methods::TTS_SPEED => self.handle_tts_speed(params).await,
            // ── Image ──
            methods::IMAGE_SAVE => self.handle_image_save(params).await,
            methods::IMAGE_OPEN => self.handle_image_open(params).await,
            // ── Model requests ──
            methods::MODELS_FLUSH_ROUTER => self.handle_models_flush_router(params).await,
            methods::MODELS_REQUEST_ROUTER => self.handle_models_request_router(params).await,
            methods::MODELS_REQUEST_OPENAI => self.handle_models_request_openai(params).await,
            methods::MODELS_REQUEST_OLLAMA => self.handle_models_request_ollama(params).await,
            methods::MODELS_REQUEST_LMSTUDIO => self.handle_models_request_lmstudio(params).await,
            methods::MODELS_REQUEST_ROO => self.handle_models_request_roo(params).await,
            methods::MODELS_REQUEST_ROO_CREDIT => {
                self.handle_models_request_roo_credit(params).await
            }
            methods::MODELS_REQUEST_VSCODELM => self.handle_models_request_vscode_lm(params).await,
            // ── Mentions ──
            methods::MENTION_OPEN => self.handle_mention_open(params).await,
            methods::MENTION_RESOLVE => self.handle_mention_resolve(params).await,
            // ── Commands (slash) ──
            methods::COMMAND_REQUEST => self.handle_command_request(params).await,
            methods::COMMAND_OPEN_FILE => self.handle_command_open_file(params).await,
            methods::COMMAND_DELETE => self.handle_command_delete(params).await,
            methods::COMMAND_CREATE => self.handle_command_create(params).await,
            // ── Settings (additional) ──
            methods::SETTINGS_DENIED_COMMANDS => self.handle_settings_denied_commands(params).await,
            methods::SETTINGS_CONDENSING_PROMPT => {
                self.handle_settings_condensing_prompt(params).await
            }
            methods::SETTINGS_SET_API_CONFIG_PASSWORD => {
                self.handle_settings_set_api_config_password(params).await
            }
            methods::SETTINGS_HAS_OPENED_MODE_SELECTOR => {
                self.handle_settings_has_opened_mode_selector(params).await
            }
            methods::SETTINGS_TASK_SYNC_ENABLED => {
                self.handle_settings_task_sync_enabled(params).await
            }
            methods::SETTINGS_UPDATE_SETTINGS => self.handle_settings_update_settings(params).await,
            methods::SETTINGS_UPDATE_VSCODE_SETTING => {
                self.handle_settings_update_vscode_setting(params).await
            }
            methods::SETTINGS_GET_VSCODE_SETTING => {
                self.handle_settings_get_vscode_setting(params).await
            }
            // ── History (additional) ──
            methods::HISTORY_SHARE_TASK => self.handle_history_share_task(params).await,
            // ── UI / VS Code-specific (stubs) ──
            methods::WEBVIEW_DID_LAUNCH => self.handle_webview_did_launch(params).await,
            methods::ANNOUNCEMENT_DID_SHOW => self.handle_announcement_did_show(params).await,
            methods::IMAGES_SELECT => self.handle_images_select(params).await,
            methods::IMAGES_DRAGGED => self.handle_images_dragged(params).await,
            methods::PLAY_SOUND => self.handle_play_sound(params).await,
            methods::FILE_OPEN => self.handle_file_open(params).await,
            methods::EXTERNAL_OPEN => self.handle_external_open(params).await,
            methods::OPEN_KEYBOARD_SHORTCUTS => self.handle_open_keyboard_shortcuts(params).await,
            methods::OPEN_MCP_SETTINGS => self.handle_open_mcp_settings(params).await,
            methods::OPEN_PROJECT_MCP_SETTINGS => {
                self.handle_open_project_mcp_settings(params).await
            }
            methods::FOCUS_PANEL => self.handle_focus_panel(params).await,
            methods::TAB_SWITCH => self.handle_tab_switch(params).await,
            methods::INSERT_TEXT => self.handle_insert_text(params).await,
            methods::MARKDOWN_PREVIEW => self.handle_markdown_preview(params).await,
            // ── Cloud ──
            methods::CLOUD_SIGN_IN => self.handle_cloud_sign_in(params).await,
            methods::CLOUD_SIGN_OUT => self.handle_cloud_sign_out(params).await,
            methods::CLOUD_MANUAL_URL => self.handle_cloud_manual_url(params).await,
            methods::CLOUD_BUTTON_CLICKED => self.handle_cloud_button_clicked(params).await,
            methods::CLOUD_CLEAR_SKIP_MODEL => self.handle_cloud_clear_skip_model(params).await,
            methods::CLOUD_SWITCH_ORG => self.handle_cloud_switch_org(params).await,
            methods::CODEX_SIGN_IN => self.handle_codex_sign_in(params).await,
            methods::CODEX_SIGN_OUT => self.handle_codex_sign_out(params).await,
            methods::CODEX_REQUEST_RATE_LIMITS => {
                self.handle_codex_request_rate_limits(params).await
            }
            // ── Codebase Index ──
            methods::INDEX_ENABLED => self.handle_index_enabled(params).await,
            methods::INDEX_REQUEST_STATUS => self.handle_index_request_status(params).await,
            methods::INDEX_START => self.handle_index_start(params).await,
            methods::INDEX_STOP => self.handle_index_stop(params).await,
            methods::INDEX_CLEAR => self.handle_index_clear(params).await,
            methods::INDEX_TOGGLE_WORKSPACE => self.handle_index_toggle_workspace(params).await,
            methods::INDEX_SET_AUTO_ENABLE => self.handle_index_set_auto_enable(params).await,
            methods::INDEX_SAVE_SETTINGS => self.handle_index_save_settings(params).await,
            methods::INDEX_REQUEST_SECRET_STATUS => {
                self.handle_index_request_secret_status(params).await
            }
            // ── Upsell ──
            methods::UPSELL_DISMISS => self.handle_upsell_dismiss(params).await,
            methods::UPSELL_GET_DISMISSED => self.handle_upsell_get_dismissed(params).await,
            // ── Debug ──
            methods::DEBUG_API_HISTORY => self.handle_debug_api_history(params).await,
            methods::DEBUG_UI_HISTORY => self.handle_debug_ui_history(params).await,
            methods::DEBUG_DOWNLOAD_DIAGNOSTICS => {
                self.handle_debug_download_diagnostics(params).await
            }
            // ── Other ──
            methods::MDM_AUTH_NOTIFICATION => self.handle_mdm_auth_notification(params).await,
            methods::IMAGE_GENERATION_SETTINGS => {
                self.handle_image_generation_settings(params).await
            }
            _ => {
                return Message::error_response(
                    id,
                    roo_jsonrpc::types::error_codes::METHOD_NOT_FOUND,
                    &format!("Method not found: {}", method),
                );
            }
        };

        match result {
            Ok(value) => Message::response(id, value),
            Err(e) => {
                error!(error = %e, "Request handler error");
                let code = match &e {
                    ServerError::MethodNotFound(_) => {
                        roo_jsonrpc::types::error_codes::METHOD_NOT_FOUND
                    }
                    ServerError::InvalidParams { .. } => {
                        roo_jsonrpc::types::error_codes::INVALID_PARAMS
                    }
                    ServerError::NotInitialized | ServerError::AlreadyInitialized => -32000,
                    ServerError::ShutDown => -32001,
                    _ => roo_jsonrpc::types::error_codes::INTERNAL_ERROR,
                };
                Message::error_response(id, code, &e.to_string())
            }
        }
    }

    // ── Notification helpers ────────────────────────────────────────────

    /// Register an event listener on the given lifecycle that forwards
    /// events as JSON-RPC notifications to the client.
    ///
    /// Source: TS `postStateToWebview()` — forwards task state to the webview
    fn register_event_forwarder(&self, lifecycle: &TaskLifecycle) {
        let notifications = self.pending_notifications.clone();
        let task_id = lifecycle.task_id().to_string();

        lifecycle.engine().emitter().on(move |event| {
            let notification = task_event_to_notification(event, &task_id);
            if let Some(msg) = notification {
                notifications
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(msg);
            }
        });
    }

    /// Drain all pending notifications, returning them and clearing the queue.
    ///
    /// The server calls this after each request-response cycle to forward
    /// any queued task events to the client.
    pub fn drain_notifications(&self) -> Vec<Message> {
        let mut guard = self
            .pending_notifications
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    // ── Lifecycle ───────────────────────────────────────────────────────

    async fn handle_initialize(&self, _params: Value) -> ServerResult<Value> {
        info!("Initializing server");
        let mut app = self.app.write().await;
        app.initialize().await?;
        let state = app.state().await;
        Ok(json!({
            "initialized": state.initialized,
            "mode": state.current_mode,
            "cwd": app.cwd(),
        }))
    }

    async fn handle_shutdown(&self, _params: Value) -> ServerResult<Value> {
        info!("Shutting down server");
        let app = self.app.read().await;
        app.dispose().await?;
        Ok(json!(null))
    }

    async fn handle_ping(&self, _params: Value) -> ServerResult<Value> {
        Ok(json!("pong"))
    }

    // ── Task commands ───────────────────────────────────────────────────

    /// R10-A — Create a TaskLifecycle, store in TaskManager, start the task.
    ///
    /// Source: TS `startTask()` — creates a new Task and initiates the loop
    async fn handle_task_start(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let mode = params
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("code");
        let images: Vec<String> = params
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        info!(
            mode = mode,
            text_len = text.len(),
            images = images.len(),
            "Starting new task"
        );

        let task_id = generate_task_id();
        let cwd = {
            let app = self.app.read().await;
            app.cwd().to_string()
        };

        // Create a TaskLifecycle via the App so services (MCP Hub, Terminal
        // Registry, Message Queue, Telemetry) are wired in automatically.
        let mut task_config = roo_task::types::TaskConfig::new(&task_id, &cwd);
        task_config.mode = mode.to_string();
        task_config.task_text = if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        };
        task_config.images = images;

        let lifecycle = {
            let app = self.app.read().await;
            match app.create_task_lifecycle(task_config) {
                Ok(lc) => lc,
                Err(e) => {
                    error!(error = %e, "Failed to create task lifecycle");
                    return Ok(json!({
                        "taskId": task_id,
                        "mode": mode,
                        "status": "error",
                        "error": e.to_string(),
                    }));
                }
            }
        };

        // Register event forwarder before storing
        self.register_event_forwarder(&lifecycle);

        // Store lifecycle in TaskManager, set as active task
        self.task_manager.create_task(task_id.clone(), lifecycle);

        // Build the API provider from the app's provider settings and inject
        // it into the lifecycle so that `start()` can spawn the AgentLoop.
        let lifecycle_arc = self.task_manager.get_task(&task_id).ok_or_else(|| {
            ServerError::Internal(format!(
                "task `{task_id}` missing immediately after creation"
            ))
        })?;
        {
            let app = self.app.read().await;
            let settings = app.provider_settings();
            if let Ok(provider) = roo_provider::handler::build_api_handler(settings) {
                let mut lc = lifecycle_arc.lock().await;
                lc.set_provider(provider);
            } else {
                debug!("No API provider available; task will start without AgentLoop");
            }
        }

        // Now start the task via TaskLifecycle
        let mut lc = lifecycle_arc.lock().await;
        match lc.start().await {
            Ok(()) => Ok(json!({
                "taskId": task_id,
                "mode": mode,
                "status": "started",
            })),
            Err(e) => {
                error!(error = %e, "Failed to start task lifecycle");
                Ok(json!({
                    "taskId": task_id,
                    "mode": mode,
                    "status": "error",
                    "error": e.to_string(),
                }))
            }
        }
    }

    /// R10-A — Cancel the active or specified task.
    ///
    /// Source: TS `cancelCurrentRequest()` — aborts the current API request
    async fn handle_task_cancel(&self, params: Value) -> ServerResult<Value> {
        let task_id = params.get("taskId").and_then(|v| v.as_str());
        info!(task_id = task_id, "Cancelling task");

        let lifecycle_arc = match task_id {
            Some(id) => self.task_manager.get_task(id),
            None => self.task_manager.get_active_task(),
        };

        match lifecycle_arc {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                // Use TaskLifecycle::cancel_current_request()
                lc.cancel_current_request();
                let tid = lc.task_id().to_string();
                let state = lc.state();
                Ok(json!({
                    "taskId": tid,
                    "status": format!("{}", state).to_lowercase(),
                }))
            }
            None => Ok(json!({"status": "cancelled", "note": "no active task found"})),
        }
    }

    /// R10-A — Close and abort a task, then remove from TaskManager.
    ///
    /// Source: TS `abortTask()` + `dispose()` — clean up and remove
    async fn handle_task_close(&self, params: Value) -> ServerResult<Value> {
        let task_id = params.get("taskId").and_then(|v| v.as_str());
        info!(task_id = task_id, "Closing task");

        match task_id {
            Some(id) => {
                // First abort and dispose the lifecycle
                if let Some(lifecycle) = self.task_manager.get_task(id) {
                    let mut lc = lifecycle.lock().await;
                    // Abort the task (graceful abort, not abandoned)
                    let _ = lc.abort_task(false).await;
                    lc.dispose();
                }
                // Then remove from manager
                let removed = self.task_manager.remove_task(id);
                if removed.is_some() {
                    Ok(json!({"taskId": id, "status": "closed"}))
                } else {
                    Ok(json!({"taskId": id, "status": "not_found"}))
                }
            }
            None => {
                // Close the active task
                let active = self.task_manager.get_active_task();
                match active {
                    Some(lifecycle) => {
                        let id = {
                            let mut lc = lifecycle.lock().await;
                            let id = lc.task_id().to_string();
                            let _ = lc.abort_task(false).await;
                            lc.dispose();
                            id
                        };
                        self.task_manager.remove_task(&id);
                        Ok(json!({"taskId": id, "status": "closed"}))
                    }
                    None => Ok(json!({"status": "no_active_task"})),
                }
            }
        }
    }

    /// R10-A — Resume a paused task or resume from history.
    ///
    /// Source: TS `resumeTaskFromHistory()` — loads history and resumes
    async fn handle_task_resume(&self, params: Value) -> ServerResult<Value> {
        let task_id = params.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
        let history_item_id = params.get("historyItemId").and_then(|v| v.as_str());
        info!(task_id = task_id, "Resuming task");

        let lifecycle_arc = if task_id.is_empty() {
            self.task_manager.get_active_task()
        } else {
            self.task_manager.get_task(task_id)
        };

        match lifecycle_arc {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let tid = lc.task_id().to_string();

                if history_item_id.is_some() {
                    // Resume from history — use TaskLifecycle::resume_task_from_history()
                    // Note: history_item_id should already be set in the config
                    // before the lifecycle was created. If not, we set it here.
                    match lc.resume_task_from_history().await {
                        Ok(()) => {
                            drop(lc);
                            self.task_manager.set_active_task(&tid);
                            Ok(json!({"taskId": tid, "status": "resumed"}))
                        }
                        Err(e) => {
                            Ok(json!({"taskId": tid, "status": "error", "error": e.to_string()}))
                        }
                    }
                } else {
                    // Simple resume from paused state — use engine state transition
                    match lc.engine_mut().resume() {
                        Ok(state) => {
                            drop(lc);
                            self.task_manager.set_active_task(&tid);
                            Ok(
                                json!({"taskId": tid, "status": format!("{}", state).to_lowercase()}),
                            )
                        }
                        Err(e) => {
                            Ok(json!({"taskId": tid, "status": "error", "error": e.to_string()}))
                        }
                    }
                }
            }
            None => Ok(json!({"taskId": task_id, "status": "not_found"})),
        }
    }

    /// R10-A — Send a message to the active task's conversation.
    ///
    /// Source: TS `submitUserMessage()` — handles user response to an ask
    async fn handle_task_send_message(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let images: Vec<String> = params
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        info!(
            text_len = text.len(),
            images = images.len(),
            "Sending message to task"
        );

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                // Use TaskLifecycle::submit_user_message()
                match lc
                    .submit_user_message(
                        text,
                        if images.is_empty() {
                            None
                        } else {
                            Some(images)
                        },
                        None,
                        None,
                    )
                    .await
                {
                    Ok(()) => Ok(json!({"status": "sent"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// M9 — Discover slash commands from project/global directories.
    async fn handle_task_get_commands(&self, _params: Value) -> ServerResult<Value> {
        let cwd = {
            let app = self.app.read().await;
            app.cwd().to_string()
        };

        let mut commands: HashMap<String, roo_command::types::Command> = HashMap::new();

        // Scan project commands directory (.roo/commands)
        let project_commands_dir = std::path::Path::new(&cwd).join(".roo").join("commands");
        if project_commands_dir.exists() {
            roo_command::scanner::scan_command_directory(
                &project_commands_dir,
                roo_command::types::CommandSource::Project,
                &mut commands,
            )
            .await;
        }

        let command_list: Vec<Value> = commands
            .values()
            .map(|cmd| {
                json!({
                    "name": cmd.name,
                    "description": cmd.description,
                    "source": format!("{:?}", cmd.source),
                })
            })
            .collect();

        Ok(json!({"commands": command_list}))
    }

    async fn handle_task_get_modes(&self, _params: Value) -> ServerResult<Value> {
        let modes = roo_types::mode::default_modes();
        let mode_list: Vec<Value> = modes
            .iter()
            .map(|m| json!({"slug": m.slug, "name": m.name}))
            .collect();
        Ok(json!({"modes": mode_list}))
    }

    /// M10 — Get current provider model info.
    async fn handle_task_get_models(&self, _params: Value) -> ServerResult<Value> {
        let app = self.app.read().await;
        let settings = app.provider_settings();
        let model_id = settings.api_model_id.as_deref().unwrap_or("unknown");
        Ok(json!({"models": {"current": model_id}}))
    }

    async fn handle_task_delete_queued_message(&self, params: Value) -> ServerResult<Value> {
        let message_id = params
            .get("messageId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::TASK_DELETE_QUEUED_MESSAGE.to_string(),
                detail: "Missing messageId".to_string(),
            })?;

        let app = self.app.read().await;
        match app.message_queue() {
            Some(queue) => {
                let mut q = queue.lock().await;
                let removed = q.remove_message(message_id);
                if removed {
                    Ok(json!({"status": "deleted", "messageId": message_id}))
                } else {
                    Ok(
                        json!({"status": "error", "error": "message not found", "messageId": message_id}),
                    )
                }
            }
            None => Ok(json!({"status": "error", "error": "message queue not available"})),
        }
    }

    /// R10-A — Condense the active task's context.
    ///
    /// Source: TS `condenseContext()` — manually trigger context condensation
    async fn handle_task_condense(&self, _params: Value) -> ServerResult<Value> {
        info!("Condensing task context");

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                // Use TaskLifecycle::condense_context()
                match lc.condense_context().await {
                    Ok(()) => {
                        let history_len = lc.engine().api_conversation_history().len();
                        Ok(json!({
                            "status": "condensed",
                            "historyLength": history_len,
                        }))
                    }
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// Clear the current task session.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `clearTask`
    async fn handle_task_clear(&self, _params: Value) -> ServerResult<Value> {
        info!("Clearing current task");
        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let id = {
                    let mut lc = lifecycle.lock().await;
                    let id = lc.task_id().to_string();
                    let _ = lc.abort_task(false).await;
                    lc.dispose();
                    id
                };
                self.task_manager.remove_task(&id);
                Ok(json!({"status": "cleared", "taskId": id}))
            }
            None => Ok(json!({"status": "no_active_task"})),
        }
    }

    /// Cancel any pending auto-approval timeout for the current task.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `cancelAutoApproval`
    /// TS: `provider.getCurrentTask()?.cancelAutoApprovalTimeout()`
    async fn handle_task_cancel_auto_approval(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cancelling auto-approval");
        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                if let Some(handler) = lc.auto_approval_handler_mut() {
                    handler.reset_request_count();
                }
                Ok(json!({"status": "cancelled"}))
            }
            None => Ok(json!({"status": "cancelled", "warning": "no active task"})),
        }
    }

    /// Get aggregated costs for a task including subtasks.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getTaskWithAggregatedCosts`
    async fn handle_task_get_aggregated_costs(&self, params: Value) -> ServerResult<Value> {
        let task_id = params.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
        debug!(task_id = task_id, "Getting aggregated costs");

        if task_id.is_empty() {
            return Ok(json!({"error": "missing taskId"}));
        }

        let global_storage_path = {
            let app = self.app.read().await;
            let config = app.config();
            if config.global_storage_path.is_empty() {
                config.cwd.clone()
            } else {
                config.global_storage_path.clone()
            }
        };

        let fs = roo_task_persistence::storage::OsFileSystem;
        let storage_path = Path::new(&global_storage_path);

        match roo_task_persistence::history::get_history_item(&fs, storage_path, task_id) {
            Ok(Some(item)) => {
                // Return the task's own cost. Child task aggregation requires
                // scanning all tasks with parent_task_id == this task_id.
                let own_cost = item.total_cost;
                let mut children_cost = 0.0;
                let mut child_count = 0;

                // Scan for child tasks
                if let Ok(all_items) =
                    roo_task_persistence::history::list_history(&fs, storage_path)
                {
                    for child in &all_items {
                        if child.parent_task_id.as_deref() == Some(task_id) {
                            children_cost += child.total_cost;
                            child_count += 1;
                        }
                    }
                }

                Ok(json!({
                    "taskId": task_id,
                    "ownCost": own_cost,
                    "childrenCost": children_cost,
                    "totalCost": own_cost + children_cost,
                    "childCount": child_count,
                }))
            }
            Ok(None) => Ok(json!({"taskId": task_id, "error": "task not found"})),
            Err(e) => Ok(json!({"taskId": task_id, "error": e.to_string()})),
        }
    }

    /// Show task with a specific ID.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `showTaskWithId`
    async fn handle_task_show_with_id(&self, params: Value) -> ServerResult<Value> {
        let task_id = params
            .get("taskId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::TASK_SHOW_WITH_ID.to_string(),
                detail: "Missing taskId".to_string(),
            })?;
        debug!(task_id = task_id, "Showing task with ID");

        // Set the task as active if it exists
        match self.task_manager.get_task(task_id) {
            Some(_) => {
                self.task_manager.set_active_task(task_id);
                Ok(json!({"status": "shown", "taskId": task_id}))
            }
            None => Ok(json!({"status": "not_found", "taskId": task_id})),
        }
    }

    // ── State commands ──────────────────────────────────────────────────

    async fn handle_state_get(&self, _params: Value) -> ServerResult<Value> {
        let app = self.app.read().await;
        let state = app.state().await;
        let task_count = self.task_manager.list_tasks().len();
        let has_active = self.task_manager.get_active_task().is_some();
        Ok(json!({
            "initialized": state.initialized,
            "mode": state.current_mode,
            "activeTaskCount": task_count,
            "taskRunning": has_active,
            "disposed": state.disposed,
            "cwd": app.cwd(),
            "mcpEnabled": app.mcp_hub().is_some(),
        }))
    }

    async fn handle_state_set_mode(&self, params: Value) -> ServerResult<Value> {
        let mode = params.get("mode").and_then(|v| v.as_str()).ok_or_else(|| {
            ServerError::InvalidParams {
                method: methods::STATE_SET_MODE.to_string(),
                detail: "Missing mode".to_string(),
            }
        })?;
        let app = self.app.read().await;
        app.set_mode(mode).await;
        Ok(json!({"mode": mode}))
    }

    async fn handle_system_prompt_build(&self, _params: Value) -> ServerResult<Value> {
        let app = self.app.read().await;
        let prompt = app.build_system_prompt();
        Ok(json!({"prompt": prompt}))
    }

    // ── History commands ────────────────────────────────────────────────

    async fn handle_history_get(&self, params: Value) -> ServerResult<Value> {
        let task_id = params.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
        debug!(task_id = task_id, "Getting task history");

        let global_storage_path = {
            let app = self.app.read().await;
            let config = app.config();
            if config.global_storage_path.is_empty() {
                config.cwd.clone()
            } else {
                config.global_storage_path.clone()
            }
        };

        let fs = roo_task_persistence::storage::OsFileSystem;
        let storage_path = Path::new(&global_storage_path);

        if task_id.is_empty() {
            match roo_task_persistence::history::list_history(&fs, storage_path) {
                Ok(items) => {
                    let history: Vec<Value> = items
                        .iter()
                        .map(|item| json!({"id": item.id, "task": item.task, "ts": item.timestamp}))
                        .collect();
                    Ok(json!({"taskId": task_id, "history": history}))
                }
                Err(e) => {
                    debug!(error = %e, "Failed to list history");
                    Ok(json!({"taskId": task_id, "history": []}))
                }
            }
        } else {
            match roo_task_persistence::history::get_history_item(&fs, storage_path, task_id) {
                Ok(Some(item)) => Ok(json!({"taskId": task_id, "history": [json!(item)]})),
                Ok(None) => Ok(json!({"taskId": task_id, "history": []})),
                Err(e) => {
                    debug!(error = %e, "Failed to get history item");
                    Ok(json!({"taskId": task_id, "history": []}))
                }
            }
        }
    }

    async fn handle_history_delete(&self, params: Value) -> ServerResult<Value> {
        let task_id = params
            .get("taskId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::HISTORY_DELETE.to_string(),
                detail: "Missing taskId".to_string(),
            })?;

        let global_storage_path = {
            let app = self.app.read().await;
            let config = app.config();
            if config.global_storage_path.is_empty() {
                config.cwd.clone()
            } else {
                config.global_storage_path.clone()
            }
        };

        let fs = roo_task_persistence::storage::OsFileSystem;
        match roo_task_persistence::history::delete_task(
            &fs,
            Path::new(&global_storage_path),
            task_id,
        ) {
            Ok(()) => {
                info!(task_id = task_id, "Deleted task");
                Ok(json!({"status": "deleted"}))
            }
            Err(e) => {
                error!(error = %e, "Failed to delete task");
                Ok(json!({"status": "error", "error": e.to_string()}))
            }
        }
    }

    /// Delete multiple tasks by ID.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteMultipleTasksWithIds`
    async fn handle_history_delete_multiple(&self, params: Value) -> ServerResult<Value> {
        let ids: Vec<String> = params
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if ids.is_empty() {
            return Ok(json!({"status": "error", "error": "missing ids"}));
        }

        let global_storage_path = {
            let app = self.app.read().await;
            let config = app.config();
            if config.global_storage_path.is_empty() {
                config.cwd.clone()
            } else {
                config.global_storage_path.clone()
            }
        };

        let fs = roo_task_persistence::storage::OsFileSystem;
        let mut deleted = Vec::new();
        let mut errors = Vec::new();

        for id in &ids {
            match roo_task_persistence::history::delete_task(
                &fs,
                Path::new(&global_storage_path),
                id,
            ) {
                Ok(()) => {
                    deleted.push(id.clone());
                    // Also remove from TaskManager if present
                    self.task_manager.remove_task(id);
                }
                Err(e) => {
                    errors.push(json!({"id": id, "error": e.to_string()}));
                }
            }
        }

        Ok(json!({
            "status": if errors.is_empty() { "deleted" } else { "partial" },
            "deletedCount": deleted.len(),
            "errorCount": errors.len(),
            "errors": errors,
        }))
    }

    async fn handle_history_export(&self, params: Value) -> ServerResult<Value> {
        let task_id = params
            .get("taskId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::HISTORY_EXPORT.to_string(),
                detail: "Missing taskId".to_string(),
            })?;

        let global_storage_path = {
            let app = self.app.read().await;
            let config = app.config();
            if config.global_storage_path.is_empty() {
                config.cwd.clone()
            } else {
                config.global_storage_path.clone()
            }
        };

        let fs = roo_task_persistence::storage::OsFileSystem;
        let messages_path = Path::new(&global_storage_path)
            .join("tasks")
            .join(task_id)
            .join("messages.json");
        match roo_task_persistence::messages::read_task_messages(&fs, &messages_path) {
            Ok(messages) => Ok(json!({"taskId": task_id, "data": messages})),
            Err(e) => {
                debug!(error = %e, "Failed to export task");
                Ok(json!({"taskId": task_id, "data": null, "error": e.to_string()}))
            }
        }
    }

    // ── Todo ─────────────────────────────────────────────────────────────

    async fn handle_todo_update(&self, params: Value) -> ServerResult<Value> {
        let todos = params.get("todos").cloned().unwrap_or(Value::Null);
        let task_id = params
            .get("taskId")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        debug!(task_id = task_id, "Updating todo list");

        let app = self.app.read().await;
        let mut todo_map = app.todos().write().await;
        todo_map.insert(task_id.to_string(), todos.clone());

        Ok(json!({"status": "updated", "todos": todos}))
    }

    // ── Ask response ────────────────────────────────────────────────────

    /// R10-A — Handle user response to an ask_followup_question.
    ///
    /// Source: TS `handleWebviewAskResponse()` — processes the user's
    /// response to an ask prompt via AskSayHandler::handle_response()
    async fn handle_ask_response(&self, params: Value) -> ServerResult<Value> {
        let ask_response_str = params
            .get("askResponse")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let images: Vec<String> = params
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(ask_response = ask_response_str, "Processing ask response");

        // Map the string response to AskResponse enum
        let ask_response = match ask_response_str {
            "yesButtonClicked" | "yes" => AskResponse::YesButtonClicked,
            "noButtonClicked" | "no" => AskResponse::NoButtonClicked,
            "objectResponse" => AskResponse::ObjectResponse,
            _ => AskResponse::MessageResponse,
        };

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let lc = lifecycle.lock().await;
                // Use AskSayHandler::handle_response()
                lc.ask_say()
                    .handle_response(
                        ask_response,
                        if text.is_empty() {
                            None
                        } else {
                            Some(text.to_string())
                        },
                        if images.is_empty() {
                            None
                        } else {
                            Some(images)
                        },
                    )
                    .await;
                Ok(json!({"status": "responded"}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    // ── Terminal ─────────────────────────────────────────────────────────

    /// M12 — Execute a terminal operation.
    async fn handle_terminal_operation(&self, params: Value) -> ServerResult<Value> {
        let operation = params
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("continue");
        debug!(operation = operation, "Terminal operation");

        let app = self.app.read().await;
        match app.terminal_registry() {
            Some(registry) => {
                match operation {
                    "execute" | "run" => {
                        let command = params.get("command").and_then(|v| v.as_str()).unwrap_or("");
                        if command.is_empty() {
                            return Ok(json!({"status": "error", "error": "missing command"}));
                        }

                        let cwd = app.cwd();
                        let terminal_id = registry.create_terminal(cwd).await;
                        match registry.get_terminal(terminal_id).await {
                            Some(terminal) => {
                                let guard = terminal.lock().await;
                                use roo_terminal::RooTerminal;
                                match guard
                                    .run_command(command, &roo_terminal::NoopCallbacks)
                                    .await
                                {
                                    Ok(result) => Ok(json!({
                                        "status": "ok",
                                        "exitCode": result.exit_code,
                                        "output": result.stdout,
                                    })),
                                    Err(e) => {
                                        let err_msg: String = e.to_string();
                                        Ok(json!({"status": "error", "error": err_msg}))
                                    }
                                }
                            }
                            None => {
                                Ok(json!({"status": "error", "error": "failed to create terminal"}))
                            }
                        }
                    }
                    "continue" => {
                        // Source: TS line 1633 — `this.terminalProcess?.continue()`
                        // Delegate to TaskLifecycle::handle_terminal_operation
                        // which emits TaskUnpaused event and handles terminal continue
                        drop(app);
                        match self.task_manager.get_active_task() {
                            Some(lifecycle) => {
                                let mut lc = lifecycle.lock().await;
                                if let Err(e) = lc.handle_terminal_operation("continue").await {
                                    warn!(error = %e, "Failed to continue terminal operation");
                                }
                            }
                            None => {
                                debug!("No active task for terminal continue");
                            }
                        }
                        Ok(json!({"status": "ok"}))
                    }
                    _ => Ok(json!({"status": "ok", "operation": operation})),
                }
            }
            None => Ok(json!({"status": "error", "error": "terminal registry not initialized"})),
        }
    }

    // ── Checkpoint ───────────────────────────────────────────────────────

    /// C6 — Get checkpoint diff.
    async fn handle_checkpoint_diff(&self, params: Value) -> ServerResult<Value> {
        let commit_hash = params
            .get("commitHash")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Get active task to determine task_id and workspace
        let (task_id, cwd) = {
            match self.task_manager.get_active_task() {
                Some(lifecycle) => {
                    let lc = lifecycle.lock().await;
                    (lc.task_id().to_string(), lc.engine().config().cwd.clone())
                }
                None => {
                    let app = self.app.read().await;
                    ("".to_string(), app.cwd().to_string())
                }
            }
        };

        if task_id.is_empty() {
            return Ok(json!({"diff": [], "error": "no active task for checkpoint"}));
        }

        // Build checkpoint directory path
        let checkpoints_dir = std::path::Path::new(&cwd)
            .join(".roo")
            .join("checkpoints")
            .join(&task_id);

        match roo_checkpoint::service::ShadowCheckpointService::new(
            &task_id,
            &checkpoints_dir,
            &cwd,
            None,
        ) {
            Ok(mut service) => {
                // Initialize the shadow git repo
                if let Err(e) = service.init_shadow_git().await {
                    let err_msg: String = e.to_string();
                    return Ok(json!({"diff": [], "error": err_msg}));
                }

                let diff_params = roo_checkpoint::types::GetDiffParams {
                    from: Some(commit_hash.to_string()),
                    to: None,
                };

                match service.get_diff(diff_params).await {
                    Ok(diffs) => {
                        let diff_list: Vec<Value> = diffs
                            .iter()
                            .map(|d| {
                                json!({
                                    "path": d.paths.relative,
                                    "before": d.content.before,
                                    "after": d.content.after,
                                })
                            })
                            .collect();
                        Ok(json!({"diff": diff_list}))
                    }
                    Err(e) => Ok(json!({"diff": [], "error": e.to_string()})),
                }
            }
            Err(e) => Ok(json!({"diff": [], "error": e.to_string()})),
        }
    }

    /// C7 — Restore checkpoint.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `checkpointRestore`
    /// TS: cancels the current task, restores checkpoint, then re-initializes.
    async fn handle_checkpoint_restore(&self, params: Value) -> ServerResult<Value> {
        let commit_hash = params
            .get("commitHash")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let timestamp = params.get("timestamp").and_then(|v| v.as_f64());

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                match lc.checkpoint_restore(commit_hash, timestamp).await {
                    Ok(()) => Ok(json!({"status": "restored"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => {
                // No active task — try using a standalone checkpoint service
                let app = self.app.read().await;
                let cwd = app.cwd().to_string();
                drop(app);

                let checkpoints_dir = std::path::Path::new(&cwd).join(".roo").join("checkpoints");

                // Try to find any task directory
                if let Ok(entries) = std::fs::read_dir(&checkpoints_dir) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let task_id = entry.file_name().to_string_lossy().to_string();
                            if let Ok(mut service) =
                                roo_checkpoint::service::ShadowCheckpointService::new(
                                    &task_id,
                                    &entry.path(),
                                    &cwd,
                                    None,
                                )
                            {
                                if service.init_shadow_git().await.is_ok() {
                                    match service.restore_checkpoint(commit_hash).await {
                                        Ok(()) => return Ok(json!({"status": "restored"})),
                                        Err(e) => {
                                            return Ok(
                                                json!({"status": "error", "error": e.to_string()}),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(json!({"status": "error", "error": "no active task for checkpoint"}))
            }
        }
    }

    // ── Prompt enhancement ──────────────────────────────────────────────

    /// C8 — Enhance a prompt using the provider's complete_prompt.
    async fn handle_prompt_enhance(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
            ServerError::InvalidParams {
                method: methods::PROMPT_ENHANCE.to_string(),
                detail: "Missing text".to_string(),
            }
        })?;
        debug!(text_len = text.len(), "Enhancing prompt");

        // Build an enhancement prompt wrapping the user's input.
        // Source: TS webviewMessageHandler.ts ~line 1677
        let enhancement_prompt = format!(
            "Enhance the following user prompt for clarity, specificity, and effectiveness. \
             Return ONLY the enhanced prompt text, nothing else.\n\n\
             Original prompt:\n{}",
            text
        );

        // Try to use the provider's complete_prompt for actual enhancement.
        let app = self.app.read().await;
        let settings = app.provider_settings();

        match roo_provider::handler::build_api_handler(settings) {
            Ok(provider) => match provider.complete_prompt(&enhancement_prompt).await {
                Ok(enhanced) => Ok(json!({"enhancedText": enhanced})),
                Err(e) => {
                    warn!(error = %e, "Provider complete_prompt failed, returning original");
                    Ok(json!({"enhancedText": text}))
                }
            },
            Err(_) => {
                // Provider not available — return the original text with a note
                debug!("No provider available for prompt enhancement");
                Ok(json!({"enhancedText": text}))
            }
        }
    }

    // ── Search ───────────────────────────────────────────────────────────

    async fn handle_search_files(&self, params: Value) -> ServerResult<Value> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let file_pattern = params.get("filePattern").and_then(|v| v.as_str());

        debug!(query = query, path = path, "Searching files");

        let search_path = if path.is_empty() {
            let app = self.app.read().await;
            app.cwd().to_string()
        } else {
            path.to_string()
        };

        let search_params = roo_types::tool::SearchFilesParams {
            path: search_path.clone(),
            regex: query.to_string(),
            file_pattern: file_pattern.map(|s| s.to_string()),
        };

        match roo_tools_search::search_files::validate_search_files_params(&search_params) {
            Ok(()) => {
                match roo_tools_search::search_files::search_files(
                    &search_params,
                    Path::new(&search_path),
                ) {
                    Ok(result) => {
                        let match_list: Vec<Value> = result
                            .iter()
                            .map(|m| {
                                json!({
                                    "file": m.file_path,
                                    "line": m.line_number,
                                    "content": m.line_content,
                                })
                            })
                            .collect();
                        Ok(json!({"results": match_list}))
                    }
                    Err(e) => {
                        debug!(error = %e, "Search failed");
                        Ok(json!({"results": [], "error": e.to_string()}))
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "Invalid search params");
                Ok(json!({"results": [], "error": e.to_string()}))
            }
        }
    }

    // ── File read ────────────────────────────────────────────────────────

    async fn handle_file_read(&self, params: Value) -> ServerResult<Value> {
        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            ServerError::InvalidParams {
                method: methods::FILE_READ.to_string(),
                detail: "Missing path".to_string(),
            }
        })?;

        debug!(path = path, "Reading file content");
        match tokio::fs::read_to_string(path).await {
            Ok(content) => Ok(json!({"path": path, "content": content})),
            Err(e) => Ok(json!({"path": path, "content": null, "error": e.to_string()})),
        }
    }

    // ── Git commands ──────────────────────────────────────────────────────

    /// Search git commits.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `searchCommits`
    async fn handle_git_search_commits(&self, params: Value) -> ServerResult<Value> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        debug!(query = query, "Searching git commits");

        let cwd = {
            let app = self.app.read().await;
            app.cwd().to_string()
        };

        // Use git log to search commits
        let output = tokio::process::Command::new("git")
            .args(["log", "--oneline", "--all", "-n", "50", "--grep"])
            .arg(query)
            .current_dir(&cwd)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let commits: Vec<Value> = stdout
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|line| {
                        let parts: Vec<&str> = line.splitn(2, ' ').collect();
                        json!({
                            "hash": parts.first().unwrap_or(&""),
                            "message": parts.get(1).unwrap_or(&""),
                        })
                    })
                    .collect();
                Ok(json!({"commits": commits}))
            }
            Err(e) => Ok(json!({"commits": [], "error": e.to_string()})),
        }
    }

    // ── MCP commands ─────────────────────────────────────────────────────

    async fn handle_mcp_list_servers(&self, _params: Value) -> ServerResult<Value> {
        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                let servers = hub.get_servers();
                let server_list: Vec<Value> = servers
                    .iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "status": format!("{:?}", s.status),
                            "toolCount": s.tools.len(),
                        })
                    })
                    .collect();
                Ok(json!({"servers": server_list}))
            }
            None => Ok(json!({"servers": [], "error": "MCP hub not initialized"})),
        }
    }

    async fn handle_mcp_restart_server(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(server_name = server_name, "Restarting MCP server");

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => match hub.refresh_all_connections().await {
                Ok(()) => Ok(json!({"status": "restarted"})),
                Err(e) => Ok(json!({"status": "error", "error": format!("{}", e)})),
            },
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    async fn handle_mcp_toggle_server(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let disabled = params
            .get("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(
            server_name = server_name,
            disabled = disabled,
            "Toggling MCP server"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                match hub
                    .toggle_server_disabled(
                        server_name,
                        roo_mcp::types::McpSource::Project,
                        disabled,
                    )
                    .await
                {
                    Ok(()) => Ok(json!({"status": "toggled"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    async fn handle_mcp_use_tool(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tool_name = params
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let arguments = params.get("arguments").cloned();
        debug!(
            server_name = server_name,
            tool_name = tool_name,
            "Using MCP tool"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => match hub.call_tool(server_name, tool_name, arguments).await {
                Ok(result) => Ok(json!({"result": result})),
                Err(e) => Ok(json!({"result": null, "error": e.to_string()})),
            },
            None => Ok(json!({"result": null, "error": "MCP hub not initialized"})),
        }
    }

    async fn handle_mcp_access_resource(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        debug!(
            server_name = server_name,
            uri = uri,
            "Accessing MCP resource"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => match hub.read_resource(server_name, uri).await {
                Ok(result) => Ok(json!({"result": result})),
                Err(e) => Ok(json!({"result": null, "error": e.to_string()})),
            },
            None => Ok(json!({"result": null, "error": "MCP hub not initialized"})),
        }
    }

    /// Delete an MCP server configuration.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteMcpServer`
    async fn handle_mcp_delete_server(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::MCP_DELETE_SERVER.to_string(),
                detail: "Missing serverName".to_string(),
            })?;
        debug!(server_name = server_name, "Deleting MCP server");

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                match hub
                    .delete_server(server_name, roo_mcp::types::McpSource::Project)
                    .await
                {
                    Ok(()) => Ok(json!({"status": "deleted"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    /// Update MCP server timeout.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateMcpTimeout`
    async fn handle_mcp_update_timeout(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let timeout = params.get("timeout").and_then(|v| v.as_u64());
        debug!(
            server_name = server_name,
            timeout = timeout,
            "Updating MCP server timeout"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                match hub
                    .update_server_timeout(
                        server_name,
                        timeout.unwrap_or(60),
                        roo_mcp::types::McpSource::Project,
                    )
                    .await
                {
                    Ok(()) => Ok(json!({"status": "updated"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    /// Refresh all MCP server connections.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `refreshAllMcpServers`
    async fn handle_mcp_refresh_all(&self, _params: Value) -> ServerResult<Value> {
        debug!("Refreshing all MCP servers");
        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => match hub.refresh_all_connections().await {
                Ok(()) => Ok(json!({"status": "refreshed"})),
                Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
            },
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    /// Toggle whether a tool is always allowed for an MCP server.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `toggleToolAlwaysAllow`
    async fn handle_mcp_toggle_tool_always_allow(&self, params: Value) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tool_name = params
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let always_allow = params
            .get("alwaysAllow")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(
            server_name = server_name,
            tool_name = tool_name,
            "Toggling tool always allow"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                match hub
                    .toggle_tool_always_allow(
                        server_name,
                        roo_mcp::types::McpSource::Project,
                        tool_name,
                        always_allow,
                    )
                    .await
                {
                    Ok(()) => Ok(json!({"status": "toggled"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    /// Toggle whether a tool is enabled for prompt in an MCP server.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `toggleToolEnabledForPrompt`
    async fn handle_mcp_toggle_tool_enabled_for_prompt(
        &self,
        params: Value,
    ) -> ServerResult<Value> {
        let server_name = params
            .get("serverName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tool_name = params
            .get("toolName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(
            server_name = server_name,
            tool_name = tool_name,
            "Toggling tool enabled for prompt"
        );

        let app = self.app.read().await;
        match app.mcp_hub() {
            Some(hub) => {
                match hub
                    .toggle_tool_enabled_for_prompt(
                        server_name,
                        roo_mcp::types::McpSource::Project,
                        tool_name,
                        enabled,
                    )
                    .await
                {
                    Ok(()) => Ok(json!({"status": "toggled"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "MCP hub not initialized"})),
        }
    }

    // ── Settings commands ────────────────────────────────────────────────

    /// Update application settings.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateSettings`
    async fn handle_settings_update(&self, params: Value) -> ServerResult<Value> {
        debug!("Updating settings");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        // If params contain actual data, merge it; otherwise just acknowledge
        if !params.is_null() && params.as_object().map_or(false, |o| !o.is_empty()) {
            let mut settings = Self::read_roo_settings(&gsp).await;
            if let (Some(existing), Some(incoming)) = (settings.as_object_mut(), params.as_object())
            {
                for (key, value) in incoming {
                    existing.insert(key.clone(), value.clone());
                }
            }
            Self::write_roo_settings(&gsp, &settings).await?;
        }

        Ok(json!({"status": "updated"}))
    }

    /// Save an API configuration.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `saveApiConfiguration`
    async fn handle_settings_save_api_config(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let config = params.get("apiConfiguration").cloned().unwrap_or(json!({}));
        debug!(name = name, "Saving API configuration");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let configs = settings
            .as_object_mut()
            .ok_or_else(|| ServerError::Internal("Settings file is not a JSON object".into()))?
            .entry("apiConfigs")
            .or_insert_with(|| json!({}));
        configs[&name.to_string()] = config.clone();

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "saved", "name": name}))
    }

    /// Load an API configuration by name.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `loadApiConfiguration`
    async fn handle_settings_load_api_config(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        debug!(name = name, "Loading API configuration");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let api_provider = app.provider_settings().api_provider.clone();
        let api_model_id = app.provider_settings().api_model_id.clone();
        drop(app);

        // Try to load from persisted settings first
        let persisted = Self::read_roo_settings(&gsp).await;
        if let Some(cfg) = persisted.get("apiConfigs").and_then(|c| c.get(name)) {
            let mut result = cfg.clone();
            result["name"] = json!(name);
            return Ok(result);
        }

        // Fall back to live provider settings
        Ok(json!({
            "name": name,
            "provider": api_provider,
            "modelId": api_model_id,
        }))
    }

    /// Load an API configuration by ID.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `loadApiConfigurationById`
    async fn handle_settings_load_api_config_by_id(&self, params: Value) -> ServerResult<Value> {
        let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
        debug!(id = id, "Loading API configuration by ID");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let api_provider = app.provider_settings().api_provider.clone();
        let api_model_id = app.provider_settings().api_model_id.clone();
        drop(app);

        // Try to find a config matching the requested ID in persisted settings
        let persisted = Self::read_roo_settings(&gsp).await;
        if let Some(configs) = persisted.get("apiConfigs").and_then(|c| c.as_object()) {
            for (name, cfg) in configs {
                if cfg.get("id").and_then(|v| v.as_str()) == Some(id) {
                    let mut result = cfg.clone();
                    result["name"] = json!(name);
                    result["id"] = json!(id);
                    return Ok(result);
                }
            }
        }

        // Fall back to live provider settings
        Ok(json!({
            "id": id,
            "provider": api_provider,
            "modelId": api_model_id,
        }))
    }

    /// Delete an API configuration.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteApiConfiguration`
    async fn handle_settings_delete_api_config(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        debug!(name = name, "Deleting API configuration");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let removed = if let Some(configs) = settings
            .get_mut("apiConfigs")
            .and_then(|c| c.as_object_mut())
        {
            configs.remove(name).is_some()
        } else {
            false
        };
        Self::write_roo_settings(&gsp, &settings).await?;

        if removed {
            Ok(json!({"status": "deleted", "name": name}))
        } else {
            Ok(json!({"status": "not_found", "name": name}))
        }
    }

    /// List all API configurations.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getListApiConfiguration`
    async fn handle_settings_list_api_configs(&self, _params: Value) -> ServerResult<Value> {
        debug!("Listing API configurations");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let api_provider = app.provider_settings().api_provider.clone();
        let api_model_id = app.provider_settings().api_model_id.clone();
        drop(app);

        // Start with the live provider as the "default" entry
        let mut configs = vec![json!({
            "name": "default",
            "provider": api_provider,
            "modelId": api_model_id,
        })];

        // Merge in any persisted API configs from the settings file
        let persisted = Self::read_roo_settings(&gsp).await;
        if let Some(saved_configs) = persisted.get("apiConfigs").and_then(|v| v.as_object()) {
            for (name, cfg) in saved_configs {
                let mut entry = cfg.clone();
                entry["name"] = json!(name);
                configs.push(entry);
            }
        }

        Ok(json!({"configs": configs}))
    }

    /// Upsert an API configuration.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `upsertApiConfiguration`
    async fn handle_settings_upsert_api_config(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let config = params.get("apiConfiguration").cloned().unwrap_or(json!({}));
        debug!(name = name, "Upserting API configuration");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let configs = settings
            .as_object_mut()
            .ok_or_else(|| ServerError::Internal("Settings file is not a JSON object".into()))?
            .entry("apiConfigs")
            .or_insert_with(|| json!({}));
        let existed = configs.get(name).is_some();
        configs[&name.to_string()] = config;

        Self::write_roo_settings(&gsp, &settings).await?;
        let action = if existed { "updated" } else { "created" };
        Ok(json!({"status": "upserted", "action": action, "name": name}))
    }

    /// Rename an API configuration.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `renameApiConfiguration`
    async fn handle_settings_rename_api_config(&self, params: Value) -> ServerResult<Value> {
        let old_name = params.get("oldName").and_then(|v| v.as_str()).unwrap_or("");
        let new_name = params.get("newName").and_then(|v| v.as_str()).unwrap_or("");
        debug!(
            old_name = old_name,
            new_name = new_name,
            "Renaming API configuration"
        );

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let renamed = if let Some(configs) = settings
            .get_mut("apiConfigs")
            .and_then(|c| c.as_object_mut())
        {
            if let Some(entry) = configs.remove(old_name) {
                configs.insert(new_name.to_string(), entry);
                true
            } else {
                false
            }
        } else {
            false
        };
        Self::write_roo_settings(&gsp, &settings).await?;

        if renamed {
            Ok(json!({"status": "renamed", "oldName": old_name, "newName": new_name}))
        } else {
            Ok(json!({"status": "not_found", "oldName": old_name}))
        }
    }

    /// Update custom instructions.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `customInstructions`
    async fn handle_settings_custom_instructions(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Updating custom instructions");
        let app = self.app.read().await;
        app.set_custom_instructions(text).await;
        Ok(json!({"status": "updated"}))
    }

    /// Update prompt for a specific mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updatePrompt`
    async fn handle_settings_update_prompt(&self, params: Value) -> ServerResult<Value> {
        let prompt_mode = params
            .get("promptMode")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let custom_prompt = params.get("customPrompt").cloned().unwrap_or(Value::Null);
        debug!(prompt_mode = prompt_mode, "Updating prompt for mode");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let prompts = settings
            .as_object_mut()
            .ok_or_else(|| ServerError::Internal("Settings file is not a JSON object".into()))?
            .entry("customPrompts")
            .or_insert_with(|| json!({}));
        prompts[&prompt_mode.to_string()] = custom_prompt;

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "promptMode": prompt_mode}))
    }

    /// Copy system prompt to clipboard.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `copySystemPrompt`
    async fn handle_settings_copy_system_prompt(&self, _params: Value) -> ServerResult<Value> {
        let app = self.app.read().await;
        let prompt = app.build_system_prompt();
        Ok(json!({"prompt": prompt, "copied": true}))
    }

    /// Reset all state.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `resetState`
    async fn handle_settings_reset_state(&self, _params: Value) -> ServerResult<Value> {
        info!("Resetting state");
        let app = self.app.read().await;
        app.reset_state().await?;
        Ok(json!({"status": "reset"}))
    }

    /// Import settings.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `importSettings`
    async fn handle_settings_import_settings(&self, params: Value) -> ServerResult<Value> {
        debug!("Importing settings");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        // The import data is the params itself — merge it into the existing settings
        let mut settings = Self::read_roo_settings(&gsp).await;
        if let (Some(existing), Some(incoming)) = (settings.as_object_mut(), params.as_object()) {
            for (key, value) in incoming {
                existing.insert(key.clone(), value.clone());
            }
        }

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "imported"}))
    }

    /// Export settings.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `exportSettings`
    async fn handle_settings_export_settings(&self, _params: Value) -> ServerResult<Value> {
        debug!("Exporting settings");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let api_provider = app.provider_settings().api_provider.clone();
        let api_model_id = app.provider_settings().api_model_id.clone();
        let provider_info = json!({
            "provider": api_provider,
            "modelId": api_model_id,
        });
        drop(app);

        // Merge persisted settings with the live provider info
        let persisted = Self::read_roo_settings(&gsp).await;
        let mut exported = persisted;
        if let (Some(obj), Some(pobj)) = (exported.as_object_mut(), provider_info.as_object()) {
            for (k, v) in pobj {
                obj.insert(k.clone(), v.clone());
            }
        }

        Ok(json!({"settings": exported}))
    }

    /// Lock API config across modes.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `lockApiConfigAcrossModes`
    async fn handle_settings_lock_api_config(&self, params: Value) -> ServerResult<Value> {
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(enabled = enabled, "Locking API config across modes");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["lockApiConfigAcrossModes"] = json!(enabled);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "enabled": enabled}))
    }

    /// Toggle API config pin.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `toggleApiConfigPin`
    async fn handle_settings_toggle_api_config_pin(&self, params: Value) -> ServerResult<Value> {
        let config_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        debug!(config_name = config_name, "Toggling API config pin");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let pinned = settings
            .pointer_mut(&format!("/apiConfigs/{}/pinned", config_name))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let new_pinned = !pinned;
        if let Some(configs) = settings
            .get_mut("apiConfigs")
            .and_then(|c| c.as_object_mut())
        {
            if let Some(cfg) = configs.get_mut(config_name) {
                cfg["pinned"] = json!(new_pinned);
            } else {
                // Config entry does not exist yet — create a stub with pin state
                configs.insert(config_name.to_string(), json!({"pinned": new_pinned}));
            }
        }

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "toggled", "name": config_name, "pinned": new_pinned}))
    }

    /// Set enhancement API config ID.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `enhancementApiConfigId`
    async fn handle_settings_enhancement_api_config_id(
        &self,
        params: Value,
    ) -> ServerResult<Value> {
        let config_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
        debug!(config_id = config_id, "Setting enhancement API config ID");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["enhancementApiConfigId"] = json!(config_id);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "id": config_id}))
    }

    /// Set auto-approval enabled.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `autoApprovalEnabled`
    async fn handle_settings_auto_approval_enabled(&self, params: Value) -> ServerResult<Value> {
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(enabled = enabled, "Setting auto-approval enabled");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["autoApprovalEnabled"] = json!(enabled);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "enabled": enabled}))
    }

    /// Set debug setting.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `debugSetting`
    async fn handle_settings_debug_setting(&self, params: Value) -> ServerResult<Value> {
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(enabled = enabled, "Setting debug setting");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["debugSetting"] = json!(enabled);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "enabled": enabled}))
    }

    /// Set allowed commands.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `allowedCommands` / `deniedCommands`
    async fn handle_settings_allowed_commands(&self, params: Value) -> ServerResult<Value> {
        let allowed: Vec<String> = params
            .get("allowed")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let denied: Vec<String> = params
            .get("denied")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(allowed = ?allowed, denied = ?denied, "Setting allowed commands");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["allowedCommands"] = json!(allowed);
        settings["deniedCommands"] = json!(denied);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "allowedCount": allowed.len(), "deniedCount": denied.len()}))
    }

    // ── Skills commands ──────────────────────────────────────────────────

    /// List available skills.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestSkills`
    async fn handle_skills_list(&self, _params: Value) -> ServerResult<Value> {
        debug!("Listing skills");
        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mgr = manager.read().await;
                let skills = mgr.get_all_skills();
                let skill_list: Vec<Value> = skills
                    .iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "description": s.description,
                            "source": format!("{:?}", s.source),
                        })
                    })
                    .collect();
                Ok(json!({"skills": skill_list}))
            }
            None => Ok(json!({"skills": []})),
        }
    }

    /// Create a new skill.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `createSkill`
    async fn handle_skills_create(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let source_str = params
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let source = match source_str {
            "global" => roo_skills::SkillSource::Global,
            _ => roo_skills::SkillSource::Project,
        };
        let modes: Option<Vec<String>> = params.get("modes").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
        debug!(name = name, "Creating skill");

        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                let cwd = app.cwd();
                let mode_slugs = modes.as_deref();
                match mgr
                    .create_skill(name, source, description, mode_slugs, cwd, None)
                    .await
                {
                    Ok(meta) => Ok(json!({"status": "created", "name": meta.name})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "skills manager not initialized"})),
        }
    }

    /// Delete a skill.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteSkill`
    async fn handle_skills_delete(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let source_str = params
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let source = match source_str {
            "global" => roo_skills::SkillSource::Global,
            _ => roo_skills::SkillSource::Project,
        };
        debug!(name = name, "Deleting skill");

        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr.delete_skill(name, source, None).await {
                    Ok(()) => Ok(json!({"status": "deleted", "name": name})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "skills manager not initialized"})),
        }
    }

    /// Move a skill.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `moveSkill`
    async fn handle_skills_move(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let source_str = params
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let source = match source_str {
            "global" => roo_skills::SkillSource::Global,
            _ => roo_skills::SkillSource::Project,
        };
        let new_mode = params
            .get("newMode")
            .and_then(|v| v.as_str())
            .unwrap_or("code");
        debug!(name = name, new_mode = new_mode, "Moving skill");

        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                let cwd = app.cwd();
                match mgr.move_skill(name, source, None, new_mode, cwd).await {
                    Ok(meta) => Ok(json!({"status": "moved", "name": meta.name})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "skills manager not initialized"})),
        }
    }

    /// Update skill modes.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateSkillModes`
    async fn handle_skills_update_modes(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let source_str = params
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let source = match source_str {
            "global" => roo_skills::SkillSource::Global,
            _ => roo_skills::SkillSource::Project,
        };
        let modes: Vec<String> = params
            .get("modes")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(name = name, modes = ?modes, "Updating skill modes");

        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr
                    .update_skill_modes(name, source, None, Some(modes.as_slice()))
                    .await
                {
                    Ok(meta) => Ok(json!({"status": "updated", "name": meta.name})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "skills manager not initialized"})),
        }
    }

    // ── Mode commands ────────────────────────────────────────────────────

    /// Update a custom mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateCustomMode`
    async fn handle_mode_update_custom(&self, params: Value) -> ServerResult<Value> {
        let slug = params.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        debug!(slug = slug, "Updating custom mode");

        let app = self.app.read().await;
        match app.custom_modes_manager() {
            Some(manager) => {
                let mut mgr = manager
                    .write()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;

                // Build ModeConfig from params
                let mode_config = roo_types::mode::ModeConfig {
                    slug: slug.to_string(),
                    name: params
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(slug)
                        .to_string(),
                    role_definition: params
                        .get("roleDefinition")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    when_to_use: params
                        .get("whenToUse")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    description: params
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    custom_instructions: params
                        .get("customInstructions")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    groups: vec![],
                    source: params
                        .get("source")
                        .and_then(|v| v.as_str())
                        .map(|s| match s {
                            "project" => roo_types::mode::ModeSource::Project,
                            _ => roo_types::mode::ModeSource::Global,
                        }),
                };

                match mgr.update_custom_mode(slug, mode_config) {
                    Ok(()) => Ok(json!({"status": "updated", "slug": slug})),
                    Err(e) => Ok(json!({"status": "error", "error": e})),
                }
            }
            None => Ok(json!({"status": "error", "error": "custom modes manager not initialized"})),
        }
    }

    /// Delete a custom mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteCustomMode`
    async fn handle_mode_delete_custom(&self, params: Value) -> ServerResult<Value> {
        let slug = params.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        let check_only = params
            .get("checkOnly")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(slug = slug, check_only = check_only, "Deleting custom mode");

        if check_only {
            // TS: sends back deleteCustomModeCheck with rules folder path
            let app = self.app.read().await;
            let rules_path = if let Some(manager) = app.custom_modes_manager() {
                let mgr = manager
                    .read()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;
                let cwd = app.cwd();
                let has_content =
                    mgr.check_rules_directory_has_content(slug, Some(std::path::Path::new(cwd)));
                if has_content {
                    Some(format!(".roo/rules-{slug}/"))
                } else {
                    None
                }
            } else {
                None
            };
            return Ok(json!({"status": "check", "slug": slug, "rulesPath": rules_path}));
        }

        let app = self.app.read().await;
        match app.custom_modes_manager() {
            Some(manager) => {
                let mut mgr = manager
                    .write()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;
                match mgr.delete_custom_mode(slug) {
                    Ok(()) => Ok(json!({"status": "deleted", "slug": slug})),
                    Err(e) => Ok(json!({"status": "error", "error": e})),
                }
            }
            None => Ok(json!({"status": "error", "error": "custom modes manager not initialized"})),
        }
    }

    // ── Message commands ─────────────────────────────────────────────────

    /// Delete a message from the conversation.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteMessage`
    async fn handle_message_delete(&self, params: Value) -> ServerResult<Value> {
        let message_ts = params.get("messageTs").and_then(|v| v.as_u64());
        debug!(message_ts = message_ts, "Deleting message");

        let message_ts = match message_ts {
            Some(ts) => ts,
            None => return Ok(json!({"status": "error", "error": "missing messageTs"})),
        };

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let engine = lc.engine_mut();
                let ts_f64 = message_ts as f64;
                engine.cline_messages_mut().retain(|m| m.ts != ts_f64);
                if let Err(e) = engine.save_cline_messages().await {
                    warn!(error = %e, "Failed to save cline messages after delete");
                }
                Ok(json!({"status": "deleted", "messageTs": message_ts}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// Edit and resubmit a message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `submitEditedMessage`
    async fn handle_message_edit(&self, params: Value) -> ServerResult<Value> {
        let message_ts = params.get("messageTs").and_then(|v| v.as_u64());
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(
            message_ts = message_ts,
            text_len = text.len(),
            "Editing message"
        );

        let message_ts = match message_ts {
            Some(ts) => ts,
            None => return Ok(json!({"status": "error", "error": "missing messageTs"})),
        };

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let engine = lc.engine_mut();
                let ts_f64 = message_ts as f64;
                if let Some(msg) = engine
                    .cline_messages_mut()
                    .iter_mut()
                    .find(|m| m.ts == ts_f64)
                {
                    msg.text = Some(text.to_string());
                }
                if let Err(e) = engine.save_cline_messages().await {
                    warn!(error = %e, "Failed to save cline messages after edit");
                }
                Ok(json!({"status": "edited", "messageTs": message_ts}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// Queue a message for the active task.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `queueMessage`
    async fn handle_message_queue(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let images: Vec<String> = params
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(
            text_len = text.len(),
            images = images.len(),
            "Queueing message"
        );

        let app = self.app.read().await;
        match app.message_queue() {
            Some(queue) => {
                let mut q = queue.lock().await;
                q.add_message(
                    text,
                    if images.is_empty() {
                        None
                    } else {
                        Some(images)
                    },
                );
                Ok(json!({"status": "queued"}))
            }
            None => Ok(json!({"status": "error", "error": "message queue not initialized"})),
        }
    }

    /// Confirm deletion of a message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteMessageConfirm`
    async fn handle_message_delete_confirm(&self, params: Value) -> ServerResult<Value> {
        let message_ts = params
            .get("messageTs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::MESSAGE_DELETE_CONFIRM.to_string(),
                detail: "Missing messageTs".to_string(),
            })?;
        debug!(message_ts = message_ts, "Confirming message deletion");

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let engine = lc.engine_mut();
                let ts_f64 = message_ts as f64;

                let api_truncate_idx = engine
                    .cline_messages()
                    .iter()
                    .position(|m| m.ts == ts_f64)
                    .and_then(|pos| engine.cline_messages()[pos].conversation_history_index);

                if let Some(pos) = engine.cline_messages().iter().position(|m| m.ts == ts_f64) {
                    engine.cline_messages_mut().truncate(pos);
                }

                if let Some(api_idx) = api_truncate_idx {
                    engine.api_conversation_history_mut().truncate(api_idx);
                }

                if let Err(e) = engine.save_cline_messages().await {
                    warn!(error = %e, "Failed to save cline messages after delete confirm");
                }
                if let Err(e) = engine.save_api_conversation_history().await {
                    warn!(error = %e, "Failed to save API history after delete confirm");
                }
                Ok(json!({"status": "deleted", "messageTs": message_ts}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// Confirm editing and resubmitting a message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `editMessageConfirm`
    async fn handle_message_edit_confirm(&self, params: Value) -> ServerResult<Value> {
        let message_ts = params
            .get("messageTs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::MESSAGE_EDIT_CONFIRM.to_string(),
                detail: "Missing messageTs".to_string(),
            })?;
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let _images: Vec<String> = params
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(
            message_ts = message_ts,
            text_len = text.len(),
            "Confirming message edit"
        );

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let engine = lc.engine_mut();
                let ts_f64 = message_ts as f64;

                let msg_pos = engine.cline_messages().iter().position(|m| m.ts == ts_f64);
                let api_truncate_idx =
                    msg_pos.and_then(|pos| engine.cline_messages()[pos].conversation_history_index);

                if let Some(pos) = msg_pos {
                    engine.cline_messages_mut()[pos].text = Some(text.to_string());
                    engine.cline_messages_mut().truncate(pos + 1);
                }

                if let Some(api_idx) = api_truncate_idx {
                    engine.api_conversation_history_mut().truncate(api_idx);
                }

                if let Err(e) = engine.save_cline_messages().await {
                    warn!(error = %e, "Failed to save cline messages after edit confirm");
                }
                if let Err(e) = engine.save_api_conversation_history().await {
                    warn!(error = %e, "Failed to save API history after edit confirm");
                }
                Ok(json!({"status": "edited", "messageTs": message_ts}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    /// Edit a queued message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `editQueuedMessage`
    async fn handle_message_edit_queued(&self, params: Value) -> ServerResult<Value> {
        let message_id = params
            .get("messageId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::MESSAGE_EDIT_QUEUED.to_string(),
                detail: "Missing messageId".to_string(),
            })?;
        let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
            ServerError::InvalidParams {
                method: methods::MESSAGE_EDIT_QUEUED.to_string(),
                detail: "Missing text".to_string(),
            }
        })?;
        let images: Option<Vec<String>> =
            params.get("images").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        debug!(
            message_id = message_id,
            text_len = text.len(),
            "Editing queued message"
        );

        let app = self.app.read().await;
        match app.message_queue() {
            Some(queue) => {
                let mut q = queue.lock().await;
                let updated = q.update_message(message_id, text, images);
                if updated {
                    Ok(json!({"status": "edited", "messageId": message_id}))
                } else {
                    Ok(
                        json!({"status": "error", "error": "message not found", "messageId": message_id}),
                    )
                }
            }
            None => Ok(json!({"status": "error", "error": "message queue not available"})),
        }
    }

    /// Remove a queued message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `removeQueuedMessage`
    async fn handle_message_remove_queued(&self, params: Value) -> ServerResult<Value> {
        let message_id = params
            .get("messageId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ServerError::InvalidParams {
                method: methods::MESSAGE_REMOVE_QUEUED.to_string(),
                detail: "Missing messageId".to_string(),
            })?;

        debug!(message_id = message_id, "Removing queued message");

        let app = self.app.read().await;
        match app.message_queue() {
            Some(queue) => {
                let mut q = queue.lock().await;
                let removed = q.remove_message(message_id);
                if removed {
                    Ok(json!({"status": "removed", "messageId": message_id}))
                } else {
                    Ok(
                        json!({"status": "error", "error": "message not found", "messageId": message_id}),
                    )
                }
            }
            None => Ok(json!({"status": "error", "error": "message queue not available"})),
        }
    }

    // ── Tools commands ────────────────────────────────────────────────────

    /// Refresh custom tools.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `refreshCustomTools`
    async fn handle_tools_refresh_custom(&self, _params: Value) -> ServerResult<Value> {
        debug!("Refreshing custom tools");
        let cwd = self.get_cwd().await;
        let project_tools = std::path::Path::new(&cwd).join(".roo").join("tools");
        let global_dir = roo_config::paths::get_global_roo_directory().join("tools");

        let mut total = 0usize;
        for dir in [&project_tools, &global_dir] {
            if dir.exists() {
                if let Ok(result) = roo_custom_tools::loader::load_from_directory(dir) {
                    total += result.loaded.len();
                }
            }
        }
        Ok(json!({"status": "refreshed", "totalTools": total}))
    }

    // ── Telemetry commands ───────────────────────────────────────────────

    /// Set telemetry setting.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `telemetrySetting`
    async fn handle_telemetry_set_setting(&self, params: Value) -> ServerResult<Value> {
        let setting = params
            .get("setting")
            .and_then(|v| v.as_str())
            .unwrap_or("unset");
        debug!(setting = setting, "Setting telemetry setting");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["telemetrySetting"] = json!(setting);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "setting": setting}))
    }

    // ── Skill (additional) ────────────────────────────────────────────────

    /// Open a skill file.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openSkillFile`
    async fn handle_skill_open_file(&self, params: Value) -> ServerResult<Value> {
        let name = params
            .get("skillName")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("name").and_then(|v| v.as_str()))
            .unwrap_or("");
        debug!(name = name, "Opening skill file");
        let app = self.app.read().await;
        match app.skills_manager() {
            Some(manager) => {
                let mgr = manager.read().await;
                if let Some(skill) = mgr.get_skill(name, roo_skills::SkillSource::Project, None) {
                    Ok(json!({"status": "ok", "path": skill.path}))
                } else {
                    Ok(json!({"status": "not_found", "name": name}))
                }
            }
            None => Ok(json!({"status": "error", "error": "skills manager not available"})),
        }
    }

    // ── Mode (additional) ────────────────────────────────────────────────

    /// Export a custom mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `exportMode`
    async fn handle_mode_export(&self, params: Value) -> ServerResult<Value> {
        let slug = params.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        debug!(slug = slug, "Exporting mode");

        let app = self.app.read().await;
        match app.custom_modes_manager() {
            Some(manager) => {
                let mut mgr = manager
                    .write()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;
                let cwd = app.cwd();
                let result =
                    mgr.export_mode_with_rules(slug, None, Some(std::path::Path::new(cwd)));
                Ok(json!({
                    "status": "exported",
                    "slug": slug,
                    "yaml": result.yaml,
                    "success": result.success,
                    "error": result.error,
                }))
            }
            None => Ok(json!({"status": "error", "error": "custom modes manager not initialized"})),
        }
    }

    /// Import a custom mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `importMode`
    async fn handle_mode_import(&self, params: Value) -> ServerResult<Value> {
        let yaml_content = params.get("yaml").and_then(|v| v.as_str()).unwrap_or("");
        let source_str = params
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let source = match source_str {
            "global" => roo_types::mode::ModeSource::Global,
            _ => roo_types::mode::ModeSource::Project,
        };
        debug!("Importing mode");

        let app = self.app.read().await;
        match app.custom_modes_manager() {
            Some(manager) => {
                let mut mgr = manager
                    .write()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;
                let cwd = app.cwd();
                let result = mgr.import_mode_with_rules(
                    yaml_content,
                    source,
                    Some(std::path::Path::new(cwd)),
                );
                Ok(json!({
                    "status": "imported",
                    "slug": result.slug,
                    "success": result.success,
                    "error": result.error,
                }))
            }
            None => Ok(json!({"status": "error", "error": "custom modes manager not initialized"})),
        }
    }

    /// Switch mode.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `switchMode`
    async fn handle_mode_switch(&self, params: Value) -> ServerResult<Value> {
        let mode = params
            .get("mode")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("text").and_then(|v| v.as_str()))
            .unwrap_or("code");
        debug!(mode = mode, "Switching mode");
        let app = self.app.read().await;
        app.set_mode(mode).await;
        Ok(json!({"status": "switched", "mode": mode}))
    }

    /// Check rules directory.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `checkRulesDirectory`
    async fn handle_mode_check_rules(&self, params: Value) -> ServerResult<Value> {
        let slug = params.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        debug!(slug = slug, "Checking rules directory");

        let app = self.app.read().await;
        match app.custom_modes_manager() {
            Some(manager) => {
                let mgr = manager
                    .read()
                    .map_err(|e| ServerError::Internal(e.to_string()))?;
                let cwd = app.cwd();
                let has_content =
                    mgr.check_rules_directory_has_content(slug, Some(std::path::Path::new(cwd)));
                Ok(json!({"status": "checked", "slug": slug, "hasContent": has_content}))
            }
            None => Ok(json!({"status": "checked", "slug": slug, "hasContent": false})),
        }
    }

    /// Open custom modes settings file.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openCustomModesSettings`
    async fn handle_mode_open_settings(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening custom modes settings");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let cwd = app.cwd().to_string();
        drop(app);
        let global_modes = Self::roo_settings_path(&gsp)
            .parent()
            .map(|p| p.join("roomodes"))
            .unwrap_or_default();
        let project_modes = std::path::Path::new(&cwd).join(".roomodes");
        Ok(json!({
            "action": "openModeSettings",
            "globalModesPath": global_modes.to_string_lossy(),
            "projectModesPath": project_modes.to_string_lossy(),
        }))
    }

    /// Set OpenAI custom model info.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `setopenAiCustomModelInfo`
    async fn handle_mode_set_openai_custom_model_info(
        &self,
        _params: Value,
    ) -> ServerResult<Value> {
        debug!("Setting OpenAI custom model info");
        Ok(json!({"status": "updated"}))
    }

    // ── Message (additional) ─────────────────────────────────────────────

    /// Submit an edited message.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `submitEditedMessage`
    async fn handle_message_submit_edited(&self, params: Value) -> ServerResult<Value> {
        let message_ts = params
            .get("value")
            .and_then(|v| v.as_u64())
            .or_else(|| params.get("messageTs").and_then(|v| v.as_u64()));
        let text = params
            .get("editedMessageContent")
            .or_else(|| params.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(
            message_ts = message_ts,
            text_len = text.len(),
            "Submitting edited message"
        );

        let message_ts = match message_ts {
            Some(ts) => ts,
            None => return Ok(json!({"status": "error", "error": "missing messageTs"})),
        };

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let mut lc = lifecycle.lock().await;
                let engine = lc.engine_mut();
                let ts_f64 = message_ts as f64;

                let msg_pos = engine.cline_messages().iter().position(|m| m.ts == ts_f64);
                let api_truncate_idx =
                    msg_pos.and_then(|pos| engine.cline_messages()[pos].conversation_history_index);

                if let Some(pos) = msg_pos {
                    engine.cline_messages_mut()[pos].text = Some(text.to_string());
                    engine.cline_messages_mut().truncate(pos + 1);
                }

                if let Some(api_idx) = api_truncate_idx {
                    engine.api_conversation_history_mut().truncate(api_idx);
                }

                if let Err(e) = engine.save_cline_messages().await {
                    warn!(error = %e, "Failed to save cline messages after submit edited");
                }
                if let Err(e) = engine.save_api_conversation_history().await {
                    warn!(error = %e, "Failed to save API history after submit edited");
                }
                Ok(json!({"status": "edited", "messageTs": message_ts}))
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    // ── Marketplace ────────────────────────────────────────────────────────

    /// Install a marketplace item.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `installMarketplaceItem`
    async fn handle_marketplace_install(&self, params: Value) -> ServerResult<Value> {
        let item_id = params
            .get("mpItem")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(item_id = item_id, "Installing marketplace item");

        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr.install_item(item_id) {
                    Ok(()) => Ok(json!({"success": true, "slug": item_id})),
                    Err(e) => {
                        Ok(json!({"success": false, "slug": item_id, "error": e.to_string()}))
                    }
                }
            }
            None => Ok(
                json!({"success": false, "slug": item_id, "error": "marketplace manager not initialized"}),
            ),
        }
    }

    /// Remove an installed marketplace item.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `removeInstalledMarketplaceItem`
    async fn handle_marketplace_remove(&self, params: Value) -> ServerResult<Value> {
        let item_id = params
            .get("mpItem")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(item_id = item_id, "Removing marketplace item");

        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr.remove_item(item_id) {
                    Ok(true) => Ok(json!({"success": true, "slug": item_id})),
                    Ok(false) => {
                        Ok(json!({"success": false, "slug": item_id, "error": "item not found"}))
                    }
                    Err(e) => {
                        Ok(json!({"success": false, "slug": item_id, "error": e.to_string()}))
                    }
                }
            }
            None => Ok(
                json!({"success": false, "slug": item_id, "error": "marketplace manager not initialized"}),
            ),
        }
    }

    /// Install marketplace item with parameters.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `installMarketplaceItemWithParameters`
    async fn handle_marketplace_install_with_params(&self, params: Value) -> ServerResult<Value> {
        let item_id = params
            .get("mpItem")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(
            item_id = item_id,
            "Installing marketplace item with parameters"
        );

        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr.install_item(item_id) {
                    Ok(()) => Ok(json!({"success": true, "slug": item_id})),
                    Err(e) => {
                        Ok(json!({"success": false, "slug": item_id, "error": e.to_string()}))
                    }
                }
            }
            None => Ok(json!({"success": false, "error": "marketplace manager not initialized"})),
        }
    }

    /// Fetch marketplace data.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `fetchMarketplaceData`
    async fn handle_marketplace_fetch_data(&self, _params: Value) -> ServerResult<Value> {
        debug!("Fetching marketplace data");

        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mgr = manager.read().await;
                let items = mgr.get_items();
                let item_list: Vec<Value> = items
                    .iter()
                    .map(|item| {
                        json!({
                            "id": item.id,
                            "name": item.name,
                            "type": format!("{:?}", item.item_type),
                        })
                    })
                    .collect();
                Ok(json!({"items": item_list}))
            }
            None => Ok(json!({"items": []})),
        }
    }

    /// Filter marketplace items.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `filterMarketplaceItems`
    async fn handle_marketplace_filter_items(&self, params: Value) -> ServerResult<Value> {
        let filter_type = params
            .get("filters")
            .and_then(|f| f.get("type"))
            .and_then(|v| v.as_str());
        let search = params
            .get("filters")
            .and_then(|f| f.get("search"))
            .and_then(|v| v.as_str());
        debug!(
            filter_type = filter_type,
            search = search,
            "Filtering marketplace items"
        );

        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mgr = manager.read().await;
                let item_type = filter_type.and_then(|t| match t {
                    "mcp" => Some(roo_marketplace::MarketplaceItemType::Mcp),
                    "mode" => Some(roo_marketplace::MarketplaceItemType::Mode),
                    "skill" => Some(roo_marketplace::MarketplaceItemType::Skill),
                    _ => None,
                });
                let filter = roo_marketplace::MarketplaceFilter {
                    item_type,
                    search_query: search.map(|s| s.to_string()),
                    tags: None,
                };
                let filtered = mgr.filter_items(&filter);
                let items: Vec<Value> = filtered
                    .iter()
                    .map(|item| {
                        json!({
                            "id": item.id,
                            "name": item.name,
                            "type": format!("{:?}", item.item_type),
                        })
                    })
                    .collect();
                Ok(json!({"items": items, "status": "filtered"}))
            }
            None => Ok(json!({"items": [], "status": "filtered"})),
        }
    }

    /// Marketplace button clicked.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `marketplaceButtonClicked`
    async fn handle_marketplace_button_clicked(&self, _params: Value) -> ServerResult<Value> {
        debug!("Marketplace button clicked");
        Ok(json!({"status": "acknowledged"}))
    }

    /// Cancel marketplace install.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `cancelMarketplaceInstall`
    async fn handle_marketplace_cancel_install(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cancelling marketplace install");
        let app = self.app.read().await;
        match app.marketplace_manager() {
            Some(manager) => {
                let mut mgr = manager.write().await;
                match mgr.cancel_install() {
                    Ok(()) => Ok(json!({"status": "cancelled"})),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "cancelled", "note": "no marketplace manager"})),
        }
    }

    // ── Worktree ────────────────────────────────────────────────────────────
    // Source: TS `handlers.ts` + `worktree-service.ts` + `worktree-include.ts`

    /// Helper: get the current working directory from the app.
    async fn get_cwd(&self) -> String {
        let app = self.app.read().await;
        app.cwd().to_string()
    }

    /// Get API key for a specific provider from current settings.
    async fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        let app = self.app.read().await;
        let settings = app.provider_settings();
        match provider {
            "anthropic" => settings.api_key.clone(),
            "openai" => settings.open_ai_api_key.clone(),
            "openrouter" => settings.open_router_api_key.clone(),
            "ollama" => settings.ollama_api_key.clone(),
            "deepseek" => settings.deep_seek_api_key.clone(),
            "gemini" => settings
                .gemini_api_key
                .clone()
                .or(settings.google_api_key.clone()),
            "mistral" => settings.mistral_api_key.clone(),
            "minimax" => settings.minimax_api_key.clone(),
            "xai" => settings.xai_api_key.clone(),
            "litellm" => settings.litellm_api_key.clone(),
            "requesty" => settings.requesty_api_key.clone(),
            "openai-native" => settings.open_ai_native_api_key.clone(),
            "poe" => settings.poe_api_key.clone(),
            "roo" => settings.roo_api_key.clone(),
            _ => None,
        }
        .filter(|k| !k.is_empty())
    }

    /// Get base URL for a specific provider from current settings.
    async fn get_base_url_for_provider(&self, provider: &str) -> Option<String> {
        let app = self.app.read().await;
        let settings = app.provider_settings();
        let url = match provider {
            "anthropic" => settings.anthropic_base_url.clone(),
            "openai" => settings.open_ai_base_url.clone(),
            "openrouter" => settings.open_router_base_url.clone(),
            "ollama" => settings.ollama_base_url.clone(),
            "deepseek" => settings.deep_seek_base_url.clone(),
            "gemini" => settings
                .google_gemini_base_url
                .clone()
                .or(settings.gemini_base_url.clone()),
            "mistral" => settings.mistral_base_url.clone(),
            "minimax" => settings.minimax_base_url.clone(),
            "xai" => settings.xai_base_url.clone(),
            "litellm" => settings.litellm_base_url.clone(),
            "lmstudio" => settings.lm_studio_base_url.clone(),
            _ => None,
        };
        url.filter(|u| !u.is_empty())
    }

    /// List git worktrees.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `listWorktrees`
    /// Source: TS `handlers.ts` — `handleListWorktrees`
    async fn handle_worktree_list(&self, _params: Value) -> ServerResult<Value> {
        debug!("Listing worktrees");
        let cwd = self.get_cwd().await;
        let cwd_path = Path::new(&cwd);

        let is_git_repo = roo_worktree::check_git_repo(cwd_path);
        if !is_git_repo {
            return Ok(json!({
                "worktrees": [],
                "isGitRepo": false,
                "isMultiRoot": false,
                "isSubfolder": false,
                "gitRootPath": "",
                "cwd": cwd,
            }));
        }

        let git_root = roo_worktree::get_git_root_path(cwd_path).unwrap_or_default();
        let is_subfolder = roo_worktree::is_workspace_subfolder(&cwd, &git_root);

        match roo_worktree::list_worktrees(cwd_path) {
            Ok(worktrees) => {
                let current = cwd.replace('\\', "/");
                let entries: Vec<Value> = worktrees
                    .iter()
                    .map(|wt| {
                        let wt_path_norm = wt.path.replace('\\', "/");
                        let is_current = wt_path_norm == current
                            || current.starts_with(&format!("{wt_path_norm}/"));
                        json!({
                            "name": wt.branch.as_deref().unwrap_or("detached"),
                            "path": wt.path,
                            "branch": wt.branch,
                            "isCurrent": is_current,
                            "isMainWorktree": wt_path_norm == git_root.replace('\\', "/"),
                            "isDetached": wt.is_detached,
                        })
                    })
                    .collect();
                Ok(json!({
                    "worktrees": entries,
                    "isGitRepo": true,
                    "isMultiRoot": false,
                    "isSubfolder": is_subfolder,
                    "gitRootPath": git_root,
                    "cwd": cwd,
                }))
            }
            Err(e) => Ok(json!({
                "worktrees": [],
                "isGitRepo": true,
                "isMultiRoot": false,
                "isSubfolder": false,
                "gitRootPath": git_root,
                "cwd": cwd,
                "error": e,
            })),
        }
    }

    /// Create a git worktree.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `createWorktree`
    /// Source: TS `handlers.ts` — `handleCreateWorktree`
    async fn handle_worktree_create(&self, params: Value) -> ServerResult<Value> {
        let wt_path = params
            .get("worktreePath")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let branch = params.get("worktreeBranch").and_then(|v| v.as_str());
        let source_branch = params.get("sourceBranch").and_then(|v| v.as_str());
        let create_new = params
            .get("createNewBranch")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        debug!(wt_path = wt_path, branch = branch, "Creating worktree");

        let cwd = self.get_cwd().await;
        let opts = roo_worktree::CreateWorktreeOptions {
            path: wt_path.to_string(),
            branch: branch.map(|s| s.to_string()),
            create_new_branch: create_new,
            source_branch: source_branch.map(|s| s.to_string()),
        };

        match roo_worktree::create_worktree(Path::new(&cwd), &opts) {
            Ok(result) => Ok(json!({
                "success": true,
                "text": format!("Created worktree at {} (branch: {})", result.path, result.branch),
                "path": result.path,
                "branch": result.branch,
            })),
            Err(e) => Ok(json!({"success": false, "message": e})),
        }
    }

    /// Delete a git worktree.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteWorktree`
    /// Source: TS `handlers.ts` — `handleDeleteWorktree`
    async fn handle_worktree_delete(&self, params: Value) -> ServerResult<Value> {
        let wt_path = params
            .get("worktreePath")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(wt_path = wt_path, force = force, "Deleting worktree");

        let cwd = self.get_cwd().await;
        match roo_worktree::delete_worktree(Path::new(&cwd), wt_path, force) {
            Ok(msg) => Ok(json!({"success": true, "text": msg})),
            Err(e) => Ok(json!({"success": false, "message": e})),
        }
    }

    /// Switch to a git worktree.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `switchWorktree`
    /// In headless mode, we change the working directory instead of opening a new window.
    async fn handle_worktree_switch(&self, params: Value) -> ServerResult<Value> {
        let wt_path = params
            .get("worktreePath")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(wt_path = wt_path, "Switching worktree");

        if wt_path.is_empty() {
            return Ok(json!({"success": false, "message": "missing worktreePath"}));
        }

        // Return action for the Tauri GUI to switch the working directory
        Ok(json!({
            "action": "switchWorktree",
            "path": wt_path,
            "success": true,
            "text": format!("Switch to {wt_path}"),
        }))
    }

    /// Get available git branches.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getAvailableBranches`
    /// Source: TS `handlers.ts` — `handleGetAvailableBranches`
    async fn handle_worktree_get_branches(&self, _params: Value) -> ServerResult<Value> {
        debug!("Getting available branches");
        let cwd = self.get_cwd().await;

        match roo_worktree::get_available_branches(Path::new(&cwd)) {
            Ok(result) => Ok(json!({
                "localBranches": result.local_branches,
                "remoteBranches": result.remote_branches,
                "currentBranch": result.current_branch,
            })),
            Err(e) => Ok(json!({
                "localBranches": [],
                "remoteBranches": [],
                "currentBranch": "",
                "error": e,
            })),
        }
    }

    /// Get worktree defaults.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getWorktreeDefaults`
    /// Source: TS `handlers.ts` — `handleGetWorktreeDefaults`
    async fn handle_worktree_get_defaults(&self, _params: Value) -> ServerResult<Value> {
        debug!("Getting worktree defaults");
        let cwd = self.get_cwd().await;

        let suffix = roo_worktree::generate_random_suffix(5);
        let project_name = Path::new(&cwd)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let suggested_path = format!("{home}/.roo/worktrees/{project_name}-{suffix}");
        let suggested_branch = format!("worktree/roo-{suffix}");

        Ok(json!({
            "suggestedBranch": suggested_branch,
            "suggestedPath": suggested_path,
        }))
    }

    /// Get worktree include status.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getWorktreeIncludeStatus`
    /// Source: TS `WorktreeIncludeService.getStatus`
    async fn handle_worktree_get_include_status(&self, _params: Value) -> ServerResult<Value> {
        debug!("Getting worktree include status");
        let cwd = self.get_cwd().await;
        let (exists, has_gitignore, gitignore_content) =
            roo_worktree::get_worktree_include_status(Path::new(&cwd));

        Ok(json!({
            "worktreeIncludeStatus": {
                "exists": exists,
                "hasGitignore": has_gitignore,
                "gitignoreContent": gitignore_content,
            },
        }))
    }

    /// Check branch worktree include.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `checkBranchWorktreeInclude`
    /// Source: TS `WorktreeIncludeService.branchHasWorktreeInclude`
    async fn handle_worktree_check_branch_include(&self, params: Value) -> ServerResult<Value> {
        let branch = params
            .get("worktreeBranch")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(branch = branch, "Checking branch worktree include");

        if branch.is_empty() {
            return Ok(json!({"hasWorktreeInclude": false}));
        }

        let cwd = self.get_cwd().await;
        let has = roo_worktree::branch_has_worktree_include(Path::new(&cwd), branch);
        Ok(json!({"branch": branch, "hasWorktreeInclude": has}))
    }

    /// Create worktree include.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `createWorktreeInclude`
    /// Source: TS `WorktreeIncludeService.createWorktreeInclude`
    async fn handle_worktree_create_include(&self, params: Value) -> ServerResult<Value> {
        let content = params
            .get("worktreeIncludeContent")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!("Creating worktree include");

        let cwd = self.get_cwd().await;
        match roo_worktree::create_worktree_include(Path::new(&cwd), content) {
            Ok(msg) => Ok(json!({"success": true, "text": msg})),
            Err(e) => Ok(json!({"success": false, "message": e})),
        }
    }

    /// Checkout a branch.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `checkoutBranch`
    /// Source: TS `WorktreeService.checkoutBranch`
    async fn handle_worktree_checkout_branch(&self, params: Value) -> ServerResult<Value> {
        let branch = params
            .get("worktreeBranch")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(branch = branch, "Checking out branch");

        let cwd = self.get_cwd().await;
        match roo_worktree::checkout_branch(Path::new(&cwd), branch) {
            Ok(msg) => Ok(json!({"success": true, "text": msg})),
            Err(e) => Ok(json!({"success": false, "message": e})),
        }
    }

    /// Browse for worktree path.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `browseForWorktreePath`
    /// Headless: no folder picker available — return suggested path.
    async fn handle_worktree_browse_path(&self, _params: Value) -> ServerResult<Value> {
        debug!("Browsing for worktree path");
        // Headless: no folder picker available, delegate to getWorktreeDefaults
        let defaults = self.handle_worktree_get_defaults(_params).await?;
        if let Some(path) = defaults.get("suggestedPath").and_then(|v| v.as_str()) {
            Ok(json!({"path": path}))
        } else {
            Ok(json!({"action": "showFolderPicker"}))
        }
    }

    // ── TTS ────────────────────────────────────────────────────────────────

    /// Play text-to-speech.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `playTts`
    async fn handle_tts_play(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Playing TTS");
        Ok(json!({"action": "playTts", "text": text}))
    }

    /// Stop text-to-speech.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `stopTts`
    async fn handle_tts_stop(&self, _params: Value) -> ServerResult<Value> {
        debug!("Stopping TTS");
        Ok(json!({"action": "stopTts"}))
    }

    /// Set TTS enabled.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `ttsEnabled`
    async fn handle_tts_enabled(&self, params: Value) -> ServerResult<Value> {
        let enabled = params.get("bool").and_then(|v| v.as_bool()).unwrap_or(true);
        debug!(enabled = enabled, "Setting TTS enabled");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);
        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["ttsEnabled"] = json!(enabled);
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({"status": "updated", "ttsEnabled": enabled}))
    }

    /// Set TTS speed.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `ttsSpeed`
    async fn handle_tts_speed(&self, params: Value) -> ServerResult<Value> {
        let speed = params.get("value").and_then(|v| v.as_f64()).unwrap_or(1.0);
        debug!(speed = speed, "Setting TTS speed");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);
        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["ttsSpeed"] = json!(speed);
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({"status": "updated", "ttsSpeed": speed}))
    }

    // ── Image ──────────────────────────────────────────────────────────────

    /// Save an image from data URI.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `saveImage`
    async fn handle_image_save(&self, params: Value) -> ServerResult<Value> {
        let data_uri = params.get("dataUri").and_then(|v| v.as_str()).unwrap_or("");
        debug!(data_uri_len = data_uri.len(), "Saving image");

        if data_uri.is_empty() {
            return Ok(json!({"status": "error", "error": "missing dataUri"}));
        }

        // Parse data URI: data:image/<format>;base64,<data>
        let parts: Vec<&str> = data_uri.splitn(2, ',').collect();
        if parts.len() != 2 {
            return Ok(json!({"status": "error", "error": "invalid data URI format"}));
        }

        let header = parts[0];
        let b64_data = parts[1];

        // Extract format from header
        let format = header
            .strip_prefix("data:image/")
            .and_then(|s| s.split(';').next())
            .unwrap_or("png");

        let cwd = {
            let app = self.app.read().await;
            app.cwd().to_string()
        };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let filename = format!("img_{}.{}", timestamp, format);
        let save_path = std::path::Path::new(&cwd).join(&filename);

        // Decode base64 using a simple manual decoder (no external crate needed)
        match decode_base64(b64_data) {
            Ok(bytes) => match std::fs::write(&save_path, bytes) {
                Ok(()) => Ok(json!({"status": "saved", "path": save_path.to_string_lossy()})),
                Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
            },
            Err(e) => {
                Ok(json!({"status": "error", "error": format!("base64 decode failed: {}", e)}))
            }
        }
    }

    /// Open an image.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openImage`
    async fn handle_image_open(&self, params: Value) -> ServerResult<Value> {
        let path = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(path = path, "Opening image");
        if path.is_empty() {
            return Ok(json!({"status": "error", "error": "missing path"}));
        }
        match opener::open(path) {
            Ok(()) => Ok(json!({"status": "opened"})),
            Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
        }
    }

    // ── Model requests ─────────────────────────────────────────────────────

    /// Flush router models cache.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `flushRouterModels`
    /// TS: calls `flushModels({provider: routerName}, true)` which clears the model cache.
    async fn handle_models_flush_router(&self, params: Value) -> ServerResult<Value> {
        let router_name = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(router_name = router_name, "Flushing router models");

        // Clear any cached model data from settings
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        // Remove cached model lists for this router
        if let Some(obj) = settings.as_object_mut() {
            let cache_key = format!("cachedModels_{}", router_name);
            obj.remove(&cache_key);
            obj.remove("cachedModels"); // Also clear the generic cache
        }
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({"status": "flushed", "router": router_name}))
    }

    /// Request router models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestRouterModels`
    async fn handle_models_request_router(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting router models");
        let app = self.app.read().await;
        let settings = app.provider_settings();
        Ok(json!({"models": {"provider": settings.api_provider, "modelId": settings.api_model_id}}))
    }

    /// Request OpenAI models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestOpenAiModels`
    async fn handle_models_request_openai(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting OpenAI models");
        let api_key = self.get_api_key_for_provider("openai").await;
        match roo_provider::fetcher::fetch_openai_compatible_models(
            "https://api.openai.com/v1",
            api_key.as_deref(),
        )
        .await
        {
            Ok(models) => {
                let model_list: Vec<String> = models.keys().cloned().collect();
                Ok(json!({"models": model_list}))
            }
            Err(e) => {
                warn!(error = %e, "Failed to fetch OpenAI models");
                Ok(json!({"models": [], "error": e.to_string()}))
            }
        }
    }

    /// Request Ollama models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestOllamaModels`
    async fn handle_models_request_ollama(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting Ollama models");
        let base_url = self
            .get_base_url_for_provider("ollama")
            .await
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let tags_url = format!(
            "{}/api/tags",
            base_url.trim_end_matches("/v1").trim_end_matches('/')
        );
        match roo_provider::fetcher::fetch_models_raw::<serde_json::Value>(&tags_url, None).await {
            Ok(data) => {
                let models: Vec<String> = data
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                m.get("name")
                                    .and_then(|n| n.as_str())
                                    .map(|s| s.trim_end_matches(":latest").to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({"models": models}))
            }
            Err(e) => {
                debug!(error = %e, "Ollama not available");
                Ok(json!({"models": [], "error": e.to_string()}))
            }
        }
    }

    /// Request LM Studio models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestLmStudioModels`
    async fn handle_models_request_lmstudio(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting LM Studio models");
        let base_url = self
            .get_base_url_for_provider("lmstudio")
            .await
            .unwrap_or_else(|| "http://localhost:1234".to_string());
        let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
        match roo_provider::fetcher::fetch_openai_compatible_models(&url, None).await {
            Ok(models) => {
                let model_list: Vec<String> = models.keys().cloned().collect();
                Ok(json!({"models": model_list}))
            }
            Err(e) => {
                debug!(error = %e, "LM Studio not available");
                Ok(json!({"models": [], "error": e.to_string()}))
            }
        }
    }

    /// Request Roo models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestRooModels`
    /// TS: Fetches from `https://api.roocode.com/proxy` with cloud session token.
    async fn handle_models_request_roo(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting Roo models");
        let api_key = self.get_api_key_for_provider("roo").await;
        let base_url = self
            .get_base_url_for_provider("roo")
            .await
            .unwrap_or_else(|| "https://api.roocode.com/proxy".to_string());

        let models_url = format!("{}/models", base_url.trim_end_matches('/'));
        match roo_provider::fetcher::fetch_openai_compatible_models(&models_url, api_key.as_deref())
            .await
        {
            Ok(models) => {
                let model_list: Vec<String> = models.keys().cloned().collect();
                Ok(json!({
                    "type": "singleRouterModelFetchResponse",
                    "success": true,
                    "values": {"provider": "roo", "models": model_list},
                }))
            }
            Err(e) => {
                debug!(error = %e, "Failed to fetch Roo models");
                Ok(json!({
                    "type": "singleRouterModelFetchResponse",
                    "success": false,
                    "values": {"provider": "roo", "models": []},
                    "error": e.to_string(),
                }))
            }
        }
    }

    /// Request Roo credit balance.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestRooCreditBalance`
    /// TS: Calls CloudService.instance.cloudAPI.creditBalance().
    async fn handle_models_request_roo_credit(&self, params: Value) -> ServerResult<Value> {
        debug!("Requesting Roo credit balance");
        let request_id = params
            .get("requestId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Check for Roo API key (session token equivalent)
        let api_key = self.get_api_key_for_provider("roo").await;
        let base_url = self
            .get_base_url_for_provider("roo")
            .await
            .unwrap_or_else(|| "https://api.roocode.com".to_string());

        match api_key {
            Some(key) => {
                let url = format!("{}/credits/balance", base_url.trim_end_matches('/'));
                match roo_provider::fetcher::fetch_models_raw::<serde_json::Value>(&url, Some(&key))
                    .await
                {
                    Ok(data) => Ok(json!({
                        "type": "rooCreditBalance",
                        "requestId": request_id,
                        "values": data,
                    })),
                    Err(e) => Ok(json!({
                        "type": "rooCreditBalance",
                        "requestId": request_id,
                        "error": e.to_string(),
                    })),
                }
            }
            None => Ok(json!({
                "type": "rooCreditBalance",
                "requestId": request_id,
                "error": "Cloud service not available",
            })),
        }
    }

    /// Request VS Code LM models.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestVsCodeLmModels`
    async fn handle_models_request_vscode_lm(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting VS Code LM models");
        Ok(json!({"models": []}))
    }

    // ── Mentions ───────────────────────────────────────────────────────────
    // Source: TS `src/core/mentions/index.ts` — `openMention`, `parseMentions`

    /// Open a mention (file reference, problems, git, url, terminal).
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openMention`
    /// TS handles: file paths (`/path`), `@problems`, `@git`, `@url`, `@terminal`.
    /// For file paths, returns content directly. For non-file types, delegates to Tauri GUI.
    async fn handle_mention_open(&self, params: Value) -> ServerResult<Value> {
        let mention = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(mention = mention, "Opening mention");

        // File path mentions start with / or \
        if mention.starts_with('/') || mention.starts_with("\\") {
            let cwd = self.get_cwd().await;
            let cwd_path = Path::new(&cwd);
            let mention_stripped = mention.trim_start_matches('/').trim_start_matches('\\');
            match roo_mentions::get_file_or_folder_content(mention_stripped, cwd_path).await {
                Ok(block) => Ok(json!({
                    "status": "opened",
                    "mention": mention,
                    "content": block.content,
                })),
                Err(e) => Ok(json!({
                    "status": "error",
                    "mention": mention,
                    "error": e,
                })),
            }
        }
        // @problems — open VS Code / Tauri problems panel
        else if mention == "@problems" {
            Ok(json!({"action": "openProblemsPanel", "mention": mention}))
        }
        // @git — open git history / SCM view
        else if mention == "@git" {
            let cwd = self.get_cwd().await;
            // Return recent git log as content for the mention
            match std::process::Command::new("git")
                .args(["log", "--oneline", "-20"])
                .current_dir(&cwd)
                .output()
            {
                Ok(output) => {
                    let log = String::from_utf8_lossy(&output.stdout);
                    Ok(json!({
                        "status": "opened",
                        "mention": mention,
                        "content": log,
                    }))
                }
                Err(e) => Ok(json!({
                    "status": "error",
                    "mention": mention,
                    "error": e.to_string(),
                })),
            }
        }
        // @url — open URL in browser
        else if mention.starts_with("@url:")
            || mention.starts_with("http://")
            || mention.starts_with("https://")
        {
            let url = mention.trim_start_matches("@url:").trim();
            match opener::open(url) {
                Ok(()) => Ok(json!({"status": "opened", "mention": mention})),
                Err(e) => {
                    Ok(json!({"status": "error", "mention": mention, "error": e.to_string()}))
                }
            }
        }
        // @terminal — show terminal output
        else if mention == "@terminal" {
            // Return terminal output from active task's terminals
            let output = match self.task_manager.get_active_task() {
                Some(lifecycle) => {
                    let lc = lifecycle.lock().await;
                    if let Some(ref registry) = lc.services().terminal_registry {
                        let terminals = registry.get_background_terminals(None).await;
                        let mut outputs = Vec::new();
                        for terminal in terminals {
                            let mut guard = terminal.lock().await;
                            let tid = guard.get_id();
                            let out = guard.get_unretrieved_output();
                            outputs.push(format!(
                                "Terminal [{}]: {}",
                                tid,
                                if out.is_empty() { "(no output)" } else { &out }
                            ));
                        }
                        outputs.join("\n")
                    } else {
                        "No terminal registry available".to_string()
                    }
                }
                None => "No active task".to_string(),
            };
            Ok(json!({"status": "opened", "mention": mention, "content": output}))
        } else {
            Ok(json!({"status": "not_applicable", "mention": mention}))
        }
    }

    /// Resolve mentions in text.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `resolveMentions` (internal)
    async fn handle_mention_resolve(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Resolving mentions");

        let cwd = self.get_cwd().await;
        let result = roo_mentions::parse_mentions(text, Path::new(&cwd)).await;

        let mentions: Vec<Value> = result
            .content_blocks
            .iter()
            .map(|block| {
                json!({
                    "type": format!("{:?}", block.block_type),
                    "content": block.content,
                })
            })
            .collect();

        Ok(json!({
            "text": result.text,
            "mentions": mentions,
        }))
    }

    // ── Commands (slash) ───────────────────────────────────────────────────

    /// Request discovered commands.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestCommands`
    async fn handle_command_request(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting commands");
        // Reuse task_get_commands logic
        self.handle_task_get_commands(_params).await
    }

    /// Open a command file.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openCommandFile`
    async fn handle_command_open_file(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(name = name, "Opening command file");

        if name.is_empty() {
            return Ok(json!({"status": "error", "error": "missing command name"}));
        }

        // Try project commands first, then global
        let cwd = self.get_cwd().await;
        let project_cmd = std::path::Path::new(&cwd)
            .join(".roo")
            .join("commands")
            .join(format!("{name}.md"));
        let global_dir = roo_config::paths::get_global_roo_directory()
            .join("commands")
            .join(format!("{name}.md"));

        let path_to_open = if project_cmd.exists() {
            project_cmd
        } else if global_dir.exists() {
            global_dir
        } else {
            return Ok(json!({"status": "not_found", "name": name}));
        };

        match opener::open(&path_to_open) {
            Ok(()) => Ok(json!({"status": "opened", "path": path_to_open.to_string_lossy()})),
            Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
        }
    }

    /// Delete a command.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deleteCommand`
    async fn handle_command_delete(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return Err(ServerError::InvalidParams {
                method: "command_delete".into(),
                detail: "command name is required".into(),
            });
        }

        let cwd = self.get_cwd().await;
        let cwd_path = Path::new(&cwd);

        // Build candidate paths: project first, then global
        let project_cmd = paths::get_project_roo_directory_for_cwd(cwd_path)
            .join("commands")
            .join(format!("{name}.md"));
        let global_cmd = paths::get_global_roo_directory()
            .join("commands")
            .join(format!("{name}.md"));

        let file_path = if project_cmd.exists() {
            &project_cmd
        } else if global_cmd.exists() {
            &global_cmd
        } else {
            return Err(ServerError::InvalidParams {
                method: "command_delete".into(),
                detail: format!(
                    "command '{name}' not found in project or global commands directory"
                ),
            });
        };

        match std::fs::remove_file(file_path) {
            Ok(()) => {
                debug!(name = name, path = %file_path.display(), "Deleted command");
                Ok(json!({"status": "deleted", "name": name}))
            }
            Err(e) => {
                error!(name = name, error = %e, "Failed to delete command");
                Err(ServerError::Internal(format!(
                    "failed to delete command: {e}"
                )))
            }
        }
    }

    /// Create a command.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `createCommand`
    async fn handle_command_create(&self, params: Value) -> ServerResult<Value> {
        let name = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let content = params
            .get("values")
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if name.is_empty() {
            return Err(ServerError::InvalidParams {
                method: "command_create".into(),
                detail: "command name is required".into(),
            });
        }

        let cwd = self.get_cwd().await;
        let cwd_path = Path::new(&cwd);
        let commands_dir = paths::get_project_roo_directory_for_cwd(cwd_path).join("commands");

        // Create the commands directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&commands_dir) {
            error!(dir = %commands_dir.display(), error = %e, "Failed to create commands directory");
            return Err(ServerError::Internal(format!(
                "failed to create commands directory: {e}"
            )));
        }

        let file_path = commands_dir.join(format!("{name}.md"));
        match std::fs::write(&file_path, content) {
            Ok(()) => {
                debug!(name = name, path = %file_path.display(), "Created command");
                Ok(json!({"status": "created", "name": name, "source": "project"}))
            }
            Err(e) => {
                error!(name = name, error = %e, "Failed to write command");
                Err(ServerError::Internal(format!(
                    "failed to write command: {e}"
                )))
            }
        }
    }

    // ── Settings (additional) ──────────────────────────────────────────────

    /// Set denied commands.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `deniedCommands`
    async fn handle_settings_denied_commands(&self, params: Value) -> ServerResult<Value> {
        let denied: Vec<String> = params
            .get("commands")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!(denied = ?denied, "Setting denied commands");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["deniedCommands"] = json!(denied);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "deniedCount": denied.len()}))
    }

    /// Update condensing prompt.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateCondensingPrompt`
    async fn handle_settings_condensing_prompt(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Updating condensing prompt");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["condensingPrompt"] = json!(text);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated"}))
    }

    /// Set API config password.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `setApiConfigPassword`
    async fn handle_settings_set_api_config_password(&self, _params: Value) -> ServerResult<Value> {
        debug!("Setting API config password");
        // Intentionally not persisted — passwords are never stored in plaintext settings
        Ok(json!({"status": "updated"}))
    }

    /// Set has opened mode selector.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `hasOpenedModeSelector`
    async fn handle_settings_has_opened_mode_selector(&self, params: Value) -> ServerResult<Value> {
        let opened = params.get("bool").and_then(|v| v.as_bool()).unwrap_or(true);
        debug!(opened = opened, "Setting has opened mode selector");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["hasOpenedModeSelector"] = json!(opened);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "hasOpenedModeSelector": opened}))
    }

    /// Set task sync enabled.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `taskSyncEnabled`
    async fn handle_settings_task_sync_enabled(&self, params: Value) -> ServerResult<Value> {
        let enabled = params
            .get("bool")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        debug!(enabled = enabled, "Setting task sync enabled");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["taskSyncEnabled"] = json!(enabled);

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated", "taskSyncEnabled": enabled}))
    }

    /// Batch update settings.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateSettings`
    async fn handle_settings_update_settings(&self, params: Value) -> ServerResult<Value> {
        debug!("Batch updating settings");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        // Merge all incoming keys into the existing settings
        if let (Some(existing), Some(incoming)) = (settings.as_object_mut(), params.as_object()) {
            for (key, value) in incoming {
                existing.insert(key.clone(), value.clone());
            }
        }

        Self::write_roo_settings(&gsp, &settings).await?;
        Ok(json!({"status": "updated"}))
    }

    /// Update a VS Code setting.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `updateVSCodeSetting`
    async fn handle_settings_update_vscode_setting(&self, params: Value) -> ServerResult<Value> {
        let setting = params.get("setting").and_then(|v| v.as_str()).unwrap_or("");
        debug!(setting = setting, "Updating VS Code setting");
        Ok(json!({"status": "not_applicable", "note": "headless mode - no VS Code settings"}))
    }

    /// Get a VS Code setting.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getVSCodeSetting`
    async fn handle_settings_get_vscode_setting(&self, params: Value) -> ServerResult<Value> {
        let setting = params.get("setting").and_then(|v| v.as_str()).unwrap_or("");
        debug!(setting = setting, "Getting VS Code setting");
        Ok(json!({"setting": setting, "value": null, "note": "headless mode"}))
    }

    // ── History (additional) ───────────────────────────────────────────────

    /// Share current task.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `shareCurrentTask`
    /// Share/export a task's conversation history.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `shareCurrentTask`
    /// TS: calls CloudService.instance.shareTask() which uploads to Roo Cloud.
    /// In Tauri: we export the task data as a JSON file that can be shared.
    async fn handle_history_share_task(&self, params: Value) -> ServerResult<Value> {
        let visibility = params
            .get("visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("organization");
        debug!(visibility = visibility, "Sharing/exporting task");

        match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let lc = lifecycle.lock().await;
                let task_id = lc.task_id().to_string();
                let messages = lc.engine().cline_messages();
                let api_history = lc.engine().api_conversation_history();
                let cwd = lc.engine().config().cwd.clone();

                let export = json!({
                    "taskId": task_id,
                    "visibility": visibility,
                    "exportedAt": chrono::Utc::now().to_rfc3339(),
                    "workspace": cwd,
                    "clineMessages": messages,
                    "apiConversationHistory": api_history.iter().map(|m| {
                        json!({
                            "role": format!("{:?}", m.role),
                            "contentBlocks": m.content.len(),
                        })
                    }).collect::<Vec<_>>(),
                });

                // Write export file
                let app = self.app.read().await;
                let gsp = app.config().global_storage_path.clone();
                drop(app);
                let export_dir = if gsp.is_empty() {
                    paths::get_global_roo_directory()
                } else {
                    std::path::PathBuf::from(&gsp)
                }
                .join("exports");
                let _ = tokio::fs::create_dir_all(&export_dir).await;
                let export_path = export_dir.join(format!(
                    "task-{}-{}.json",
                    &task_id[..8.min(task_id.len())],
                    chrono::Utc::now().format("%Y%m%d%H%M%S")
                ));
                match tokio::fs::write(
                    &export_path,
                    serde_json::to_string_pretty(&export).unwrap_or_default(),
                )
                .await
                {
                    Ok(()) => Ok(json!({
                        "status": "exported",
                        "taskId": task_id,
                        "path": export_path.to_string_lossy(),
                        "visibility": visibility,
                    })),
                    Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
                }
            }
            None => Ok(json!({"status": "error", "error": "no active task"})),
        }
    }

    // ── UI / Tauri GUI commands ──────────────────────────────────────────

    /// Webview did launch — full initialization.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `webviewDidLaunch`
    /// TS loads: custom modes, workspace tracker, theme, MCP servers, API configs.
    /// In Rust: returns all initial state the GUI needs to bootstrap.
    async fn handle_webview_did_launch(&self, _params: Value) -> ServerResult<Value> {
        debug!("GUI did launch — initializing");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let cwd = app.cwd().to_string();

        // 1. Load custom modes via CustomModesManager
        let custom_modes: Vec<roo_types::mode::ModeConfig> = match app.custom_modes_manager() {
            Some(mgr_arc) => {
                let mut mgr = mgr_arc.write().unwrap_or_else(|e| e.into_inner());
                mgr.get_custom_modes()
            }
            None => vec![],
        };

        // 2. Get current provider settings
        let prov_settings = app.provider_settings();
        let api_provider = prov_settings.api_provider.map(|p| format!("{:?}", p));
        let api_model_id = prov_settings.api_model_id.clone();

        // 3. List MCP servers
        let mcp_servers = match app.mcp_hub() {
            Some(hub) => hub.get_all_servers().await,
            None => vec![],
        };

        // 4. Get current state from settings
        let roo_settings = Self::read_roo_settings(&gsp).await;
        let current_config_name = roo_settings
            .get("currentApiConfigName")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();

        // 5. Read API config entries from settings file
        let api_configs: Vec<Value> = roo_settings
            .get("apiConfigEntries")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let state_json = json!({
            "mode": roo_settings.get("mode").and_then(|v| v.as_str()).unwrap_or("code"),
            "currentApiConfigName": current_config_name,
            "customModes": custom_modes,
            "apiProvider": api_provider,
            "apiModelId": api_model_id,
        });

        drop(app);

        Ok(json!({
            "status": "launched",
            "state": state_json,
            "mcpServers": mcp_servers,
            "apiConfigs": api_configs,
            "workspace": cwd,
        }))
    }

    /// Announcement did show.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `didShowAnnouncement`
    async fn handle_announcement_did_show(&self, _params: Value) -> ServerResult<Value> {
        debug!("Announcement did show");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);
        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["announcementShown"] = json!(true);
        let _ = Self::write_roo_settings(&gsp, &settings).await;
        Ok(json!({"status": "acknowledged"}))
    }

    /// Select images via file dialog.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `selectImages`
    /// In Tauri: returns an action for the GUI to show a file picker dialog.
    async fn handle_images_select(&self, _params: Value) -> ServerResult<Value> {
        debug!("Selecting images");
        Ok(json!({"action": "showImagePicker", "images": []}))
    }

    /// Dragged images.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `draggedImages`
    async fn handle_images_dragged(&self, params: Value) -> ServerResult<Value> {
        let _urls: Vec<String> = params
            .get("dataUrls")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        debug!("Dragged images");
        Ok(json!({"status": "acknowledged"}))
    }

    /// Play sound.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `playSound`
    /// In Tauri: returns an action for the GUI to play the sound.
    async fn handle_play_sound(&self, params: Value) -> ServerResult<Value> {
        let audio_type = params
            .get("audioType")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        debug!(audio_type = audio_type, "Playing sound");
        Ok(json!({"action": "playSound", "audioType": audio_type}))
    }

    /// Open a file in the system editor.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openFile`
    async fn handle_file_open(&self, params: Value) -> ServerResult<Value> {
        let path = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(path = path, "Opening file");
        if path.is_empty() {
            return Ok(json!({"status": "error", "error": "missing path"}));
        }
        match opener::open(path) {
            Ok(()) => Ok(json!({"status": "opened"})),
            Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
        }
    }

    /// Open external URL in the system browser.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openExternal`
    async fn handle_external_open(&self, params: Value) -> ServerResult<Value> {
        let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
        debug!(url = url, "Opening external URL");
        if url.is_empty() {
            return Ok(json!({"status": "error", "error": "missing url"}));
        }
        match opener::open(url) {
            Ok(()) => Ok(json!({"status": "opened"})),
            Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
        }
    }

    /// Open keyboard shortcuts.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openKeyboardShortcuts`
    async fn handle_open_keyboard_shortcuts(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening keyboard shortcuts");
        Ok(json!({"action": "openKeyboardShortcuts"}))
    }

    /// Open MCP settings file.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openMcpSettings`
    async fn handle_open_mcp_settings(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening MCP settings");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);
        let settings_path = Self::roo_settings_path(&gsp);
        let mcp_path = settings_path
            .parent()
            .map(|p| p.join("mcp.json"))
            .unwrap_or_else(|| settings_path.clone());
        if mcp_path.exists() {
            match opener::open(&mcp_path) {
                Ok(()) => Ok(json!({"status": "opened", "path": mcp_path.to_string_lossy()})),
                Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
            }
        } else {
            Ok(json!({"action": "openFile", "path": mcp_path.to_string_lossy(), "exists": false}))
        }
    }

    /// Open project MCP settings file.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openProjectMcpSettings`
    async fn handle_open_project_mcp_settings(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening project MCP settings");
        let cwd = self.get_cwd().await;
        let mcp_path = std::path::Path::new(&cwd).join(".roo").join("mcp.json");
        if mcp_path.exists() {
            match opener::open(&mcp_path) {
                Ok(()) => Ok(json!({"status": "opened", "path": mcp_path.to_string_lossy()})),
                Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
            }
        } else {
            Ok(json!({"action": "openFile", "path": mcp_path.to_string_lossy(), "exists": false}))
        }
    }

    /// Focus panel request.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `focusPanelRequest`
    async fn handle_focus_panel(&self, _params: Value) -> ServerResult<Value> {
        debug!("Focus panel request");
        Ok(json!({"action": "focusPanel"}))
    }

    /// Switch tab.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `switchTab`
    async fn handle_tab_switch(&self, params: Value) -> ServerResult<Value> {
        let tab = params.get("tab").and_then(|v| v.as_str()).unwrap_or("");
        debug!(tab = tab, "Switching tab");
        Ok(json!({"action": "switchTab", "tab": tab}))
    }

    /// Insert text into textarea.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `insertTextIntoTextarea`
    async fn handle_insert_text(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Inserting text");
        Ok(json!({"action": "insertText", "text": text}))
    }

    /// Open markdown preview.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `openMarkdownPreview`
    async fn handle_markdown_preview(&self, params: Value) -> ServerResult<Value> {
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
        debug!(text_len = text.len(), "Opening markdown preview");
        Ok(json!({"action": "openMarkdownPreview", "text": text}))
    }

    // ── Cloud ──────────────────────────────────────────────────────────────

    /// Cloud sign in — delegates to Tauri GUI auth flow.
    async fn handle_cloud_sign_in(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cloud sign in");
        Ok(json!({"action": "cloudSignIn"}))
    }

    /// Cloud sign out.
    async fn handle_cloud_sign_out(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cloud sign out");
        Ok(json!({"action": "cloudSignOut"}))
    }

    /// Cloud manual URL.
    async fn handle_cloud_manual_url(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cloud manual URL");
        Ok(json!({"action": "cloudManualUrl"}))
    }

    /// Cloud button clicked.
    async fn handle_cloud_button_clicked(&self, _params: Value) -> ServerResult<Value> {
        debug!("Cloud button clicked");
        Ok(json!({"status": "acknowledged"}))
    }

    /// Clear cloud auth skip model.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `clearCloudAuthSkipModel`
    /// TS: `await provider.context.globalState.update("roo-auth-skip-model", undefined)`
    async fn handle_cloud_clear_skip_model(&self, _params: Value) -> ServerResult<Value> {
        debug!("Clearing cloud auth skip model");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("roo-auth-skip-model");
        }
        let _ = Self::write_roo_settings(&gsp, &settings).await;
        Ok(json!({"status": "cleared"}))
    }

    /// Switch organization.
    async fn handle_cloud_switch_org(&self, params: Value) -> ServerResult<Value> {
        let org_id = params
            .get("organizationId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(org_id = org_id, "Switching organization");
        Ok(json!({"action": "switchOrganization", "organizationId": org_id}))
    }

    /// Codex sign in.
    async fn handle_codex_sign_in(&self, _params: Value) -> ServerResult<Value> {
        debug!("Codex sign in");
        Ok(json!({"action": "codexSignIn"}))
    }

    /// Codex sign out.
    async fn handle_codex_sign_out(&self, _params: Value) -> ServerResult<Value> {
        debug!("Codex sign out");
        Ok(json!({"action": "codexSignOut"}))
    }

    /// Request Codex rate limits.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `requestOpenAiCodexRateLimits`
    /// TS: Uses OAuth manager to get access token, then fetches rate limits from API.
    async fn handle_codex_request_rate_limits(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting Codex rate limits");

        // Get Codex API key from settings (if configured)
        let codex_api_key = self.get_api_key_for_provider("openai").await;
        let codex_base_url = self
            .get_base_url_for_provider("openai")
            .await
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        match codex_api_key {
            Some(api_key) => {
                let url = format!("{}/rate_limits", codex_base_url.trim_end_matches('/'));
                match roo_provider::fetcher::fetch_models_raw::<serde_json::Value>(
                    &url,
                    Some(&api_key),
                )
                .await
                {
                    Ok(data) => Ok(json!({"rateLimits": data})),
                    Err(e) => Ok(json!({"rateLimits": null, "error": e.to_string()})),
                }
            }
            None => Ok(json!({"rateLimits": null, "error": "Not authenticated with OpenAI Codex"})),
        }
    }

    // ── Codebase Index ─────────────────────────────────────────────────────

    /// Set codebase index enabled.
    async fn handle_index_enabled(&self, params: Value) -> ServerResult<Value> {
        let enabled = params.get("bool").and_then(|v| v.as_bool()).unwrap_or(true);
        debug!(enabled = enabled, "Setting codebase index enabled");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["codebaseIndexEnabled"] = json!(enabled);

        let _ = Self::write_roo_settings(&gsp, &settings).await;
        Ok(json!({"status": "updated", "enabled": enabled}))
    }

    /// Request indexing status.
    async fn handle_index_request_status(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting indexing status");
        // Report the real state: indexing is not yet available in CLI mode
        // because the embedder requires a real embedding model.
        Ok(
            json!({"action": "requestIndexStatus", "state": "Not Available", "message": "Codebase indexing requires an embedding model (not yet configured in CLI mode)"}),
        )
    }

    /// Start indexing.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `startIndexing`
    /// TS: enables workspace, initializes CodeIndexManager, starts indexing.
    async fn handle_index_start(&self, _params: Value) -> ServerResult<Value> {
        debug!("Starting indexing");
        // Persist the setting even though the engine isn't running yet.
        // When an embedding model is configured later, this preference
        // will be respected.
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let cwd = app.cwd().to_string();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["codebaseIndexEnabled"] = json!(true);
        if let Some(obj) = settings.as_object_mut() {
            let workspaces = obj
                .entry("codebaseIndexWorkspaces")
                .or_insert_with(|| json!({}));
            if let Some(ws) = workspaces.as_object_mut() {
                ws.insert(cwd.clone(), json!(true));
            }
        }
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(
            json!({"action": "startIndexing", "workspace": cwd, "state": "Pending", "message": "Indexing preference saved. Requires embedding model to start."}),
        )
    }

    /// Stop indexing.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `stopIndexing`
    async fn handle_index_stop(&self, _params: Value) -> ServerResult<Value> {
        debug!("Stopping indexing");
        // Persist the disabled setting.
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["codebaseIndexEnabled"] = json!(false);
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({"action": "stopIndexing", "state": "Stopped"}))
    }

    /// Clear index data.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `clearIndexData`
    /// TS: calls `manager.clearIndexData()` then posts indexCleared event.
    async fn handle_index_clear(&self, _params: Value) -> ServerResult<Value> {
        debug!("Clearing index data");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let cwd = app.cwd().to_string();
        drop(app);

        // Clear the index data directory for this workspace
        let index_dir = std::path::Path::new(&gsp)
            .join("codebase-index")
            .join(roo_checkpoint::service::ShadowCheckpointService::hash_workspace_dir(&cwd));

        if index_dir.exists() {
            match tokio::fs::remove_dir_all(&index_dir).await {
                Ok(()) => Ok(json!({"action": "indexCleared", "success": true})),
                Err(e) => {
                    Ok(json!({"action": "indexCleared", "success": false, "error": e.to_string()}))
                }
            }
        } else {
            Ok(json!({"action": "indexCleared", "success": true}))
        }
    }

    /// Toggle workspace indexing.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `toggleWorkspaceIndexing`
    /// TS: enables/disables workspace in CodeIndexManager.
    async fn handle_index_toggle_workspace(&self, params: Value) -> ServerResult<Value> {
        let enabled = params.get("bool").and_then(|v| v.as_bool()).unwrap_or(true);
        debug!(enabled = enabled, "Toggling workspace indexing");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        let cwd = app.cwd().to_string();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        if let Some(obj) = settings.as_object_mut() {
            let workspaces = obj
                .entry("codebaseIndexWorkspaces")
                .or_insert_with(|| json!({}));
            if let Some(ws) = workspaces.as_object_mut() {
                ws.insert(cwd.clone(), json!(enabled));
            }
        }
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({
            "action": "toggleWorkspaceIndexing",
            "workspace": cwd,
            "enabled": enabled,
            "state": if enabled { "Indexing" } else { "Standby" },
        }))
    }

    /// Set auto-enable default.
    async fn handle_index_set_auto_enable(&self, params: Value) -> ServerResult<Value> {
        let enabled = params.get("bool").and_then(|v| v.as_bool()).unwrap_or(true);
        debug!(enabled = enabled, "Setting auto-enable default");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        settings["codebaseIndexAutoEnable"] = json!(enabled);

        let _ = Self::write_roo_settings(&gsp, &settings).await;
        Ok(json!({"status": "updated", "autoEnable": enabled}))
    }

    /// Save code index settings atomically.
    async fn handle_index_save_settings(&self, params: Value) -> ServerResult<Value> {
        debug!("Saving code index settings");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        if let (Some(obj), Some(incoming)) = (settings.as_object_mut(), params.as_object()) {
            for (key, value) in incoming {
                obj.insert(key.clone(), value.clone());
            }
        }

        let _ = Self::write_roo_settings(&gsp, &settings).await;
        Ok(json!({"status": "saved"}))
    }

    /// Request code index secret status.
    async fn handle_index_request_secret_status(&self, _params: Value) -> ServerResult<Value> {
        debug!("Requesting code index secret status");
        Ok(json!({"action": "requestSecretStatus"}))
    }

    // ── Upsell ─────────────────────────────────────────────────────────────

    /// Dismiss an upsell.
    /// Dismiss an upsell.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `dismissUpsell`
    /// TS: Persists to `globalState("dismissedUpsells")`.
    async fn handle_upsell_dismiss(&self, params: Value) -> ServerResult<Value> {
        let upsell_id = params
            .get("upsellId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        debug!(upsell_id = upsell_id, "Dismissing upsell");

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let mut settings = Self::read_roo_settings(&gsp).await;
        let mut dismissed: Vec<String> = settings
            .get("dismissedUpsells")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if !dismissed.contains(&upsell_id.to_string()) {
            dismissed.push(upsell_id.to_string());
        }

        settings["dismissedUpsells"] = serde_json::to_value(&dismissed).unwrap_or(json!([]));
        let _ = Self::write_roo_settings(&gsp, &settings).await;

        Ok(json!({"type": "dismissedUpsells", "list": dismissed}))
    }

    /// Get dismissed upsells.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `getDismissedUpsells`
    async fn handle_upsell_get_dismissed(&self, _params: Value) -> ServerResult<Value> {
        debug!("Getting dismissed upsells");
        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        let settings = Self::read_roo_settings(&gsp).await;
        let dismissed: Vec<String> = settings
            .get("dismissedUpsells")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(json!({"type": "dismissedUpsells", "list": dismissed}))
    }

    // ── Debug ──────────────────────────────────────────────────────────────

    /// Open debug API history.
    async fn handle_debug_api_history(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening debug API history");
        Ok(json!({"action": "openDebugApiHistory"}))
    }

    /// Open debug UI history.
    async fn handle_debug_ui_history(&self, _params: Value) -> ServerResult<Value> {
        debug!("Opening debug UI history");
        Ok(json!({"action": "openDebugUiHistory"}))
    }

    /// Download error diagnostics.
    ///
    /// Source: TS `webviewMessageHandler.ts` — `downloadErrorDiagnostics`
    /// TS: calls `generateErrorDiagnostics()` which creates a diagnostics file.
    async fn handle_debug_download_diagnostics(&self, params: Value) -> ServerResult<Value> {
        debug!("Downloading error diagnostics");

        let (task_id, cwd) = match self.task_manager.get_active_task() {
            Some(lifecycle) => {
                let lc = lifecycle.lock().await;
                (lc.task_id().to_string(), lc.engine().config().cwd.clone())
            }
            None => {
                let app = self.app.read().await;
                ("".to_string(), app.cwd().to_string())
            }
        };

        let app = self.app.read().await;
        let gsp = app.config().global_storage_path.clone();
        drop(app);

        // Generate diagnostics file
        let diagnostics = json!({
            "taskId": task_id,
            "workspace": cwd,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "values": params.get("values"),
            "settings": {
                "codebaseIndexEnabled": true,
            },
            "system": {
                "platform": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            },
        });

        let diag_dir = if gsp.is_empty() {
            paths::get_global_roo_directory()
        } else {
            std::path::PathBuf::from(&gsp)
        }
        .join("diagnostics");
        let _ = tokio::fs::create_dir_all(&diag_dir).await;
        let diag_path = diag_dir.join(format!(
            "diagnostics-{}.json",
            chrono::Utc::now().format("%Y%m%d%H%M%S")
        ));

        match tokio::fs::write(
            &diag_path,
            serde_json::to_string_pretty(&diagnostics).unwrap_or_default(),
        )
        .await
        {
            Ok(()) => Ok(json!({"status": "generated", "path": diag_path.to_string_lossy()})),
            Err(e) => Ok(json!({"status": "error", "error": e.to_string()})),
        }
    }

    // ── Other ──────────────────────────────────────────────────────────────

    /// Show MDM auth required notification.
    async fn handle_mdm_auth_notification(&self, _params: Value) -> ServerResult<Value> {
        debug!("Showing MDM auth notification");
        Ok(json!({"action": "showMdmAuthNotification"}))
    }

    /// Image generation settings.
    async fn handle_image_generation_settings(&self, _params: Value) -> ServerResult<Value> {
        debug!("Image generation settings");
        Ok(json!({"action": "openImageGenerationSettings"}))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a unique task ID using UUID v7 (time-ordered).
fn generate_task_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Decode a base64 string to bytes without requiring an external crate.
///
/// Supports the standard Base64 alphabet (RFC 4648 §4).
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8; 256] = &{
        let mut table = [0xFFu8; 256];
        let mut i = 0;
        while i < 64 {
            let c = match i {
                0..=25 => b'A' + i as u8,
                26..=51 => b'a' + (i - 26) as u8,
                52..=61 => b'0' + (i - 52) as u8,
                62 => b'+',
                63 => b'/',
                _ => unreachable!(),
            };
            table[c as usize] = i as u8;
            i += 1;
        }
        table
    };

    let input = input.trim_end_matches('=');
    let len = input.len();
    if len == 0 {
        return Ok(Vec::new());
    }

    let mut bytes = Vec::with_capacity(len * 3 / 4);
    let chunks = len / 4;
    let remainder = len % 4;

    for i in 0..chunks {
        let off = i * 4;
        let b0 = TABLE[input.as_bytes()[off] as usize];
        let b1 = TABLE[input.as_bytes()[off + 1] as usize];
        let b2 = TABLE[input.as_bytes()[off + 2] as usize];
        let b3 = TABLE[input.as_bytes()[off + 3] as usize];

        if b0 == 0xFF || b1 == 0xFF || b2 == 0xFF || b3 == 0xFF {
            return Err("invalid base64 character".to_string());
        }

        bytes.push(b0 << 2 | b1 >> 4);
        bytes.push(b1 << 4 | b2 >> 2);
        bytes.push(b2 << 6 | b3);
    }

    if remainder > 0 {
        let off = chunks * 4;
        let b0 = TABLE[input.as_bytes()[off] as usize];
        let b1 = TABLE[input.as_bytes()[off + 1] as usize];
        if b0 == 0xFF || b1 == 0xFF {
            return Err("invalid base64 character".to_string());
        }
        bytes.push(b0 << 2 | b1 >> 4);
        if remainder == 3 {
            let b2 = TABLE[input.as_bytes()[off + 2] as usize];
            if b2 == 0xFF {
                return Err("invalid base64 character".to_string());
            }
            bytes.push(b1 << 4 | b2 >> 2);
        }
    }

    Ok(bytes)
}

/// Convert a [`TaskEvent`] to a JSON-RPC notification message.
///
/// Source: TS `postStateToWebview()` — converts internal events to
/// webview-compatible messages.
fn task_event_to_notification(event: &TaskEvent, task_id: &str) -> Option<Message> {
    let (event_type, data) = match event {
        TaskEvent::StateChanged { from, to } => (
            "stateChanged",
            json!({
                "taskId": task_id,
                "from": format!("{}", from),
                "to": format!("{}", to),
            }),
        ),
        TaskEvent::MessageCreated { message } => (
            "messageCreated",
            json!({
                "taskId": task_id,
                "message": serde_json::to_value(message).ok(),
            }),
        ),
        TaskEvent::MessageUpdated { message } => (
            "messageUpdated",
            json!({
                "taskId": task_id,
                "message": serde_json::to_value(message).ok(),
            }),
        ),
        TaskEvent::ToolExecuted { tool_name, success } => (
            "toolExecuted",
            json!({
                "taskId": task_id,
                "toolName": tool_name,
                "success": success,
            }),
        ),
        TaskEvent::TokenUsageUpdated { usage } => (
            "tokenUsageUpdated",
            json!({
                "taskId": task_id,
                "usage": serde_json::to_value(usage).ok(),
            }),
        ),
        TaskEvent::TaskStarted { .. } => ("taskStarted", json!({"taskId": task_id})),
        TaskEvent::TaskCompleted {
            token_usage,
            tool_usage,
            is_subtask,
            ..
        } => (
            "taskCompleted",
            json!({
                "taskId": task_id,
                "tokenUsage": serde_json::to_value(token_usage).ok(),
                "toolUsage": serde_json::to_value(tool_usage).ok(),
                "isSubtask": is_subtask,
            }),
        ),
        TaskEvent::TaskAborted { reason, .. } => {
            ("taskAborted", json!({"taskId": task_id, "reason": reason}))
        }
        TaskEvent::TaskPaused { .. } => ("taskPaused", json!({"taskId": task_id})),
        TaskEvent::TaskUnpaused { .. } => ("taskUnpaused", json!({"taskId": task_id})),
        TaskEvent::TaskDelegated {
            parent_task_id,
            child_task_id,
        } => (
            "taskDelegated",
            json!({"parentTaskId": parent_task_id, "childTaskId": child_task_id}),
        ),
        TaskEvent::TaskInteractive { .. } => ("taskInteractive", json!({"taskId": task_id})),
        TaskEvent::TaskIdle { .. } => ("taskIdle", json!({"taskId": task_id})),
        TaskEvent::TaskResumable { .. } => ("taskResumable", json!({"taskId": task_id})),
        TaskEvent::ApiRequestStarted { .. } => ("apiRequestStarted", json!({"taskId": task_id})),
        TaskEvent::ApiRequestRetryDelayed {
            delay_seconds,
            retry_attempt,
            ..
        } => (
            "apiRequestRetryDelayed",
            json!({
                "taskId": task_id,
                "delaySeconds": delay_seconds,
                "retryAttempt": retry_attempt,
            }),
        ),
        TaskEvent::ApiRequestFinished {
            cost,
            tokens_in,
            tokens_out,
            ..
        } => (
            "apiRequestFinished",
            json!({
                "taskId": task_id,
                "cost": cost,
                "tokensIn": tokens_in,
                "tokensOut": tokens_out,
            }),
        ),
        TaskEvent::ContextCondensationRequested { .. } => {
            ("contextCondensationRequested", json!({"taskId": task_id}))
        }
        TaskEvent::ContextCondensationCompleted {
            messages_removed, ..
        } => (
            "contextCondensationCompleted",
            json!({"taskId": task_id, "messagesRemoved": messages_removed}),
        ),
        TaskEvent::ContextTruncationPerformed {
            messages_removed, ..
        } => (
            "contextTruncationPerformed",
            json!({"taskId": task_id, "messagesRemoved": messages_removed}),
        ),
        TaskEvent::CheckpointSaved { commit, .. } => (
            "checkpointSaved",
            json!({"taskId": task_id, "commit": commit}),
        ),
        TaskEvent::CheckpointRestored { .. } => ("checkpointRestored", json!({"taskId": task_id})),
        TaskEvent::TaskSpawned {
            parent_task_id,
            child_task_id,
        } => (
            "taskSpawned",
            json!({"parentTaskId": parent_task_id, "childTaskId": child_task_id}),
        ),
        TaskEvent::TaskDelegationCompleted {
            parent_task_id,
            child_task_id,
            summary,
        } => (
            "taskDelegationCompleted",
            json!({"parentTaskId": parent_task_id, "childTaskId": child_task_id, "summary": summary}),
        ),
        TaskEvent::TaskDelegationResumed {
            parent_task_id,
            child_task_id,
        } => (
            "taskDelegationResumed",
            json!({"parentTaskId": parent_task_id, "childTaskId": child_task_id}),
        ),
        TaskEvent::TaskModeSwitched { mode, .. } => {
            ("taskModeSwitched", json!({"taskId": task_id, "mode": mode}))
        }
        TaskEvent::StreamingTextDelta { text, .. } => (
            "streamingTextDelta",
            json!({"taskId": task_id, "text": text}),
        ),
        TaskEvent::StreamingToolUseStarted {
            tool_name, tool_id, ..
        } => (
            "streamingToolUseStarted",
            json!({"taskId": task_id, "toolName": tool_name, "toolId": tool_id}),
        ),
        TaskEvent::StreamingToolUseCompleted {
            tool_name,
            tool_id,
            success,
            ..
        } => (
            "streamingToolUseCompleted",
            json!({"taskId": task_id, "toolName": tool_name, "toolId": tool_id, "success": success}),
        ),
        TaskEvent::StreamingCompleted { .. } => ("streamingCompleted", json!({"taskId": task_id})),
        TaskEvent::StreamingReasoningDelta { text, task_id } => (
            "streamingReasoningDelta",
            json!({"taskId": task_id, "text": text}),
        ),
        TaskEvent::StreamingToolUseDelta {
            task_id,
            tool_id,
            delta,
        } => (
            "streamingToolUseDelta",
            json!({"taskId": task_id, "toolId": tool_id, "delta": delta}),
        ),
        TaskEvent::Error {
            task_id: _tid,
            error,
        } => ("error", json!({"taskId": task_id, "error": error})),
        TaskEvent::ApiRateLimitWait {
            task_id: _tid,
            seconds,
        } => (
            "apiRateLimitWait",
            json!({"taskId": task_id, "seconds": seconds}),
        ),
        TaskEvent::ToolError {
            task_id,
            tool_name,
            error,
        } => (
            "toolError",
            json!({"taskId": task_id, "toolName": tool_name, "error": error}),
        ),
        // --- New event types matching TS RooCodeEventName ---
        TaskEvent::TaskCreated { .. } => ("taskCreated", json!({"taskId": task_id})),
        TaskEvent::TaskFocused { .. } => ("taskFocused", json!({"taskId": task_id})),
        TaskEvent::TaskUnfocused { .. } => ("taskUnfocused", json!({"taskId": task_id})),
        TaskEvent::TaskActive { .. } => ("taskActive", json!({"taskId": task_id})),
        TaskEvent::Message {
            action, message, ..
        } => (
            "message",
            json!({
                "taskId": task_id,
                "action": action,
                "message": serde_json::to_value(message).ok(),
            }),
        ),
        TaskEvent::TaskAskResponded { .. } => ("taskAskResponded", json!({"taskId": task_id})),
        TaskEvent::QueuedMessagesUpdated { messages, .. } => (
            "queuedMessagesUpdated",
            json!({"taskId": task_id, "messages": serde_json::to_value(messages).ok()}),
        ),
        TaskEvent::TaskTokenUsageUpdated {
            token_usage,
            tool_usage,
            ..
        } => (
            "taskTokenUsageUpdated",
            json!({
                "taskId": task_id,
                "tokenUsage": serde_json::to_value(token_usage).ok(),
                "toolUsage": serde_json::to_value(tool_usage).ok(),
            }),
        ),
        TaskEvent::TaskToolFailed {
            tool_name, error, ..
        } => (
            "taskToolFailed",
            json!({"taskId": task_id, "toolName": tool_name, "error": error}),
        ),
        TaskEvent::ModeChanged { mode } => ("modeChanged", json!({"mode": mode})),
        TaskEvent::ProviderProfileChanged { name, provider } => (
            "providerProfileChanged",
            json!({"name": name, "provider": provider}),
        ),
        TaskEvent::UserMessage { .. } => ("taskUserMessage", json!({"taskId": task_id})),
        TaskEvent::InteractionRequired { .. } => {
            ("interactionRequired", json!({"taskId": task_id}))
        }
        TaskEvent::ToolApprovalRequired {
            tool_name,
            tool_id,
            reason,
            ..
        } => (
            "toolApprovalRequired",
            json!({"taskId": task_id, "toolName": tool_name, "toolId": tool_id, "reason": reason}),
        ),
        TaskEvent::ApiRequestFailed { error, .. } => (
            "apiRequestFailed",
            json!({"taskId": task_id, "error": error}),
        ),
        TaskEvent::MistakeLimitReached { count, limit, .. } => (
            "mistakeLimitReached",
            json!({"taskId": task_id, "count": count, "limit": limit}),
        ),
        TaskEvent::AutoApprovalLimitReached { .. } => {
            ("autoApprovalLimitReached", json!({"taskId": task_id}))
        }
        TaskEvent::FollowupQuestion {
            tool_call_id,
            question_json,
            ..
        } => (
            "followupQuestion",
            json!({"taskId": task_id, "toolCallId": tool_call_id, "question": question_json}),
        ),
        TaskEvent::CompletionResult {
            tool_call_id,
            completion_text,
            ..
        } => (
            "completionResult",
            json!({"taskId": task_id, "toolCallId": tool_call_id, "completionText": completion_text}),
        ),
    };

    Some(Message::notification(
        methods::NOTIFICATION_TASK_EVENT,
        json!({
            "type": event_type,
            "data": data,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use roo_app::AppConfig;

    fn test_handler() -> Handler {
        let config = AppConfig {
            cwd: "/tmp/test".to_string(),
            mode: "code".to_string(),
            ..Default::default()
        };
        Handler::new(App::new(config))
    }

    #[tokio::test]
    async fn test_initialize() {
        let handler = test_handler();
        let request = Message::request(1, methods::INITIALIZE, json!(null));
        let response = handler.handle(&request).await;
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["initialized"], true);
        assert_eq!(result["mode"], "code");
    }

    #[tokio::test]
    async fn test_ping() {
        let handler = test_handler();
        let request = Message::request(2, methods::PING, json!(null));
        let response = handler.handle(&request).await;
        assert_eq!(response.result.unwrap(), json!("pong"));
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let handler = test_handler();
        let request = Message::request(3, "nonexistent/method", json!(null));
        let response = handler.handle(&request).await;
        assert!(response.error.is_some());
        assert_eq!(
            response.error.unwrap().code,
            roo_jsonrpc::types::error_codes::METHOD_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn test_state_get() {
        let handler = test_handler();
        let init_request = Message::request(99, methods::INITIALIZE, json!(null));
        handler.handle(&init_request).await;

        let request = Message::request(4, methods::STATE_GET, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["initialized"], true);
        assert_eq!(result["mode"], "code");
    }

    #[tokio::test]
    async fn test_state_set_mode() {
        let handler = test_handler();
        let init_request = Message::request(99, methods::INITIALIZE, json!(null));
        handler.handle(&init_request).await;

        let request = Message::request(5, methods::STATE_SET_MODE, json!({"mode": "architect"}));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["mode"], "architect");
    }

    #[tokio::test]
    async fn test_state_set_mode_missing_param() {
        let handler = test_handler();
        let request = Message::request(6, methods::STATE_SET_MODE, json!({}));
        let response = handler.handle(&request).await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn test_system_prompt_build() {
        let handler = test_handler();
        let request = Message::request(7, methods::SYSTEM_PROMPT_BUILD, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        let prompt = result["prompt"].as_str().unwrap();
        assert!(prompt.contains("TOOL USE"));
    }

    #[tokio::test]
    async fn test_task_start() {
        let handler = test_handler();
        let init_request = Message::request(99, methods::INITIALIZE, json!(null));
        handler.handle(&init_request).await;

        let request = Message::request(
            8,
            methods::TASK_START,
            json!({"text": "Hello", "mode": "code"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "started");
        assert_eq!(result["mode"], "code");
        // Verify task ID is a valid UUID
        let task_id = result["taskId"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(task_id).is_ok());

        // Verify task is stored in TaskManager
        assert!(handler.task_manager.get_task(task_id).is_some());
    }

    #[tokio::test]
    async fn test_task_start_emits_event() {
        let handler = test_handler();
        let init_request = Message::request(99, methods::INITIALIZE, json!(null));
        handler.handle(&init_request).await;

        let request = Message::request(
            8,
            methods::TASK_START,
            json!({"text": "Hello", "mode": "code"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "started");

        // Verify that event notifications were generated
        let notifications = handler.drain_notifications();
        assert!(
            !notifications.is_empty(),
            "Should have emitted event notifications"
        );

        // At least one notification should be a taskStarted event
        let has_started = notifications.iter().any(|n| {
            n.params
                .as_ref()
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                .map_or(false, |t| t == "taskStarted")
        });
        assert!(
            has_started,
            "Should have emitted a taskStarted notification"
        );
    }

    #[tokio::test]
    async fn test_task_cancel() {
        let handler = test_handler();

        // Start a task first
        let start_request = Message::request(
            99,
            methods::TASK_START,
            json!({"text": "test", "mode": "code"}),
        );
        let start_response = handler.handle(&start_request).await;
        let task_id = start_response.result.unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_string();

        // Cancel the task — cancel_current_request sets the abort flag
        let request = Message::request(9, methods::TASK_CANCEL, json!({"taskId": task_id}));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        // cancel_current_request sets abort=true, state remains as-is (Idle)
        assert_eq!(result["taskId"], task_id);
    }

    #[tokio::test]
    async fn test_task_cancel_no_active() {
        let handler = test_handler();
        let request = Message::request(9, methods::TASK_CANCEL, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "cancelled");
    }

    #[tokio::test]
    async fn test_task_close() {
        let handler = test_handler();

        // Start a task first
        let start_request = Message::request(
            99,
            methods::TASK_START,
            json!({"text": "test", "mode": "code"}),
        );
        let start_response = handler.handle(&start_request).await;
        let task_id = start_response.result.unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_string();

        // Close the task
        let request = Message::request(10, methods::TASK_CLOSE, json!({"taskId": task_id}));
        let response = handler.handle(&request).await;
        assert_eq!(response.result.unwrap()["status"], "closed");

        // Verify task is removed
        assert!(handler.task_manager.get_task(&task_id).is_none());
    }

    #[tokio::test]
    async fn test_task_get_modes() {
        let handler = test_handler();
        let request = Message::request(10, methods::TASK_GET_MODES, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        let modes = result["modes"].as_array().unwrap();
        assert!(!modes.is_empty());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let handler = test_handler();
        let init_request = Message::request(99, methods::INITIALIZE, json!(null));
        handler.handle(&init_request).await;

        let request = Message::request(11, methods::SHUTDOWN, json!(null));
        let response = handler.handle(&request).await;
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_file_read_missing_path() {
        let handler = test_handler();
        let request = Message::request(12, methods::FILE_READ, json!({}));
        let response = handler.handle(&request).await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn test_file_read_nonexistent() {
        let handler = test_handler();
        let request = Message::request(
            13,
            methods::FILE_READ,
            json!({"path": "/nonexistent/file.txt"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert!(result["error"].is_string());
    }

    #[tokio::test]
    async fn test_history_delete_missing_id() {
        let handler = test_handler();
        let request = Message::request(14, methods::HISTORY_DELETE, json!({}));
        let response = handler.handle(&request).await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn test_task_send_message() {
        let handler = test_handler();

        // Start a task first
        let start_request = Message::request(
            99,
            methods::TASK_START,
            json!({"text": "test", "mode": "code"}),
        );
        handler.handle(&start_request).await;

        let request = Message::request(
            15,
            methods::TASK_SEND_MESSAGE,
            json!({"text": "Hello world"}),
        );
        let response = handler.handle(&request).await;
        assert_eq!(response.result.unwrap()["status"], "sent");
    }

    #[tokio::test]
    async fn test_task_send_message_no_active() {
        let handler = test_handler();
        let request = Message::request(
            15,
            methods::TASK_SEND_MESSAGE,
            json!({"text": "Hello world"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "error");
    }

    #[tokio::test]
    async fn test_todo_update() {
        let handler = test_handler();
        let todos = json!([{"text": "Task 1", "status": "completed"}]);
        let request = Message::request(
            17,
            methods::TODO_UPDATE,
            json!({"todos": todos, "taskId": "test-task"}),
        );
        let response = handler.handle(&request).await;
        assert_eq!(response.result.unwrap()["status"], "updated");
    }

    #[tokio::test]
    async fn test_mcp_list_servers() {
        let handler = test_handler();
        let request = Message::request(18, methods::MCP_LIST_SERVERS, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert!(result["servers"].is_array());
    }

    #[tokio::test]
    async fn test_prompt_enhance_missing_text() {
        let handler = test_handler();
        let request = Message::request(19, methods::PROMPT_ENHANCE, json!({}));
        let response = handler.handle(&request).await;
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn test_prompt_enhance_returns_text() {
        let handler = test_handler();
        let request = Message::request(
            20,
            methods::PROMPT_ENHANCE,
            json!({"text": "Write a hello world"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        // Without a real provider, returns original text
        assert!(result["enhancedText"].is_string());
    }

    #[tokio::test]
    async fn test_task_get_commands() {
        let handler = test_handler();
        let request = Message::request(21, methods::TASK_GET_COMMANDS, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert!(result["commands"].is_array());
    }

    #[tokio::test]
    async fn test_ask_response_no_active() {
        let handler = test_handler();
        let request = Message::request(22, methods::ASK_RESPONSE, json!({"askResponse": "yes"}));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "error");
    }

    #[tokio::test]
    async fn test_ask_response_with_active_task() {
        let handler = test_handler();

        // Start a task first
        let start_request = Message::request(
            99,
            methods::TASK_START,
            json!({"text": "test", "mode": "code"}),
        );
        handler.handle(&start_request).await;

        // Send ask response
        let request = Message::request(
            22,
            methods::ASK_RESPONSE,
            json!({"askResponse": "yes", "text": "My answer"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "responded");
    }

    #[tokio::test]
    async fn test_terminal_operation_no_registry() {
        let handler = test_handler();
        // Without initialization, terminal registry is not available
        let request = Message::request(
            23,
            methods::TERMINAL_OPERATION,
            json!({"operation": "continue"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "error");
    }

    #[tokio::test]
    async fn test_checkpoint_diff_no_active() {
        let handler = test_handler();
        let request = Message::request(
            24,
            methods::CHECKPOINT_DIFF,
            json!({"commitHash": "abc123"}),
        );
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert!(result["error"].is_string());
    }

    #[tokio::test]
    async fn test_generate_task_id_is_uuid() {
        let id = generate_task_id();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }

    #[tokio::test]
    async fn test_task_manager_integration() {
        let handler = test_handler();

        // Start two tasks
        let start1 = Message::request(
            100,
            methods::TASK_START,
            json!({"text": "Task 1", "mode": "code"}),
        );
        let resp1 = handler.handle(&start1).await;
        let id1 = resp1.result.unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_string();

        let start2 = Message::request(
            101,
            methods::TASK_START,
            json!({"text": "Task 2", "mode": "architect"}),
        );
        let resp2 = handler.handle(&start2).await;
        let id2 = resp2.result.unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_string();

        // Both tasks should be in the manager
        assert_eq!(handler.task_manager.list_tasks().len(), 2);

        // Active should be id2 (last created)
        let active = handler.task_manager.get_active_task().unwrap();
        let lc = active.lock().await;
        assert_eq!(lc.task_id(), id2);
        drop(lc);

        // Close task 1
        let close1 = Message::request(102, methods::TASK_CLOSE, json!({"taskId": id1}));
        handler.handle(&close1).await;
        assert_eq!(handler.task_manager.list_tasks().len(), 1);
        assert!(handler.task_manager.get_task(&id1).is_none());
    }

    #[tokio::test]
    async fn test_drain_notifications() {
        let handler = test_handler();

        // Initially empty
        let notifications = handler.drain_notifications();
        assert!(notifications.is_empty());

        // After draining, should be empty again
        let notifications = handler.drain_notifications();
        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn test_task_condense_no_active() {
        let handler = test_handler();
        let request = Message::request(25, methods::TASK_CONDENSE, json!(null));
        let response = handler.handle(&request).await;
        let result = response.result.unwrap();
        assert_eq!(result["status"], "error");
    }

    #[test]
    fn test_task_event_to_notification() {
        let event = TaskEvent::TaskStarted {
            task_id: "test-123".to_string(),
        };
        let notification = task_event_to_notification(&event, "test-123");
        assert!(notification.is_some());
        let msg = notification.unwrap();
        assert_eq!(
            msg.method,
            Some(methods::NOTIFICATION_TASK_EVENT.to_string())
        );
    }

    #[test]
    fn test_task_event_streaming_text_delta() {
        let event = TaskEvent::StreamingTextDelta {
            task_id: "test-123".to_string(),
            text: "Hello world".to_string(),
        };
        let notification = task_event_to_notification(&event, "test-123");
        assert!(notification.is_some());
        let msg = notification.unwrap();
        let params = msg.params.unwrap();
        assert_eq!(params["type"], "streamingTextDelta");
        assert_eq!(params["data"]["text"], "Hello world");
    }

    #[test]
    fn test_task_event_streaming_tool_use_started() {
        let event = TaskEvent::StreamingToolUseStarted {
            task_id: "test-123".to_string(),
            tool_name: "read_file".to_string(),
            tool_id: "call_1".to_string(),
        };
        let notification = task_event_to_notification(&event, "test-123");
        assert!(notification.is_some());
        let msg = notification.unwrap();
        let params = msg.params.unwrap();
        assert_eq!(params["type"], "streamingToolUseStarted");
        assert_eq!(params["data"]["toolName"], "read_file");
    }
}
