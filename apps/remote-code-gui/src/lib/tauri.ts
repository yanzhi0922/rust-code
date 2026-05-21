import { invoke } from '@tauri-apps/api/core';
import { listen, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AgentStatusChangedInfo,
  AgentTypeInfo,
  AgentType,
  BatchProgressInfo,
  ConfigScope,
  CodexAppServerRequest,
  CodexConfigBatchWriteRequest,
  CodexConfigReadResponse,
  CodexConfigValueWriteRequest,
  CodexConfigWriteResponse,
  CodexDeviceKeySignRequest,
  CodexEmptyResponse,
  CodexExecRequest,
  CodexExecResizeRequest,
  CodexExecResponse,
  CodexExecWriteRequest,
  CodexExperimentalFeatureSetRequest,
  CodexExternalAgentConfigImportRequest,
  CodexFeedbackResponse,
  CodexFeedbackRequest,
  CodexFsCopyRequest,
  CodexFsPathRequest,
  CodexFsWriteFileRequest,
  CodexFuzzyFileSearchRequest,
  CodexJsonValue,
  CodexMarketplaceRequest,
  CodexMcpOAuthLoginRequest,
  CodexMcpResourceReadResponse,
  CodexMcpResourceReadRequest,
  CodexMcpStatusResponse,
  CodexMcpStatusRequest,
  CodexMcpToolCallResponse,
  CodexMcpToolCallRequest,
  CodexAppServerNotificationInfo,
  CodexMemoryModeRequest,
  CodexRecoverableErrorInfo,
  CodexAccountLoginRequest,
  CodexPluginInstallRequest,
  CodexPluginListRequest,
  CodexPluginReadRequest,
  CodexPluginUninstallRequest,
  CodexReviewStartRequest,
  CodexRealtimeAppendTextRequest,
  CodexRealtimeRequest,
  CodexSkillsConfigWriteRequest,
  CodexSkillsListRequest,
  CodexThreadArchiveRequest,
  CodexThreadArchiveResponse,
  CodexThreadGoalRequest,
  CodexThreadGoalSetRequest,
  CodexThreadListRequest,
  CodexThreadListResponse,
  CodexThreadMetadataUpdateRequest,
  CodexThreadNativeRequest,
  CodexThreadReadResponse,
  CodexThreadRefRequest,
  CodexThreadRollbackRequest,
  CodexThreadShellCommandRequest,
  CodexThreadSessionResponse,
  CodexThreadSetNameRequest,
  CodexThreadTurnsListRequest,
  CodexTurnInterruptRequest,
  CodexTurnStartRequest,
  CodexTurnSteerRequest,
  ConversationEntry,
  ContextCompactedInfo,
  ContextOverflowInfo,
  ContextUsageInfo,
  DoctorReportInfo,
  FullSettings,
  InitResult,
  McpMutationResult,
  McpServerDraft,
  McpServerListInfo,
  PermissionDecisionInfo,
  PermissionRequestInfo,
  ProjectInfo,
  PromptDoneInfo,
  ProviderConfig,
  ProviderConfigList,
  ProviderInfo,
  RuntimeMcpInventoryInfo,
  RuntimeStatusInfo,
  SessionExportFormat,
  SessionExportResult,
  SessionSummary,
  SessionSubtask,
  SubtaskCompletedInfo,
  SubtaskProgressInfo,
  SubtaskStartedInfo,
  TaskSnapshotInfo,
  StreamingDeltaInfo,
  ToolProgressInfo,
  ToolResultInfo,
  UpdateProviderRequest,
} from './types';

export function initApp(): Promise<InitResult> {
  return invoke<InitResult>('init_app');
}

export function listSessions(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>('list_sessions');
}

export function listArchivedSessions(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>('list_archived_sessions');
}

export function getSessionConversation(sessionId: string): Promise<ConversationEntry[]> {
  return invoke<ConversationEntry[]>('get_session_conversation', { sessionId });
}

export function getSessionTasks(sessionId: string): Promise<SessionSubtask[]> {
  return invoke<SessionSubtask[]>('get_session_tasks', { sessionId });
}

export function createSession(title?: string, projectPath?: string, agentType?: AgentType): Promise<string> {
  return invoke<string>('create_session', {
    title: title ?? null,
    projectPath: projectPath ?? null,
    agentType: agentType ?? null,
  });
}

export function getProviderInfo(): Promise<ProviderInfo | null> {
  return invoke<ProviderInfo | null>('get_provider_info');
}

