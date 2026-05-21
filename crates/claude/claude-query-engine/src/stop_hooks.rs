//! Runtime stop-hook types plus retry logic for graceful query termination.
//!
//! Mirrors TS `stopHooks.ts` with a 7-phase execution pipeline:
//! 1. saveCacheSafeParams — persist cache parameters
//! 2. Job classification — determine if blocking
//! 3. Fire-and-forget background — prompt suggestion, memory extraction, auto-dream
//! 4. Computer-use cleanup — release resources
//! 5. User-configured Stop/SubagentStop hooks (streaming)
//! 6. TeammateIdle/TaskCompleted — teammate-only hooks
//! 7. Return — final decision

use std::collections::BTreeMap;
use std::env;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use claude_core::{AgentId, Message, SessionId};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::config::QuerySource;

/// Generic hook context shared by post-sampling and stop-hook callbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplHookContext {
    pub session_id: SessionId,
    pub turn: u32,
    pub messages: Vec<Message>,
    pub query_source: QuerySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub user_context: BTreeMap<String, String>,
    #[serde(default)]
    pub system_context: BTreeMap<String, String>,
}

/// Terminal metadata for a stop-hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StopHookRequest {
    pub stop_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
}

/// Host-directed outcome of a stop-hook callback.
#[derive(Debug, Clone)]
pub enum StopHookOutcome {
    /// The stop is allowed to proceed immediately.
    Allow,
    /// The engine should append the supplied messages and continue the loop.
    Retry { injected_messages: Vec<Message> },
    /// The current stop attempt should be denied.
    Deny,
}

/// Manages stop hook retry behavior for query termination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookManager {
    /// Maximum number of retry attempts for stop hooks.
    max_retries: usize,
    /// Current retry count.
    retry_count: usize,
    /// Whether a stop is currently pending.
    pending_stop: bool,
    /// Reason for the stop request.
    stop_reason: Option<String>,
}

/// Result of a stop hook evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopHookResult {
    /// The stop is allowed to proceed.
    Allow,
    /// The stop should be retried after the hook's feedback is incorporated.
    Retry,
    /// The stop is denied; the query should continue.
    Deny,
}

impl From<&StopHookOutcome> for StopHookResult {
    fn from(value: &StopHookOutcome) -> Self {
        match value {
            StopHookOutcome::Allow => Self::Allow,
            StopHookOutcome::Retry { .. } => Self::Retry,
            StopHookOutcome::Deny => Self::Deny,
        }
    }
}

impl StopHookManager {
    /// Create a new stop hook manager with the given maximum retries.
    #[must_use]
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            retry_count: 0,
            pending_stop: false,
            stop_reason: None,
        }
    }

    /// Returns the maximum number of retries.
    #[must_use]
    pub fn max_retries(&self) -> usize {
        self.max_retries
    }

    /// Returns the current retry count.
    #[must_use]
    pub fn retry_count(&self) -> usize {
        self.retry_count
    }

    /// Returns true if a stop is currently pending.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending_stop
    }

    /// Request a stop with the given reason.
    pub fn request_stop(&mut self, reason: impl Into<String>) {
        self.pending_stop = true;
        self.stop_reason = Some(reason.into());
        self.retry_count = 0;
    }

    /// Evaluate a stop hook result. Returns true if the stop should proceed.
    pub fn evaluate(&mut self, result: StopHookResult) -> bool {
        match result {
            StopHookResult::Allow => {
                self.pending_stop = false;
                true
            }
            StopHookResult::Retry => {
                if self.retry_count >= self.max_retries {
                    self.pending_stop = false;
                    true
                } else {
                    self.retry_count += 1;
                    false
                }
            }
            StopHookResult::Deny => {
                self.pending_stop = false;
                false
            }
        }
    }

    /// Cancel a pending stop request.
    pub fn cancel(&mut self) {
        self.pending_stop = false;
        self.stop_reason = None;
        self.retry_count = 0;
    }

    /// Returns the stop reason if a stop is pending.
    #[must_use]
    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    /// Returns true if retries are exhausted.
    #[must_use]
    pub fn retries_exhausted(&self) -> bool {
        self.retry_count >= self.max_retries
    }

    /// Reset the manager to its initial state.
    pub fn reset(&mut self) {
        self.retry_count = 0;
        self.pending_stop = false;
        self.stop_reason = None;
    }
}

impl Default for StopHookManager {
    fn default() -> Self {
        Self::new(3)
    }
}

// ---------------------------------------------------------------------------
// 7-Phase Stop Hook Pipeline
// ---------------------------------------------------------------------------

/// Base input for all stop hook phases.
/// Mirrors TS `BaseHookInput`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookBaseInput {
    pub session_id: SessionId,
    pub turn: u32,
    pub stop_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub query_source: QuerySource,
    pub messages: Vec<Message>,
}

/// Result of the full stop hook pipeline.
#[derive(Debug, Clone)]
pub struct StopHookPipelineResult {
    /// Whether the stop is allowed to proceed.
    pub should_stop: bool,
    /// Messages to inject if retrying.
    pub injected_messages: Vec<Message>,
    /// Blocking errors from phase 2 (job classification) or hook errors.
    pub blocking_errors: Vec<String>,
    /// Whether continuation should be prevented.
    pub prevent_continuation: bool,
    /// Which phases were executed.
    pub phases_executed: Vec<StopHookPhase>,
    /// Number of user-configured hooks that ran.
    pub hook_count: usize,
    /// Per-hook execution info for summary.
    pub hook_infos: Vec<HookInfo>,
    /// Hook error messages (non-blocking).
    pub hook_errors: Vec<String>,
    /// Optional summary message from the hooks phase.
    pub summary_message: Option<String>,
}

impl Default for StopHookPipelineResult {
    fn default() -> Self {
        Self {
            should_stop: true,
            injected_messages: Vec::new(),
            blocking_errors: Vec::new(),
            prevent_continuation: false,
            phases_executed: Vec::new(),
            hook_count: 0,
            hook_infos: Vec::new(),
            hook_errors: Vec::new(),
            summary_message: None,
        }
    }
}

/// Phases in the stop hook pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopHookPhase {
    /// Phase 1: Save cache safe parameters.
    SaveCacheSafeParams,
    /// Phase 2: Job classification (blocking).
    JobClassification,
    /// Phase 3: Fire-and-forget background hooks.
    BackgroundFireAndForget,
    /// Phase 4: Computer-use cleanup.
    ComputerUseCleanup,
    /// Phase 5: User-configured Stop/SubagentStop hooks.
    UserConfiguredStopHooks,
    /// Phase 6: TeammateIdle/TaskCompleted hooks.
    TeammateHooks,
    /// Phase 7: Return.
    Return,
}

impl std::fmt::Display for StopHookPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveCacheSafeParams => write!(f, "saveCacheSafeParams"),
            Self::JobClassification => write!(f, "jobClassification"),
            Self::BackgroundFireAndForget => write!(f, "backgroundFireAndForget"),
            Self::ComputerUseCleanup => write!(f, "computerUseCleanup"),
            Self::UserConfiguredStopHooks => write!(f, "userConfiguredStopHooks"),
            Self::TeammateHooks => write!(f, "teammateHooks"),
            Self::Return => write!(f, "return"),
        }
    }
}

/// Definition of a user-configured hook.
/// Mirrors TS hook definitions with event type, command, and optional timeout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Event type: "Stop", "SubagentStop", "TaskCompleted", "TeammateIdle"
    pub event: String,
    /// Command to execute, e.g. ["node", "script.js", "arg"]
    pub command: Vec<String>,
    /// Optional per-hook timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Result of executing a single hook process.
#[derive(Debug, Clone)]
pub struct HookExecutionResult {
    /// Optional message from the hook (stdout).
    pub message: Option<String>,
    /// Blocking error message that should halt the pipeline.
    pub blocking_error: Option<String>,
    /// Whether the hook requests preventing continuation.
    pub prevent_continuation: bool,
    /// Stop reason if the hook forced a stop.
    pub stop_reason: Option<String>,
}

/// Per-hook tracking info for the summary message.
#[derive(Debug, Clone, Default)]
pub struct HookInfo {
    /// The command that was executed.
    pub command: Vec<String>,
    /// Prompt text from hook progress (if applicable).
    pub prompt_text: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: Option<u64>,
}

/// Trait for individual phase handlers in the pipeline.
/// Each phase receives the base input and can modify the pipeline result.
#[async_trait::async_trait]
pub trait StopHookPhaseHandler: Send + Sync {
    /// Execute this phase. Returns Ok(()) to continue, Err to block.
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()>;
}

/// Input passed to the optional template-job classifier callback.
#[derive(Debug, Clone)]
pub struct JobClassificationInput {
    pub job_dir: PathBuf,
    pub session_id: SessionId,
    pub turn: u32,
    pub assistant_messages: Vec<Message>,
}

/// Async callback used by the job classification phase.
pub type JobClassificationCallback = dyn Fn(JobClassificationInput) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>
    + Send
    + Sync;

/// Background task kinds fired after the model stops without blocking the
/// foreground turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskKind {
    /// Generate speculative next-prompt suggestions.
    PromptSuggestion,
    /// Extract durable memories from the completed turn.
    ExtractMemories,
    /// Run auto-dream / memory-consolidation work.
    AutoDream,
}

/// Input passed to the optional background fire-and-forget callback.
#[derive(Debug, Clone)]
pub struct BackgroundFireAndForgetInput {
    pub task: BackgroundTaskKind,
    pub session_id: SessionId,
    pub turn: u32,
    pub stop_reason: String,
    pub final_text: Option<String>,
    pub query_source: QuerySource,
    pub messages: Vec<Message>,
}

/// Async callback used by the background fire-and-forget phase.
pub type BackgroundFireAndForgetCallback = dyn Fn(BackgroundFireAndForgetInput) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>
    + Send
    + Sync;

/// Input passed to the optional computer-use cleanup callback.
#[derive(Debug, Clone)]
pub struct ComputerUseCleanupInput {
    pub session_id: SessionId,
    pub turn: u32,
}

/// Result returned by computer-use cleanup implementations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputerUseCleanupReport {
    pub unhidden_apps: usize,
    pub released_lock: bool,
}

/// Async callback used by the computer-use cleanup phase.
pub type ComputerUseCleanupCallback = dyn Fn(
        ComputerUseCleanupInput,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ComputerUseCleanupReport>> + Send>>
    + Send
    + Sync;

/// Orchestrates the 7-phase stop hook pipeline.
pub struct StopHookPipeline {
    /// Phase handlers indexed by phase.
    handlers: Vec<(StopHookPhase, Box<dyn StopHookPhaseHandler>)>,
    /// Optional abort signal (mirrors TS AbortController.signal).
    abort_signal: Option<Arc<AtomicBool>>,
}

impl StopHookPipeline {
    /// Create a new empty pipeline.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            abort_signal: None,
        }
    }

    /// Set the abort signal for the pipeline.
    /// When the signal is set to `true`, remaining phases are skipped.
    pub fn with_abort_signal(mut self, signal: Arc<AtomicBool>) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Register a handler for a phase.
    pub fn register_phase(&mut self, phase: StopHookPhase, handler: Box<dyn StopHookPhaseHandler>) {
        self.handlers.push((phase, handler));
    }

    /// Check if the abort signal is set.
    fn is_aborted(&self) -> bool {
        self.abort_signal
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }

    /// Execute all registered phases in order.
    /// Phases 1-2 are blocking. Phases 3-4 are fire-and-forget.
    /// Phases 5-6 are user-configured hooks.
    pub async fn execute(&self, input: &StopHookBaseInput) -> StopHookPipelineResult {
        let mut result = StopHookPipelineResult::default();

        for (phase, handler) in &self.handlers {
            // Check abort signal between phases
            if self.is_aborted() {
                result.prevent_continuation = true;
                tracing::info!("Stop hook pipeline aborted before phase {phase}");
                return result;
            }

            result.phases_executed.push(*phase);

            match phase {
                // Phases 1-2: Blocking — errors stop the pipeline
                StopHookPhase::SaveCacheSafeParams | StopHookPhase::JobClassification => {
                    if let Err(err) = handler.execute(input, &mut result).await {
                        result.blocking_errors.push(err.to_string());
                        if *phase == StopHookPhase::JobClassification {
                            result.prevent_continuation = true;
                        }
                    }
                }

                // Phase 3: Fire-and-forget — errors are logged but don't block
                StopHookPhase::BackgroundFireAndForget => {
                    if let Err(err) = handler.execute(input, &mut result).await {
                        tracing::warn!("Background stop hook failed: {err:#}");
                    }
                }

                // Phase 4: Computer-use cleanup — errors are logged
                StopHookPhase::ComputerUseCleanup => {
                    if let Err(err) = handler.execute(input, &mut result).await {
                        tracing::warn!("Computer-use cleanup hook failed: {err:#}");
                    }
                }

                // Phases 5-6: User-configured hooks — can inject messages
                StopHookPhase::UserConfiguredStopHooks | StopHookPhase::TeammateHooks => {
                    if let Err(err) = handler.execute(input, &mut result).await {
                        tracing::warn!("Stop hook phase {phase} failed: {err:#}");
                    }
                }

                StopHookPhase::Return => {
                    // Terminal phase — no action
                }
            }
        }

        result
    }

    /// Create a pipeline with all default handlers.
    /// `stop_hooks` are user-configured hooks for Stop/SubagentStop events.
    /// `teammate_hooks` are user-configured hooks for TaskCompleted/TeammateIdle events.
    /// `abort_signal` is an optional cancellation flag.
    pub fn default_with_handlers(
        stop_hooks: Vec<HookDefinition>,
        teammate_hooks: Vec<HookDefinition>,
        abort_signal: Option<Arc<AtomicBool>>,
    ) -> Self {
        let mut pipeline = Self {
            handlers: Vec::new(),
            abort_signal,
        };

        pipeline.register_phase(
            StopHookPhase::SaveCacheSafeParams,
            Box::new(SaveCacheSafeParamsHandler),
        );
        pipeline.register_phase(
            StopHookPhase::JobClassification,
            Box::new(JobClassificationHandler::default()),
        );
        pipeline.register_phase(
            StopHookPhase::BackgroundFireAndForget,
            Box::new(BackgroundFireAndForgetHandler::default()),
        );
        pipeline.register_phase(
            StopHookPhase::ComputerUseCleanup,
            Box::new(ComputerUseCleanupHandler::default()),
        );
        pipeline.register_phase(
            StopHookPhase::UserConfiguredStopHooks,
            Box::new(UserConfiguredStopHooksHandler::new(stop_hooks)),
        );
        pipeline.register_phase(
            StopHookPhase::TeammateHooks,
            Box::new(TeammateHooksHandler::new(teammate_hooks)),
        );
        pipeline.register_phase(StopHookPhase::Return, Box::new(NoOpPhaseHandler));

        pipeline
    }
}