export function getRuntimeStatus(): Promise<RuntimeStatusInfo> {
  return invoke<RuntimeStatusInfo>('get_runtime_status');
}

export function runDoctorReport(
  probeNetwork = false,
  probeProvider = false,
  probeMcp = false,
  includeEnvProviders = false,
): Promise<DoctorReportInfo> {
  return invoke<DoctorReportInfo>('run_doctor_report', {
    probeNetwork,
    probeProvider,
    probeMcp,
    includeEnvProviders,
  });
}

export function exportSessionBundle(
  sessionId: string,
  format: SessionExportFormat,
): Promise<SessionExportResult> {
  return invoke<SessionExportResult>('export_session_bundle', { sessionId, format });
}

export function listMcpServers(
  scope: ConfigScope,
  projectPath: string | null,
  connect = false,
  includeDisabled = true,
): Promise<McpServerListInfo> {
  return invoke<McpServerListInfo>('list_mcp_servers', {
    scope,
    projectPath: projectPath ?? null,
    connect,
    includeDisabled,
  });
}

export function listRuntimeMcpInventory(
  projectPath: string | null,
  connect = false,
  includeDisabled = true,
): Promise<RuntimeMcpInventoryInfo> {
  return invoke<RuntimeMcpInventoryInfo>('list_runtime_mcp_inventory', {
    projectPath: projectPath ?? null,
    connect,
    includeDisabled,
  });
}

export function saveMcpServer(request: McpServerDraft): Promise<McpMutationResult> {
  return invoke<McpMutationResult>('save_mcp_server', { request });
}

export function toggleMcpServer(
  scope: ConfigScope,
  projectPath: string | null,
  name: string,
  enabled: boolean,
  ifExists = true,
): Promise<McpMutationResult> {
  return invoke<McpMutationResult>('toggle_mcp_server', {
    scope,
    projectPath: projectPath ?? null,
    name,
    enabled,
    ifExists,
  });
}

export function removeMcpServer(
  scope: ConfigScope,
  projectPath: string | null,
  name: string,
  ifExists = true,
): Promise<McpMutationResult> {
  return invoke<McpMutationResult>('remove_mcp_server', {
    scope,
    projectPath: projectPath ?? null,
    name,
    ifExists,
  });
}

export function resetMcpServers(
  scope: ConfigScope,
  projectPath: string | null,
  ifExists = true,
): Promise<McpMutationResult> {
  return invoke<McpMutationResult>('reset_mcp_servers', {
    scope,
    projectPath: projectPath ?? null,
    ifExists,
  });
}

export function sendPrompt(prompt: string, sessionId?: string): Promise<string> {
  return invoke<string>('send_prompt', {
    prompt,
    sessionId: sessionId ?? null,
  });
}

export function cancelPrompt(sessionId: string): Promise<boolean> {
  return invoke<boolean>('cancel_prompt', { sessionId });
}

export function getSettings(): Promise<FullSettings> {
  return invoke<FullSettings>('get_settings');
}

export function updateProvider(request: UpdateProviderRequest): Promise<void> {
  return invoke('update_provider', { request });
}

export function codexListThreads(
  params?: CodexThreadListRequest | null,
): Promise<CodexThreadListResponse> {
  return invoke<CodexThreadListResponse>('codex_list_threads', { params: params ?? null });
}

export function codexReadThread(request: CodexThreadRefRequest): Promise<CodexThreadReadResponse> {
  return invoke<CodexThreadReadResponse>('codex_read_thread', { request });
}

export function codexResumeThread(
  request: CodexThreadRefRequest,
): Promise<CodexThreadSessionResponse> {
  return invoke<CodexThreadSessionResponse>('codex_resume_thread', { request });
}

export function codexForkThread(
  request: CodexThreadRefRequest,
): Promise<CodexThreadSessionResponse> {
  return invoke<CodexThreadSessionResponse>('codex_fork_thread', { request });
}

export function codexArchiveThread(
  request: CodexThreadArchiveRequest,
): Promise<CodexThreadArchiveResponse> {
  return invoke<CodexThreadArchiveResponse>('codex_archive_thread', { request });
}

export function codexUnarchiveThread(
  request: CodexThreadArchiveRequest,
): Promise<CodexThreadReadResponse> {
  return invoke<CodexThreadReadResponse>('codex_unarchive_thread', { request });
}