impl Default for StopHookPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Concrete Phase Handlers
// ---------------------------------------------------------------------------

/// No-op phase handler used for placeholder phases (e.g., Return).
struct NoOpPhaseHandler;

#[async_trait::async_trait]
impl StopHookPhaseHandler for NoOpPhaseHandler {
    async fn execute(
        &self,
        _input: &StopHookBaseInput,
        _result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 1: SaveCacheSafeParamsHandler
// ---------------------------------------------------------------------------

/// Phase 1 handler: saves cache-safe parameters for main session queries.
///
/// Mirrors TS `saveCacheSafeParams` — only runs for `ReplMainThread` and `Sdk`
/// query sources (sub-agents must not overwrite the cache).
pub struct SaveCacheSafeParamsHandler;

#[async_trait::async_trait]
impl StopHookPhaseHandler for SaveCacheSafeParamsHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        _result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        // Only save params for main session queries — subagents must not overwrite.
        match input.query_source {
            QuerySource::ReplMainThread | QuerySource::Sdk => {
                tracing::debug!(
                    session_id = %input.session_id,
                    turn = input.turn,
                    "Saving cache safe params for main session"
                );
                // Placeholder: in a full implementation, this would serialize
                // context parameters and persist them to a cache store.
            }
            _ => {
                tracing::trace!(
                    query_source = ?input.query_source,
                    "Skipping cache save for non-main query source"
                );
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 2: JobClassificationHandler
// ---------------------------------------------------------------------------

const JOB_CLASSIFICATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Phase 2 handler: classifies template job state at the end of a main turn.
///
/// Mirrors TS `feature('TEMPLATES') && CLAUDE_JOB_DIR &&
/// querySource.startsWith('repl_main_thread') && !agentId`. Classification is
/// non-blocking for conversation flow: classifier failures are logged and
/// swallowed, matching the reference `.catch(...)` behavior.
pub struct JobClassificationHandler {
    classifier: Option<Arc<JobClassificationCallback>>,
    timeout: Duration,
}

impl JobClassificationHandler {
    #[must_use]
    pub fn new(classifier: Arc<JobClassificationCallback>) -> Self {
        Self {
            classifier: Some(classifier),
            timeout: JOB_CLASSIFICATION_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for JobClassificationHandler {
    fn default() -> Self {
        Self {
            classifier: None,
            timeout: JOB_CLASSIFICATION_TIMEOUT,
        }
    }
}

#[async_trait::async_trait]
impl StopHookPhaseHandler for JobClassificationHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        _result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        let Some(job_dir) = env::var_os("CLAUDE_JOB_DIR").map(PathBuf::from) else {
            return Ok(());
        };
        if input.agent_id.is_some() || input.query_source != QuerySource::ReplMainThread {
            return Ok(());
        }

        let assistant_messages = input
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Assistant(_)))
            .cloned()
            .collect::<Vec<_>>();

        let Some(classifier) = self.classifier.as_ref() else {
            tracing::debug!(
                job_dir = %job_dir.display(),
                assistant_message_count = assistant_messages.len(),
                "Template job classification skipped because no classifier callback is registered"
            );
            return Ok(());
        };

        let callback_input = JobClassificationInput {
            job_dir,
            session_id: input.session_id.clone(),
            turn: input.turn,
            assistant_messages,
        };

        match timeout(self.timeout, classifier(callback_input)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::error!("Template job classifier error: {err:#}");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = self.timeout.as_millis() as u64,
                    "Template job classifier timed out"
                );
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 3: BackgroundFireAndForgetHandler
// ---------------------------------------------------------------------------

/// Phase 3 handler: fires background tasks (prompt suggestion, memory
/// extraction, auto-dream) without awaiting them.
///
/// Mirrors TS logic: only fires on the main agent (no `agent_id`),
/// and skips entirely in bare mode (scripted `-p` calls).
pub struct BackgroundFireAndForgetHandler {
    callback: Option<Arc<BackgroundFireAndForgetCallback>>,
}

impl BackgroundFireAndForgetHandler {
    #[must_use]
    pub fn new(callback: Arc<BackgroundFireAndForgetCallback>) -> Self {
        Self {
            callback: Some(callback),
        }
    }
}

impl Default for BackgroundFireAndForgetHandler {
    fn default() -> Self {
        Self { callback: None }
    }
}

#[async_trait::async_trait]
impl StopHookPhaseHandler for BackgroundFireAndForgetHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        _result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        // Only fire on the main agent (no agent_id), not in bare mode.
        if input.agent_id.is_some() {
            tracing::trace!("Skipping background hooks for sub-agent");
            return Ok(());
        }

        // Check for bare mode — in TS this is `isBareMode()` which checks env vars.
        // For now, we check if query_source is User (which covers `-p`/bare mode).
        if matches!(input.query_source, QuerySource::User) {
            tracing::trace!("Skipping background hooks in bare/user mode");
            return Ok(());
        }

        let Some(callback) = self.callback.as_ref() else {
            tracing::trace!(
                "Background fire-and-forget phase skipped because no callback is registered"
            );
            return Ok(());
        };

        for task in [
            BackgroundTaskKind::PromptSuggestion,
            BackgroundTaskKind::ExtractMemories,
            BackgroundTaskKind::AutoDream,
        ] {
            let callback = Arc::clone(callback);
            let callback_input = BackgroundFireAndForgetInput {
                task,
                session_id: input.session_id.clone(),
                turn: input.turn,
                stop_reason: input.stop_reason.clone(),
                final_text: input.final_text.clone(),
                query_source: input.query_source,
                messages: input.messages.clone(),
            };
            tokio::spawn(async move {
                if let Err(err) = callback(callback_input).await {
                    tracing::warn!(task = ?task, "Background stop task failed: {err:#}");
                }
            });
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 4: ComputerUseCleanupHandler
// ---------------------------------------------------------------------------

const COMPUTER_USE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Phase 4 handler: releases computer-use resources at turn end.
///
/// The actual resource cleanup is host/app-state dependent, so it is injected
/// as a callback. The phase itself enforces the Claude Code ordering,
/// main-thread-only gate, timeout, and non-blocking failure semantics.
pub struct ComputerUseCleanupHandler {
    cleanup: Option<Arc<ComputerUseCleanupCallback>>,
    timeout: Duration,
}

impl ComputerUseCleanupHandler {
    #[must_use]
    pub fn new(cleanup: Arc<ComputerUseCleanupCallback>) -> Self {
        Self {
            cleanup: Some(cleanup),
            timeout: COMPUTER_USE_CLEANUP_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for ComputerUseCleanupHandler {
    fn default() -> Self {
        Self {
            cleanup: None,
            timeout: COMPUTER_USE_CLEANUP_TIMEOUT,
        }
    }
}

#[async_trait::async_trait]
impl StopHookPhaseHandler for ComputerUseCleanupHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        _result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        if input.agent_id.is_some() {
            return Ok(());
        }

        let Some(cleanup) = self.cleanup.as_ref() else {
            tracing::trace!(
                "Computer-use cleanup skipped because no cleanup callback is registered"
            );
            return Ok(());
        };

        let callback_input = ComputerUseCleanupInput {
            session_id: input.session_id.clone(),
            turn: input.turn,
        };

        match timeout(self.timeout, cleanup(callback_input)).await {
            Ok(Ok(report)) => {
                tracing::debug!(
                    unhidden_apps = report.unhidden_apps,
                    released_lock = report.released_lock,
                    "Computer-use cleanup completed"
                );
            }
            Ok(Err(err)) => {
                tracing::warn!("Computer-use cleanup failed: {err:#}");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = self.timeout.as_millis() as u64,
                    "Computer-use cleanup timed out"
                );
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 5: UserConfiguredStopHooksHandler
// ---------------------------------------------------------------------------

/// Phase 5 handler: runs user-configured Stop/SubagentStop hooks.
///
/// Mirrors TS `executeStopHooks` — spawns each hook as a child process,
/// captures stdout/stderr, and tracks blocking errors and prevent-continuation.
pub struct UserConfiguredStopHooksHandler {
    hooks: Vec<HookDefinition>,
}

impl UserConfiguredStopHooksHandler {
    /// Create a new handler with the given hook definitions.
    pub fn new(hooks: Vec<HookDefinition>) -> Self {
        Self { hooks }
    }
}

#[async_trait::async_trait]
impl StopHookPhaseHandler for UserConfiguredStopHooksHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        // Determine which event type to match.
        let event = if input.agent_id.is_some() {
            "SubagentStop"
        } else {
            "Stop"
        };

        let matching_hooks: Vec<&HookDefinition> =
            self.hooks.iter().filter(|h| h.event == event).collect();

        if matching_hooks.is_empty() {
            return Ok(());
        }

        // Build the JSON input for hooks.
        let input_json = serde_json::json!({
            "session_id": input.session_id.to_string(),
            "turn": input.turn,
            "stop_reason": input.stop_reason,
            "final_text": input.final_text,
            "agent_id": input.agent_id.as_ref().map(|id| id.to_string()),
            "query_source": format!("{:?}", input.query_source),
        });
        let input_str = serde_json::to_string(&input_json).unwrap_or_else(|_| "{}".to_owned());

        let mut hook_count = 0usize;
        let mut hook_errors: Vec<String> = Vec::new();
        let mut hook_infos: Vec<HookInfo> = Vec::new();

        for hook in &matching_hooks {
            let default_timeout = 60_000u64;
            let timeout_ms = hook.timeout_ms.unwrap_or(default_timeout);

            let start = Instant::now();
            let exec_result = execute_hook_process(hook, &input_str, timeout_ms).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            hook_count += 1;
            hook_infos.push(HookInfo {
                command: hook.command.clone(),
                prompt_text: None,
                duration_ms: Some(duration_ms),
            });

            if let Some(ref err) = exec_result.blocking_error {
                result.blocking_errors.push(err.clone());
                hook_errors.push(err.clone());
            }

            if let Some(ref msg) = exec_result.message {
                hook_errors.push(msg.clone());
            }

            if exec_result.prevent_continuation {
                result.prevent_continuation = true;
                tracing::info!(
                    command = ?hook.command,
                    stop_reason = ?exec_result.stop_reason,
                    "Stop hook prevented continuation"
                );
            }
        }

        result.hook_count += hook_count;
        result.hook_infos.extend(hook_infos);
        result.hook_errors.extend(hook_errors.clone());

        if hook_count > 0 {
            result.summary_message = Some(format!(
                "Ran {hook_count} stop hook(s) with {} error(s)",
                hook_errors.len()
            ));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Phase 6: TeammateHooksHandler
// ---------------------------------------------------------------------------

/// Phase 6 handler: runs TaskCompleted and TeammateIdle hooks for teammate agents.
///
/// Mirrors TS teammate hook logic — only runs when the agent is a teammate
/// (has `agent_id`). Runs TaskCompleted hooks for in-progress tasks, then
/// TeammateIdle hooks.
pub struct TeammateHooksHandler {
    hooks: Vec<HookDefinition>,
}

impl TeammateHooksHandler {
    /// Create a new handler with the given hook definitions.
    pub fn new(hooks: Vec<HookDefinition>) -> Self {
        Self { hooks }
    }
}

#[async_trait::async_trait]
impl StopHookPhaseHandler for TeammateHooksHandler {
    async fn execute(
        &self,
        input: &StopHookBaseInput,
        result: &mut StopHookPipelineResult,
    ) -> anyhow::Result<()> {
        // Only run when the agent is a teammate (has an agent_id).
        let agent_id = match &input.agent_id {
            Some(id) => id,
            None => return Ok(()),
        };

        // Build JSON input for teammate hooks.
        let input_json = serde_json::json!({
            "session_id": input.session_id.to_string(),
            "turn": input.turn,
            "stop_reason": input.stop_reason,
            "agent_id": agent_id.to_string(),
            "query_source": format!("{:?}", input.query_source),
        });
        let input_str = serde_json::to_string(&input_json).unwrap_or_else(|_| "{}".to_owned());

        let default_timeout = 60_000u64;

        // Run TaskCompleted hooks for in-progress tasks.
        let task_completed_hooks: Vec<&HookDefinition> = self
            .hooks
            .iter()
            .filter(|h| h.event == "TaskCompleted")
            .collect();

        for hook in &task_completed_hooks {
            let timeout_ms = hook.timeout_ms.unwrap_or(default_timeout);
            let start = Instant::now();
            let exec_result = execute_hook_process(hook, &input_str, timeout_ms).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            result.hook_count += 1;
            result.hook_infos.push(HookInfo {
                command: hook.command.clone(),
                prompt_text: None,
                duration_ms: Some(duration_ms),
            });

            if let Some(ref err) = exec_result.blocking_error {
                result.blocking_errors.push(err.clone());
            }
            if exec_result.prevent_continuation {
                result.prevent_continuation = true;
            }
        }

        // Run TeammateIdle hooks.
        let idle_hooks: Vec<&HookDefinition> = self
            .hooks
            .iter()
            .filter(|h| h.event == "TeammateIdle")
            .collect();

        for hook in &idle_hooks {
            let timeout_ms = hook.timeout_ms.unwrap_or(default_timeout);
            let start = Instant::now();
            let exec_result = execute_hook_process(hook, &input_str, timeout_ms).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            result.hook_count += 1;
            result.hook_infos.push(HookInfo {
                command: hook.command.clone(),
                prompt_text: None,
                duration_ms: Some(duration_ms),
            });

            if let Some(ref err) = exec_result.blocking_error {
                result.blocking_errors.push(err.clone());
            }
            if exec_result.prevent_continuation {
                result.prevent_continuation = true;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Hook Process Execution
// ---------------------------------------------------------------------------

/// Execute a single hook as a child process.
///
/// Spawns the hook command, pipes `input_json` to stdin, captures stdout and
/// stderr, and enforces a timeout. Returns the execution result with any
/// errors or continuation directives extracted from the output.
pub async fn execute_hook_process(
    hook: &HookDefinition,
    input_json: &str,
    timeout_ms: u64,
) -> HookExecutionResult {
    if hook.command.is_empty() {
        return HookExecutionResult {
            message: None,
            blocking_error: None,
            prevent_continuation: false,
            stop_reason: None,
        };
    }

    let program = &hook.command[0];
    let args: Vec<&str> = hook.command.iter().skip(1).map(String::as_str).collect();

    let mut child = match Command::new(program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return HookExecutionResult {
                message: None,
                blocking_error: Some(format!(
                    "Failed to spawn hook '{}': {e}",
                    hook.command.join(" ")
                )),
                prevent_continuation: false,
                stop_reason: None,
            };
        }
    };

    // Write input to stdin.
    if let Some(mut stdin) = child.stdin.take() {
        // Use a spawned task so the write doesn't block the timeout.
        let input_owned = input_json.to_owned();
        let write_result = tokio::spawn(async move {
            stdin.write_all(input_owned.as_bytes()).await?;
            stdin.flush().await?;
            Ok::<(), std::io::Error>(())
        })
        .await;

        if let Err(e) = write_result {
            return HookExecutionResult {
                message: None,
                blocking_error: Some(format!(
                    "Hook stdin write failed for '{}': {e}",
                    hook.command.join(" ")
                )),
                prevent_continuation: false,
                stop_reason: None,
            };
        }
    }

    // Take stdout/stderr handles before waiting, so we can read them after.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // Wait for the process with timeout, using tokio::select! to properly
    // handle the race between completion and timeout.
    let timeout_duration = Duration::from_millis(timeout_ms);

    let status_result = tokio::select! {
        status = child.wait() => status,
        _ = tokio::time::sleep(timeout_duration) => {
            // Timeout elapsed — kill the child process.
            let _ = child.start_kill();
            return HookExecutionResult {
                message: None,
                blocking_error: Some(format!(
                    "Hook '{}' timed out after {timeout_ms}ms",
                    hook.command.join(" ")
                )),
                prevent_continuation: false,
                stop_reason: None,
            };
        }
    };

    let status = match status_result {
        Ok(s) => s,
        Err(e) => {
            return HookExecutionResult {
                message: None,
                blocking_error: Some(format!(
                    "Hook '{}' process error: {e}",
                    hook.command.join(" ")
                )),
                prevent_continuation: false,
                stop_reason: None,
            };
        }
    };

    // Read stdout and stderr from the captured handles.
    let stdout = match stdout_handle {
        Some(mut reader) => {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        }
        None => String::new(),
    };
    let stderr = match stderr_handle {
        Some(mut reader) => {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        }
        None => String::new(),
    };

    if !status.success() {
        let exit_code = status.code().unwrap_or(-1);
        let error_msg = if !stderr.is_empty() {
            stderr
        } else {
            format!("Exit code {exit_code}")
        };

        let (blocking, prevent, stop_reason) = parse_hook_output(&stdout);

        HookExecutionResult {
            message: Some(error_msg),
            blocking_error: blocking,
            prevent_continuation: prevent,
            stop_reason,
        }
    } else {
        let (blocking, prevent, stop_reason) = parse_hook_output(&stdout);

        HookExecutionResult {
            message: if stdout.trim().is_empty() && stderr.trim().is_empty() {
                None
            } else {
                Some(stdout)
            },
            blocking_error: blocking,
            prevent_continuation: prevent,
            stop_reason,
        }
    }
}

/// Parse hook stdout for blocking/prevent-continuation directives.
///
/// Hooks communicate by outputting structured JSON. We look for known keys:
/// - `{"blockingError": "..."}` — signals a blocking error
/// - `{"preventContinuation": true, "stopReason": "..."}` — prevent continuation
fn parse_hook_output(stdout: &str) -> (Option<String>, bool, Option<String>) {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return (None, false, None);
    }

    // Try to parse as JSON first.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let blocking = val
            .get("blockingError")
            .and_then(|v| v.as_str())
            .map(String::from);

        let prevent = val
            .get("preventContinuation")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let stop_reason = val
            .get("stopReason")
            .and_then(|v| v.as_str())
            .map(String::from);

        return (blocking, prevent, stop_reason);
    }

    // Not JSON — treat non-empty output as informational.
    (None, false, None)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use claude_core::{AgentId, ConversationEntry, Message, PermissionMode, SessionId};
    use tokio::sync::mpsc;
    use tokio::time::timeout as tokio_timeout;

    use crate::config::{ProcessUserInputContext, QuerySource};

    use super::{
        BackgroundFireAndForgetHandler, BackgroundTaskKind, ComputerUseCleanupHandler,
        ComputerUseCleanupReport, HookDefinition, JobClassificationHandler, ReplHookContext,
        SaveCacheSafeParamsHandler, StopHookBaseInput, StopHookManager, StopHookOutcome,
        StopHookPhase, StopHookPhaseHandler, StopHookPipeline, StopHookPipelineResult,
        StopHookRequest, StopHookResult, TeammateHooksHandler, UserConfiguredStopHooksHandler,
        execute_hook_process,
    };

    // ----- Existing tests (preserved) -----

    #[test]
    fn stop_hook_allows_immediate_stop() {
        let mut mgr = StopHookManager::new(3);
        mgr.request_stop("user requested");
        assert!(mgr.is_pending());
        let should_stop = mgr.evaluate(StopHookResult::Allow);
        assert!(should_stop);
        assert!(!mgr.is_pending());
    }

    #[test]
    fn stop_hook_retries_then_allows() {
        let mut mgr = StopHookManager::new(2);
        mgr.request_stop("budget");
        assert!(!mgr.evaluate(StopHookResult::Retry));
        assert!(!mgr.evaluate(StopHookResult::Retry));
        // After max retries, should force-stop
        assert!(mgr.evaluate(StopHookResult::Retry));
    }

    #[test]
    fn stop_hook_deny_cancels_stop() {
        let mut mgr = StopHookManager::new(3);
        mgr.request_stop("user");
        let should_stop = mgr.evaluate(StopHookResult::Deny);
        assert!(!should_stop);
        assert!(!mgr.is_pending());
    }

    #[test]
    fn stop_hook_cancel_clears_state() {
        let mut mgr = StopHookManager::new(3);
        mgr.request_stop("test");
        mgr.cancel();
        assert!(!mgr.is_pending());
        assert!(mgr.stop_reason().is_none());
    }

    #[test]
    fn stop_hook_retries_exhausted() {
        let mut mgr = StopHookManager::new(1);
        assert!(!mgr.retries_exhausted());
        mgr.request_stop("test");
        mgr.evaluate(StopHookResult::Retry);
        assert!(mgr.retries_exhausted());
    }

    #[test]
    fn stop_hook_reset_clears_all() {
        let mut mgr = StopHookManager::new(3);
        mgr.request_stop("test");
        mgr.evaluate(StopHookResult::Retry);
        mgr.reset();
        assert!(!mgr.is_pending());
        assert_eq!(mgr.retry_count(), 0);
    }

    #[test]
    fn stop_hook_default_is_3_retries() {
        let mgr = StopHookManager::default();
        assert_eq!(mgr.max_retries(), 3);
    }

    #[test]
    fn stop_hook_outcome_maps_to_retry_result() {
        assert_eq!(
            StopHookResult::from(&StopHookOutcome::Allow),
            StopHookResult::Allow
        );
        assert_eq!(
            StopHookResult::from(&StopHookOutcome::Retry {
                injected_messages: vec![Message::from(ConversationEntry::user("retry"))],
            }),
            StopHookResult::Retry
        );
        assert_eq!(
            StopHookResult::from(&StopHookOutcome::Deny),
            StopHookResult::Deny
        );
    }

    #[test]
    fn repl_hook_context_carries_prompt_and_context_maps() {
        let session_id = SessionId::new();
        let mut process =
            ProcessUserInputContext::new(session_id.clone(), PermissionMode::Default, "mock");
        process.system_prompt = Some("system".to_owned());
        process
            .user_context
            .insert("currentDate".to_owned(), "Today".to_owned());
        process
            .system_context
            .insert("gitStatus".to_owned(), "clean".to_owned());

        let context = ReplHookContext {
            session_id,
            turn: 2,
            messages: vec![Message::from(ConversationEntry::user("hello"))],
            query_source: process.query_source,
            agent_id: process.agent_id.clone(),
            system_prompt: process.system_prompt.clone(),
            user_context: process.user_context.clone(),
            system_context: process.system_context.clone(),
        };

        assert_eq!(context.turn, 2);
        assert_eq!(context.system_prompt.as_deref(), Some("system"));
        assert_eq!(
            context.user_context.get("currentDate").map(String::as_str),
            Some("Today")
        );
        assert_eq!(
            context.system_context.get("gitStatus").map(String::as_str),
            Some("clean")
        );
    }

    #[test]
    fn stop_hook_request_carries_terminal_metadata() {
        let request = StopHookRequest {
            stop_reason: "end_turn".to_owned(),
            final_text: Some("done".to_owned()),
        };

        assert_eq!(request.stop_reason, "end_turn");
        assert_eq!(request.final_text.as_deref(), Some("done"));
    }

    // ----- Helper to create a test base input -----

    fn test_input(query_source: QuerySource, agent_id: Option<AgentId>) -> StopHookBaseInput {
        StopHookBaseInput {
            session_id: SessionId::new(),
            turn: 1,
            stop_reason: "end_turn".to_owned(),
            final_text: None,
            agent_id,
            query_source,
            messages: vec![],
        }
    }

    fn echo_command(message: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".to_owned(), "/C".to_owned(), format!("echo {message}")]
        } else {
            vec!["echo".to_owned(), message.to_owned()]
        }
    }

    fn test_echo_hook(event: &str, message: &str) -> HookDefinition {
        HookDefinition {
            event: event.to_owned(),
            command: echo_command(message),
            timeout_ms: Some(5_000),
        }
    }

    // ----- Hook process execution tests -----

    #[tokio::test]
    async fn execute_hook_process_echo_succeeds() {
        let hook = test_echo_hook("Stop", "hello world");

        let result = execute_hook_process(&hook, "{}", 5_000).await;
        // echo should succeed (exit code 0).
        assert!(
            result.blocking_error.is_none(),
            "Expected no blocking error, got: {:?}",
            result.blocking_error
        );
        assert!(!result.prevent_continuation);
    }

    #[tokio::test]
    async fn execute_hook_process_empty_command_is_noop() {
        let hook = HookDefinition {
            event: "Stop".to_owned(),
            command: vec![],
            timeout_ms: None,
        };

        let result = execute_hook_process(&hook, "{}", 5_000).await;
        assert!(result.blocking_error.is_none());
        assert!(result.message.is_none());
        assert!(!result.prevent_continuation);
    }

    #[tokio::test]
    async fn execute_hook_process_nonexistent_command_fails() {
        let hook = HookDefinition {
            event: "Stop".to_owned(),
            command: vec!["__nonexistent_command_xyz_123__".to_owned()],
            timeout_ms: Some(5_000),
        };

        let result = execute_hook_process(&hook, "{}", 5_000).await;
        assert!(
            result.blocking_error.is_some(),
            "Expected blocking error for nonexistent command"
        );
        assert!(
            result
                .blocking_error
                .expect("nonexistent command should produce blocking error")
                .contains("Failed to spawn")
        );
    }

    #[tokio::test]
    async fn execute_hook_process_blocking_error_detection() {
        // On Windows, use `cmd /C exit 1` to produce a non-zero exit code.
        // On Unix, `false` returns exit code 1.
        let hook = if cfg!(windows) {
            HookDefinition {
                event: "Stop".to_owned(),
                command: vec!["cmd".to_owned(), "/C".to_owned(), "exit 1".to_owned()],
                timeout_ms: Some(5_000),
            }
        } else {
            HookDefinition {
                event: "Stop".to_owned(),
                command: vec!["false".to_owned()],
                timeout_ms: Some(5_000),
            }
        };

        let result = execute_hook_process(&hook, "{}", 5_000).await;
        assert!(
            result.message.is_some(),
            "Expected error message for non-zero exit code"
        );
    }

    #[tokio::test]
    async fn execute_hook_process_prevent_continuation_from_stdout() {
        // Use a command that outputs JSON with preventContinuation.
        // Write a temp script to avoid Windows echo quoting issues.
        let json_output = r#"{"preventContinuation":true,"stopReason":"hook says stop"}"#;

        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let script_path = if cfg!(windows) {
            let path = temp_dir.path().join("hook.bat");
            std::fs::write(&path, format!("@echo {json_output}"))
                .expect("Windows hook fixture should be written");
            path
        } else {
            let path = temp_dir.path().join("hook.sh");
            std::fs::write(&path, format!("#!/bin/sh\necho '{json_output}'"))
                .expect("Unix hook fixture should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("Unix hook fixture should be executable");
            }
            path
        };

        let hook = HookDefinition {
            event: "Stop".to_owned(),
            command: vec![
                script_path
                    .to_str()
                    .expect("script fixture path should be valid UTF-8")
                    .to_owned(),
            ],
            timeout_ms: Some(5_000),
        };

        let result = execute_hook_process(&hook, "{}", 5_000).await;
        assert!(
            result.prevent_continuation,
            "Expected prevent_continuation to be true, got message: {:?}",
            result.message
        );
        assert_eq!(result.stop_reason.as_deref(), Some("hook says stop"));
    }

    // ----- Phase handler tests -----

    #[tokio::test]
    async fn save_cache_only_for_main_thread_and_sdk() {
        let handler = SaveCacheSafeParamsHandler;

        // Should run for ReplMainThread.
        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());

        // Should run for Sdk.
        let input = test_input(QuerySource::Sdk, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());

        // Should skip for Agent (sub-agent).
        let input = test_input(QuerySource::Agent, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());

        // Should skip for User.
        let input = test_input(QuerySource::User, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
    }

    #[tokio::test]
    async fn background_fire_and_forget_skips_sub_agents() {
        let called = Arc::new(AtomicBool::new(false));
        let called_for_callback = called.clone();
        let handler = BackgroundFireAndForgetHandler::new(Arc::new(move |_input| {
            let called = called_for_callback.clone();
            Box::pin(async move {
                called.store(true, Ordering::Relaxed);
                Ok(())
            })
        }));

        // Should skip for sub-agent (has agent_id).
        let agent_id = AgentId::new(Some("test-agent"));
        let input = test_input(QuerySource::ReplMainThread, Some(agent_id));
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert!(!called.load(Ordering::Relaxed));

        // Should skip for User query source (bare mode).
        let input = test_input(QuerySource::User, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert!(!called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn background_fire_and_forget_fires_all_background_tasks_for_main() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handler = BackgroundFireAndForgetHandler::new(Arc::new(move |input| {
            let tx = tx.clone();
            Box::pin(async move {
                tx.send(input.task).expect("receiver should be open");
                Ok(())
            })
        }));

        // Should run for main thread (no agent_id, non-User source).
        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());

        let mut observed = BTreeSet::new();
        for _ in 0..3 {
            let task = tokio_timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("background task should fire")
                .expect("background task channel should stay open");
            observed.insert(task);
        }

        assert_eq!(
            observed,
            BTreeSet::from([
                BackgroundTaskKind::PromptSuggestion,
                BackgroundTaskKind::ExtractMemories,
                BackgroundTaskKind::AutoDream,
            ])
        );
    }

    #[tokio::test]
    async fn job_classification_without_job_dir_is_noop() {
        let handler = JobClassificationHandler::default();
        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();

        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert!(result.blocking_errors.is_empty());
        assert!(!result.prevent_continuation);
    }

    #[tokio::test]
    async fn computer_use_cleanup_runs_for_main_thread() {
        let called = Arc::new(AtomicBool::new(false));
        let called_for_cleanup = called.clone();
        let handler = ComputerUseCleanupHandler::new(Arc::new(move |_input| {
            let called = called_for_cleanup.clone();
            Box::pin(async move {
                called.store(true, Ordering::Relaxed);
                Ok(ComputerUseCleanupReport {
                    unhidden_apps: 2,
                    released_lock: true,
                })
            })
        }))
        .with_timeout(Duration::from_secs(1));

        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();

        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert!(called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn computer_use_cleanup_skips_sub_agents() {
        let called = Arc::new(AtomicBool::new(false));
        let called_for_cleanup = called.clone();
        let handler = ComputerUseCleanupHandler::new(Arc::new(move |_input| {
            let called = called_for_cleanup.clone();
            Box::pin(async move {
                called.store(true, Ordering::Relaxed);
                Ok(ComputerUseCleanupReport::default())
            })
        }));

        let input = test_input(QuerySource::Agent, Some(AgentId::new(Some("sub-agent"))));
        let mut result = StopHookPipelineResult::default();

        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert!(!called.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn user_configured_hooks_match_event_type() {
        let hooks = vec![test_echo_hook("Stop", "ok")];
        let handler = UserConfiguredStopHooksHandler::new(hooks);

        // Main agent → event is "Stop", should match.
        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert_eq!(result.hook_count, 1);
        assert_eq!(result.hook_infos.len(), 1);
        assert!(result.hook_infos[0].duration_ms.is_some());
        assert!(result.summary_message.is_some());
    }

    #[tokio::test]
    async fn user_configured_hooks_subagent_uses_subagentstop() {
        let hooks = vec![test_echo_hook("Stop", "ok")];
        let handler = UserConfiguredStopHooksHandler::new(hooks);

        // Sub-agent → event is "SubagentStop", should NOT match "Stop" hook.
        let agent_id = AgentId::new(Some("sub-agent"));
        let input = test_input(QuerySource::Agent, Some(agent_id));
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert_eq!(result.hook_count, 0);
    }

    #[tokio::test]
    async fn teammate_hooks_only_run_for_teammates() {
        let hooks = vec![
            test_echo_hook("TaskCompleted", "task-done"),
            test_echo_hook("TeammateIdle", "idle"),
        ];
        let handler = TeammateHooksHandler::new(hooks);

        // No agent_id → should be a no-op.
        let input = test_input(QuerySource::ReplMainThread, None);
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert_eq!(result.hook_count, 0);

        // With agent_id → should run both hooks.
        let agent_id = AgentId::new(Some("teammate-1"));
        let input = test_input(QuerySource::Agent, Some(agent_id));
        let mut result = StopHookPipelineResult::default();
        assert!(handler.execute(&input, &mut result).await.is_ok());
        assert_eq!(result.hook_count, 2);
    }

    // ----- Abort signal tests -----

    #[tokio::test]
    async fn pipeline_abort_skips_remaining_phases() {
        let signal = Arc::new(AtomicBool::new(false));
        let mut pipeline = StopHookPipeline::new().with_abort_signal(signal.clone());

        pipeline.register_phase(
            StopHookPhase::SaveCacheSafeParams,
            Box::new(SaveCacheSafeParamsHandler),
        );
        pipeline.register_phase(StopHookPhase::Return, Box::new(super::NoOpPhaseHandler));

        // Abort before execution.
        signal.store(true, Ordering::Relaxed);
        let input = test_input(QuerySource::ReplMainThread, None);
        let result = pipeline.execute(&input).await;

        assert!(result.prevent_continuation);
        assert!(result.phases_executed.is_empty());
    }

    #[tokio::test]
    async fn pipeline_abort_between_phases() {
        let signal = Arc::new(AtomicBool::new(false));

        struct AbortAfterFirstPhase {
            signal: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl super::StopHookPhaseHandler for AbortAfterFirstPhase {
            async fn execute(
                &self,
                _input: &super::StopHookBaseInput,
                _result: &mut super::StopHookPipelineResult,
            ) -> anyhow::Result<()> {
                self.signal.store(true, Ordering::Relaxed);
                Ok(())
            }
        }

        let mut pipeline = StopHookPipeline::new().with_abort_signal(signal.clone());

        pipeline.register_phase(
            StopHookPhase::SaveCacheSafeParams,
            Box::new(AbortAfterFirstPhase {
                signal: signal.clone(),
            }),
        );
        pipeline.register_phase(StopHookPhase::Return, Box::new(super::NoOpPhaseHandler));

        let input = test_input(QuerySource::ReplMainThread, None);
        let result = pipeline.execute(&input).await;

        // First phase executed, but then abort was detected before second phase.
        assert!(result.prevent_continuation);
        assert_eq!(result.phases_executed.len(), 1);
        assert_eq!(
            result.phases_executed[0],
            StopHookPhase::SaveCacheSafeParams
        );
    }

    // ----- default_with_handlers integration test -----

    #[tokio::test]
    async fn default_pipeline_with_handlers_executes_phases() {
        let stop_hooks = vec![test_echo_hook("Stop", "ok")];
        let teammate_hooks = vec![];

        let pipeline = StopHookPipeline::default_with_handlers(stop_hooks, teammate_hooks, None);

        let input = test_input(QuerySource::ReplMainThread, None);
        let result = pipeline.execute(&input).await;

        // Should execute all reference stop-hook phases in order.
        assert!(!result.phases_executed.is_empty());
        assert_eq!(
            result.phases_executed,
            vec![
                StopHookPhase::SaveCacheSafeParams,
                StopHookPhase::JobClassification,
                StopHookPhase::BackgroundFireAndForget,
                StopHookPhase::ComputerUseCleanup,
                StopHookPhase::UserConfiguredStopHooks,
                StopHookPhase::TeammateHooks,
                StopHookPhase::Return,
            ]
        );
        assert_eq!(result.hook_count, 1);
    }

    #[tokio::test]
    async fn default_pipeline_teammate_runs_teammate_hooks() {
        let stop_hooks = vec![test_echo_hook("SubagentStop", "sub-stop")];
        let teammate_hooks = vec![
            test_echo_hook("TaskCompleted", "task-done"),
            test_echo_hook("TeammateIdle", "idle"),
        ];

        let pipeline = StopHookPipeline::default_with_handlers(stop_hooks, teammate_hooks, None);

        let agent_id = AgentId::new(Some("teammate-agent"));
        let mut input = test_input(QuerySource::Agent, Some(agent_id));
        input.query_source = QuerySource::Agent;

        let result = pipeline.execute(&input).await;

        // Should have executed TeammateHooks with 2 hooks + SubagentStop with 1.
        assert!(
            result
                .phases_executed
                .contains(&StopHookPhase::TeammateHooks)
        );
        assert!(
            result
                .phases_executed
                .contains(&StopHookPhase::UserConfiguredStopHooks)
        );
        // 1 SubagentStop + 2 Teammate hooks = 3 total.
        assert_eq!(result.hook_count, 3);
    }

    // ----- HookDefinition serialization roundtrip -----

    #[test]
    fn hook_definition_serializes_correctly() {
        let hook = HookDefinition {
            event: "Stop".to_owned(),
            command: vec![
                "node".to_owned(),
                "script.js".to_owned(),
                "--arg".to_owned(),
            ],
            timeout_ms: Some(30_000),
        };

        let json = serde_json::to_string(&hook).expect("hook definition should serialize");
        assert!(json.contains("\"event\":\"Stop\""));
        assert!(json.contains("\"command\""));
        assert!(json.contains("\"timeout_ms\":30000"));

        let deserialized: HookDefinition =
            serde_json::from_str(&json).expect("hook definition should deserialize");
        assert_eq!(deserialized.event, "Stop");
        assert_eq!(deserialized.command, vec!["node", "script.js", "--arg"]);
        assert_eq!(deserialized.timeout_ms, Some(30_000));
    }

    #[test]
    fn hook_definition_optional_timeout_skipped_when_none() {
        let hook = HookDefinition {
            event: "SubagentStop".to_owned(),
            command: vec!["echo".to_owned()],
            timeout_ms: None,
        };

        let json = serde_json::to_string(&hook).expect("hook definition should serialize");
        assert!(!json.contains("timeout_ms"));
    }
}