export function codexExec(request: CodexExecRequest): Promise<CodexExecResponse> {
  return invoke<CodexExecResponse>('codex_exec', { request });
}

export function codexAppServerRequest(request: CodexAppServerRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_app_server_request', { request });
}

export function codexThreadSetName(request: CodexThreadSetNameRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_set_name', { request });
}

export function codexCurrentThreadId(sessionId?: string | null): Promise<string | null> {
  return invoke<string | null>('codex_current_thread_id', { sessionId: sessionId ?? null });
}

export function codexThreadGoalSet(request: CodexThreadGoalSetRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_goal_set', { request });
}

export function codexThreadGoalGet(request: CodexThreadGoalRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_goal_get', { request });
}

export function codexThreadGoalClear(request: CodexThreadGoalRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_goal_clear', { request });
}

/** Narrow a goal-set/get response to `{ goal }`. */
export function asGoalResponse(val: CodexJsonValue): { goal: Record<string, unknown> | null } {
  if (val && typeof val === 'object' && !Array.isArray(val)) {
    return val as unknown as { goal: Record<string, unknown> | null };
  }
  return { goal: null };
}

/** Narrow a goal-clear response to `{ cleared }`. */
export function asClearResponse(val: CodexJsonValue): { cleared: boolean } {
  if (val && typeof val === 'object' && !Array.isArray(val)) {
    return { cleared: Boolean((val as Record<string, unknown>)['cleared']) };
  }
  return { cleared: false };
}

export function codexThreadCompactStart(request: CodexThreadGoalRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_compact_start', { request });
}

export function codexThreadRollback(request: CodexThreadRollbackRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_rollback', { request });
}

export function codexThreadTurnsList(
  request: CodexThreadTurnsListRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_turns_list', { request });
}

export function codexTurnSteer(request: CodexTurnSteerRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_turn_steer', { request });
}

export function codexTurnInterrupt(request: CodexTurnInterruptRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_turn_interrupt', { request });
}

export function codexModelList(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_model_list');
}

export function codexCollaborationModeList(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_collaboration_mode_list');
}

export function codexExperimentalFeatureList(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_experimental_feature_list');
}

export function codexExperimentalFeatureSet(
  request: CodexExperimentalFeatureSetRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_experimental_feature_set', { request });
}

export function codexAccountRead(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_read');
}

export function codexAccountRateLimitsRead(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_rate_limits_read');
}

export function codexAppsList(): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_apps_list');
}

export function codexSkillsList(request: CodexSkillsListRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_skills_list', { request });
}

export function codexSkillsConfigWrite(
  request: CodexSkillsConfigWriteRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_skills_config_write', { request });
}

export function codexPluginList(request: CodexPluginListRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_plugin_list', { request });
}

export function codexPluginRead(request: CodexPluginReadRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_plugin_read', { request });
}

export function codexPluginInstall(request: CodexPluginInstallRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_plugin_install', { request });
}

export function codexPluginUninstall(
  request: CodexPluginUninstallRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_plugin_uninstall', { request });
}

export function codexMarketplaceAdd(request: CodexMarketplaceRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_marketplace_add', { request });
}

export function codexMarketplaceRemove(request: CodexMarketplaceRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_marketplace_remove', { request });
}

export function codexMarketplaceUpgrade(request: CodexMarketplaceRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_marketplace_upgrade', { request });
}

export function codexMcpOAuthLogin(request: CodexMcpOAuthLoginRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_mcp_oauth_login', { request });
}

export function codexReviewStart(request: CodexReviewStartRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_review_start', { request });
}

export function codexExecWrite(request: CodexExecWriteRequest): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_exec_write', { request });
}

export function codexExecTerminate(
  processId: string,
  sessionId?: string | null,
): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_exec_terminate', {
    processId,
    sessionId: sessionId ?? null,
  });
}

export function codexExecResize(request: CodexExecResizeRequest): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_exec_resize', { request });
}

export function codexMcpRefresh(sessionId?: string | null): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_mcp_refresh', { sessionId: sessionId ?? null });
}

export function codexMcpStatus(
  request: CodexMcpStatusRequest = {},
): Promise<CodexMcpStatusResponse> {
  return invoke<CodexMcpStatusResponse>('codex_mcp_status', { request });
}

export function codexMcpReadResource(
  request: CodexMcpResourceReadRequest,
): Promise<CodexMcpResourceReadResponse> {
  return invoke<CodexMcpResourceReadResponse>('codex_mcp_read_resource', { request });
}

export function codexMcpCallTool(
  request: CodexMcpToolCallRequest,
): Promise<CodexMcpToolCallResponse> {
  return invoke<CodexMcpToolCallResponse>('codex_mcp_call_tool', { request });
}

export function codexReadConfig(includeLayers = false): Promise<CodexConfigReadResponse> {
  return invoke<CodexConfigReadResponse>('codex_read_config', { includeLayers });
}

export function codexWriteConfigValue(
  request: CodexConfigValueWriteRequest,
): Promise<CodexConfigWriteResponse> {
  return invoke<CodexConfigWriteResponse>('codex_write_config_value', { request });
}

export function codexWriteConfigBatch(
  request: CodexConfigBatchWriteRequest,
): Promise<CodexConfigWriteResponse> {
  return invoke<CodexConfigWriteResponse>('codex_write_config_batch', { request });
}

export function codexUploadFeedback(request: CodexFeedbackRequest): Promise<CodexFeedbackResponse> {
  return invoke<CodexFeedbackResponse>('codex_upload_feedback', { request });
}

export function codexSetThreadMemoryMode(
  request: CodexMemoryModeRequest,
): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_set_thread_memory_mode', { request });
}

export function codexResetMemories(): Promise<CodexEmptyResponse> {
  return invoke<CodexEmptyResponse>('codex_reset_memories');
}

export function codexThreadStart(request: CodexThreadNativeRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_start', { request });
}

export function codexThreadUnsubscribe(request: CodexThreadNativeRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_unsubscribe', {
    request: { sessionId: request.sessionId ?? null, threadId: request.threadId ?? '' },
  });
}

export function codexThreadElicitationIncrement(
  request: CodexThreadNativeRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_elicitation_increment', {
    request: { sessionId: request.sessionId ?? null, threadId: request.threadId ?? '' },
  });
}

export function codexThreadElicitationDecrement(
  request: CodexThreadNativeRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_elicitation_decrement', {
    request: { sessionId: request.sessionId ?? null, threadId: request.threadId ?? '' },
  });
}

export function codexThreadMetadataUpdate(
  request: CodexThreadMetadataUpdateRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_metadata_update', {
    request: {
      sessionId: request.sessionId ?? null,
      threadId: request.threadId ?? '',
      sha: request.sha ?? null,
      branch: request.branch ?? null,
      originUrl: request.originUrl ?? null,
    },
  });
}

export function codexThreadShellCommand(
  request: CodexThreadShellCommandRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_shell_command', { request });
}

export function codexThreadBackgroundTerminalsClean(
  request: CodexThreadNativeRequest = {},
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_background_terminals_clean', {
    request: { sessionId: request.sessionId ?? null, threadId: request.threadId ?? '' },
  });
}

export function codexThreadLoadedList(request: CodexThreadNativeRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_loaded_list', {
    request: { sessionId: request.sessionId ?? null, ...(request.params ?? {}) },
  });
}

export function codexThreadInjectItems(request: CodexThreadNativeRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_thread_inject_items', {
    request: {
      sessionId: request.sessionId ?? null,
      threadId: request.threadId ?? '',
      params: { threadId: request.threadId ?? '', ...(request.params ?? {}) },
    },
  });
}

export function codexTurnStart(request: CodexTurnStartRequest): Promise<CodexJsonValue> {
  const prompt = request.prompt?.trim();
  const input = prompt
    ? [{ type: 'text', text: prompt, textElements: [] }]
    : ((request.params?.input as unknown) ?? []);
  return invoke<CodexJsonValue>('codex_turn_start', {
    request: {
      sessionId: request.sessionId ?? null,
      params: { threadId: request.threadId ?? '', ...(request.params ?? {}), input },
    },
  });
}

export function codexAccountLogin(request: CodexAccountLoginRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_login', { request });
}

export function codexAccountLoginCancel(request: CodexAccountLoginRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_login_cancel', { request });
}

export function codexAccountLogout(sessionId?: string | null): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_logout', { sessionId: sessionId ?? null });
}

export function codexAccountAddCreditsNudge(request: CodexAccountLoginRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_account_add_credits_nudge', { request });
}

export function codexConfigRequirementsRead(sessionId?: string | null): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_config_requirements_read', { sessionId: sessionId ?? null });
}

export function codexExternalAgentConfigDetect(
  request: { includeHome?: boolean; cwds?: string[] | null } = {},
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_external_agent_config_detect', { request });
}

export function codexExternalAgentConfigImport(
  request: CodexExternalAgentConfigImportRequest = {},
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_external_agent_config_import', { request });
}

export function codexWindowsSandboxSetupStart(request: CodexRealtimeRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_windows_sandbox_setup_start', { request });
}

export function codexRealtimeStart(request: CodexRealtimeRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_realtime_start', { request });
}

export function codexRealtimeAppendText(
  request: CodexRealtimeAppendTextRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_realtime_append_text', {
    request: { params: { ...(request.params ?? {}), text: request.text } },
  });
}

export function codexRealtimeStop(request: CodexRealtimeRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_realtime_stop', { request });
}

export function codexRealtimeVoicesList(sessionId?: string | null): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_realtime_voices_list', { sessionId: sessionId ?? null });
}

export function codexDeviceKeyCreate(request: CodexAccountLoginRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_device_key_create', { request });
}

export function codexDeviceKeyPublic(request: CodexAccountLoginRequest = {}): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_device_key_public', { request });
}

export function codexDeviceKeySign(request: CodexDeviceKeySignRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_device_key_sign', { request });
}

export function codexFsReadFile(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_read_file', {
    request: { params: { ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsWriteFile(request: CodexFsWriteFileRequest): Promise<CodexJsonValue> {
  const dataBase64 = btoa(Array.from(new TextEncoder().encode(request.contents), b => String.fromCodePoint(b)).join(''));
  return invoke<CodexJsonValue>('codex_fs_write_file', {
    request: { params: { ...(request.params ?? {}), path: request.path, dataBase64 } },
  });
}

export function codexFsCreateDirectory(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_create_directory', {
    request: { params: { recursive: true, ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsGetMetadata(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_get_metadata', {
    request: { params: { ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsReadDirectory(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_read_directory', {
    request: { params: { ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsRemove(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_remove', {
    request: { params: { recursive: true, force: true, ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsCopy(request: CodexFsCopyRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_copy', {
    request: {
      params: {
        recursive: true,
        ...(request.params ?? {}),
        sourcePath: request.from,
        destinationPath: request.to,
      },
    },
  });
}

export function codexFsWatch(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_watch', {
    request: { params: { watchId: request.path, ...(request.params ?? {}), path: request.path } },
  });
}

export function codexFsUnwatch(request: CodexFsPathRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fs_unwatch', {
    request: { params: { watchId: request.path, ...(request.params ?? {}) } },
  });
}

export function codexFuzzyFileSearch(request: CodexFuzzyFileSearchRequest): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fuzzy_file_search', {
    request: { params: { roots: request.cwd ? [request.cwd] : [], ...(request.params ?? {}), query: request.query } },
  });
}

export function codexFuzzyFileSearchSessionStart(
  request: CodexFuzzyFileSearchRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fuzzy_file_search_session_start', {
    request: {
      params: {
        sessionId: request.query,
        roots: request.cwd ? [request.cwd] : [],
        ...(request.params ?? {}),
      },
    },
  });
}

export function codexFuzzyFileSearchSessionUpdate(
  request: CodexFuzzyFileSearchRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fuzzy_file_search_session_update', {
    request: { params: { sessionId: request.query, ...(request.params ?? {}), query: request.query } },
  });
}

export function codexFuzzyFileSearchSessionStop(
  request: CodexFuzzyFileSearchRequest,
): Promise<CodexJsonValue> {
  return invoke<CodexJsonValue>('codex_fuzzy_file_search_session_stop', {
    request: { params: { sessionId: request.query, ...(request.params ?? {}) } },
  });
}

export function codexAdapterStop(sessionId?: string | null): Promise<void> {
  return invoke<void>('codex_adapter_stop', { sessionId: sessionId ?? null });
}

export function codexAdapterRestart(sessionId?: string | null): Promise<void> {
  return invoke<void>('codex_adapter_restart', { sessionId: sessionId ?? null });
}

export function listProjects(): Promise<ProjectInfo[]> {
  return invoke<ProjectInfo[]>('list_projects');
}

export function addProject(path: string): Promise<ProjectInfo> {
  return invoke<ProjectInfo>('add_project', { path });
}

export function removeProject(path: string): Promise<void> {
  return invoke('remove_project', { path });
}

export function archiveSession(sessionId: string): Promise<void> {
  return invoke('archive_session', { sessionId });
}

export function restoreSession(sessionId: string): Promise<void> {
  return invoke('restore_session', { sessionId });
}

export function pickFolder(): Promise<string | null> {
  return invoke<string | null>('pick_folder');
}

export function listProviderConfigs(): Promise<ProviderConfigList> {
  return invoke<ProviderConfigList>('list_provider_configs');
}

export function saveProviderConfig(config: ProviderConfig, setActive: boolean): Promise<void> {
  return invoke<void>('save_provider_config', { config, setActive });
}

export function deleteProviderConfig(name: string): Promise<void> {
  return invoke<void>('delete_provider_config', { name });
}

export function setActiveProvider(name: string): Promise<void> {
  return invoke<void>('set_active_provider', { name });
}

export function switchProfile(
  providerName: string,
  profileName: string | null,
): Promise<void> {
  return invoke<void>('switch_profile', {
    providerName,
    profileName: profileName ?? null,
  });
}

export interface PermissionResolutionRequest {
  allowed: boolean;
  message?: string | null;
  updated_input?: unknown;
  permission_updates?: unknown[];
  feedback?: string | null;
  content_blocks?: unknown[];
  codex_response?: unknown;
  allow_all?: boolean;
}

export function resolvePermissionRequest(
  requestId: string,
  resolution: PermissionResolutionRequest,
): Promise<boolean> {
  return invoke<boolean>('resolve_permission_request', { requestId, ...resolution });
}

export function onPermissionRequest(
  callback: EventCallback<PermissionRequestInfo>,
): Promise<UnlistenFn> {
  return listen<PermissionRequestInfo>('gui://permission-request', callback);
}

export function onPermissionResolved(
  callback: EventCallback<PermissionDecisionInfo>,
): Promise<UnlistenFn> {
  return listen<PermissionDecisionInfo>('gui://permission-resolved', callback);
}

export function onToolStart(callback: EventCallback<ToolProgressInfo>): Promise<UnlistenFn> {
  return listen<ToolProgressInfo>('gui://tool-start', callback);
}

export function onToolProgress(callback: EventCallback<ToolProgressInfo>): Promise<UnlistenFn> {
  return listen<ToolProgressInfo>('gui://tool-progress', callback);
}

export function onToolResult(callback: EventCallback<ToolResultInfo>): Promise<UnlistenFn> {
  return listen<ToolResultInfo>('gui://tool-result', callback);
}

export function onCodexAppServerNotification(
  callback: EventCallback<CodexAppServerNotificationInfo>,
): Promise<UnlistenFn> {
  return listen<CodexAppServerNotificationInfo>('gui://codex-app-server-notification', callback);
}

export function onCodexRecoverableError(
  callback: EventCallback<CodexRecoverableErrorInfo>,
): Promise<UnlistenFn> {
  return listen<CodexRecoverableErrorInfo>('gui://codex-recoverable-error', callback);
}

export function onStreamingDelta(
  callback: EventCallback<StreamingDeltaInfo>,
): Promise<UnlistenFn> {
  return listen<StreamingDeltaInfo>('gui://streaming-delta', callback);
}

export function onPromptDone(callback: EventCallback<PromptDoneInfo>): Promise<UnlistenFn> {
  return listen<PromptDoneInfo>('gui://prompt-done', callback);
}

export function onSubtaskStarted(
  callback: EventCallback<SubtaskStartedInfo>,
): Promise<UnlistenFn> {
  return listen<SubtaskStartedInfo>('gui://subtask-started', callback);
}

export function onSubtaskProgress(
  callback: EventCallback<SubtaskProgressInfo>,
): Promise<UnlistenFn> {
  return listen<SubtaskProgressInfo>('gui://subtask-progress', callback);
}

export function onSubtaskCompleted(
  callback: EventCallback<SubtaskCompletedInfo>,
): Promise<UnlistenFn> {
  return listen<SubtaskCompletedInfo>('gui://subtask-completed', callback);
}

export function onBatchProgress(
  callback: EventCallback<BatchProgressInfo>,
): Promise<UnlistenFn> {
  return listen<BatchProgressInfo>('gui://batch-progress', callback);
}

export function onTaskSnapshot(
  callback: EventCallback<TaskSnapshotInfo>,
): Promise<UnlistenFn> {
  return listen<TaskSnapshotInfo>('gui://task-snapshot', callback);
}

export function onContextUsage(
  callback: EventCallback<ContextUsageInfo>,
): Promise<UnlistenFn> {
  return listen<ContextUsageInfo>('gui://context-usage', callback);
}

export function onContextOverflow(
  callback: EventCallback<ContextOverflowInfo>,
): Promise<UnlistenFn> {
  return listen<ContextOverflowInfo>('gui://context-overflow', callback);
}

export function onContextCompacted(
  callback: EventCallback<ContextCompactedInfo>,
): Promise<UnlistenFn> {
  return listen<ContextCompactedInfo>('gui://context-compacted', callback);
}

export function onRuntimeStatus(
  callback: EventCallback<RuntimeStatusInfo>,
): Promise<UnlistenFn> {
  return listen<RuntimeStatusInfo>('gui://runtime-status', callback);
}

// ── Multi-Agent APIs ────────────────────────────────────────────────

export function listAvailableAgents(): Promise<AgentTypeInfo[]> {
  return invoke<AgentTypeInfo[]>('list_available_agents');
}

export function installAgent(agentType: AgentType): Promise<void> {
  return invoke<void>('install_agent', { agentType });
}

export function uninstallAgent(agentType: AgentType): Promise<void> {
  return invoke<void>('uninstall_agent', { agentType });
}

export function onAgentStatusChanged(
  callback: EventCallback<AgentStatusChangedInfo>,
): Promise<UnlistenFn> {
  return listen<AgentStatusChangedInfo>('gui://agent-status-changed', callback);
}

// ── Voice / STT APIs ────────────────────────────────────────────────

export function resolveRooPermissionRequest(
  requestId: string,
  allowed: boolean,
  message?: string | null,
  feedback?: string | null,
  allow_all?: boolean,
): Promise<boolean> {
  return invoke<boolean>('resolve_roo_permission_request', {
    requestId,
    allowed,
    message: message ?? null,
    feedback: feedback ?? null,
    allowAll: allow_all ?? null,
  });
}

// ── Voice / STT APIs ────────────────────────────────────────────────

/**
 * Transcribe audio data via the Rust STT backend (OpenAI Whisper API).
 *
 * @param audioData - Raw audio bytes (0–255).  Accepts `Uint8Array` for type
 *   safety; the data is spread into a plain array before crossing the
 *   Tauri bridge because the command expects `Vec<u8>`.
 * @param audioFormat - Audio container format (e.g. `"webm"`, `"mp4"`, `"ogg"`, `"wav"`).
 *   Derived from the MIME type when omitted.
 */
export function transcribeAudio(
  audioData: Uint8Array,
  audioFormat?: string,
): Promise<string> {
  return invoke<string>('transcribe_audio', {
    audioData: Array.from(audioData),
    audioFormat: audioFormat || 'webm',
  });
}

// ─── Remote Control ──────────────────────────────────────────────────────────

export type RemoteControlStatus = 'disabled' | 'enabled' | 'running';

export interface RemoteConnectionInfo {
  control_plane_url: string;
  runner_id: string;
  auto_start: boolean;
  configured: boolean;
  running: boolean;
  connected: boolean;
}

export function remoteGetStatus(): Promise<RemoteControlStatus> {
  return invoke<RemoteControlStatus>('remote_get_status');
}

export function remoteSetPassword(password: string): Promise<void> {
  return invoke<void>('remote_set_password', { password });
}

export function remoteSetUsername(username: string): Promise<void> {
  return invoke<void>('remote_set_username', { username });
}

export function remoteSetCredentials(username: string, password: string): Promise<void> {
  return invoke<void>('remote_set_credentials', { username, password });
}

export function remoteGetUsername(): Promise<string | null> {
  return invoke<string | null>('remote_get_username');
}

export function remoteGetConnectionInfo(): Promise<RemoteConnectionInfo> {
  return invoke<RemoteConnectionInfo>('remote_get_connection_info');
}

export function remoteSetConnection(
  controlPlaneUrl: string,
  runnerId?: string,
  autoStart?: boolean,
): Promise<RemoteConnectionInfo> {
  return invoke<RemoteConnectionInfo>('remote_set_connection', {
    controlPlaneUrl,
    runnerId: runnerId || null,
    autoStart: autoStart ?? null,
  });
}

export function remoteStartService(): Promise<RemoteControlStatus> {
  return invoke<RemoteControlStatus>('remote_start_service');
}

export function remoteHasPassword(): Promise<boolean> {
  return invoke<boolean>('remote_has_password');
}
