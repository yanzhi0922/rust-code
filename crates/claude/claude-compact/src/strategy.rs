//! Compact strategy trait and core types.
//!
//! Defines the [`CompactStrategy`] trait that all compaction strategies implement,
//! along with shared configuration ([`CompactOptions`]), progress events
//! ([`CompactProgressEvent`]), and the result type ([`CompactionResult`]).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use claude_core::Message;

// ---------------------------------------------------------------------------
// Strategy type enum
// ---------------------------------------------------------------------------

/// Identifies which compaction strategy was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompactStrategyType {
    /// Full conversation compaction — summarize everything, keep recent tail.
    Full,
    /// Partial compaction around a pivot message.
    Partial,
    /// Automatic compaction triggered when token usage exceeds a threshold.
    Auto,
    /// Micro compaction via cache editing (clear old tool results).
    Micro,
    /// Snip compaction — trim oversized tool outputs.
    Snip,
    /// Reactive compaction — triggered by API prompt-too-long errors.
    Reactive,
    /// Session-memory compaction — preserve key facts, compress the rest.
    SessionMemory,
}

impl fmt::Display for CompactStrategyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Partial => write!(f, "partial"),
            Self::Auto => write!(f, "auto"),
            Self::Micro => write!(f, "micro"),
            Self::Snip => write!(f, "snip"),
            Self::Reactive => write!(f, "reactive"),
            Self::SessionMemory => write!(f, "session_memory"),
        }
    }
}

// ---------------------------------------------------------------------------
// Preserved segment metadata
// ---------------------------------------------------------------------------

/// Describes a range of messages that survived compaction.
#[derive(Debug, Clone)]
pub struct PreservedSegment {
    /// UUID of the first preserved message.
    pub head_uuid: uuid::Uuid,
    /// UUID of the message immediately preceding the preserved range.
    pub anchor_uuid: uuid::Uuid,
    /// UUID of the last preserved message.
    pub tail_uuid: uuid::Uuid,
}

// ---------------------------------------------------------------------------
// Recompaction info (telemetry)
// ---------------------------------------------------------------------------

/// Diagnosis context passed from auto-compact into the compact engine.
///
/// Lets the `tengu_compact` telemetry event disambiguate same-chain loops
/// from cross-agent and manual-vs-auto compactions without joins.
///
/// Mirrors `RecompactionInfo` from the TypeScript reference
/// (`services/compact/compact.ts`).
#[derive(Debug, Clone, Default)]
pub struct RecompactionInfo {
    /// Whether this compaction happens in a chain where a previous compact
    /// already occurred.
    pub is_recompaction_in_chain: bool,
    /// How many turns elapsed since the last compaction.
    pub turns_since_previous_compact: i64,
    /// UUID of the turn where the previous compact happened.
    pub previous_compact_turn_id: Option<String>,
    /// Token threshold that triggers auto-compaction for the current model.
    pub auto_compact_threshold: u64,
    /// Origin of the query (e.g., "user", "api").
    pub query_source: Option<String>,
}

// ---------------------------------------------------------------------------
// Compaction result
// ---------------------------------------------------------------------------

/// Result returned by every compaction strategy.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The LLM-generated summary text (after formatting).
    pub summary: String,
    /// How many messages were removed / replaced by the summary.
    pub messages_removed: usize,
    /// Approximate number of tokens saved by the compaction.
    pub tokens_saved: u64,
    /// Which strategy produced this result.
    pub strategy_used: CompactStrategyType,
    /// Segments of the original conversation that were preserved verbatim.
    pub preserved_segments: Vec<PreservedSegment>,
    /// Token count before compaction.
    pub pre_compact_token_count: Option<u64>,
    /// Token count after compaction (estimated from the result payload).
    pub post_compact_token_count: Option<u64>,
    /// Messages to keep verbatim (for partial compaction).
    pub messages_to_keep: Vec<Message>,
    /// Attachment messages to re-inject after compaction.
    pub attachments: Vec<Message>,
    /// Hook result messages produced during compaction.
    pub hook_results: Vec<Message>,
    /// Optional user-facing display message (from hooks).
    pub user_display_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Compact options
// ---------------------------------------------------------------------------

/// Callback that provides SessionStart hook result messages after compaction.
///
/// Mirrors `processSessionStartHooks('compact', ...)` from the TS reference.
/// The callback receives the source (`"compact"`) and returns a list of
/// hook result messages to include in the [`CompactionResult`].
pub type SessionStartHookProvider =
    dyn Fn() -> Pin<Box<dyn Future<Output = Vec<Message>> + Send>> + Send + Sync;

/// Callback that provides post-compact attachment messages (deferred tools
/// delta, agent listing delta, MCP instructions delta).
///
/// Mirrors the TS pattern of calling `getDeferredToolsDeltaAttachment`,
/// `getAgentListingDeltaAttachment`, and `getMcpInstructionsDeltaAttachment`
/// with an empty message history (full compaction) or kept messages (partial
/// compaction) to produce re-injection attachments.
pub type PostCompactAttachmentProvider =
    dyn Fn() -> Pin<Box<dyn Future<Output = Vec<Message>> + Send>> + Send + Sync;

/// Result of executing PreCompact hooks.
///
/// Mirrors `executePreCompactHooks()` from the TS reference. Hooks can
/// inject additional custom instructions into the compact prompt and/or
/// produce a user-facing display message.
#[derive(Debug, Clone, Default)]
pub struct PreCompactHookResult {
    /// Merged custom instructions from hook outputs (joined by `\n\n`).
    pub new_custom_instructions: Option<String>,
    /// User-facing display message from hook execution.
    pub user_display_message: Option<String>,
}

/// Callback that executes PreCompact hooks before compaction.
///
/// Mirrors `executePreCompactHooks()` from the TS reference. The callback
/// receives the trigger (`"manual"` or `"auto"`) and any existing custom
/// instructions, and returns a [`PreCompactHookResult`].
pub type PreCompactHookProvider = dyn Fn(String, Option<String>) -> Pin<Box<dyn Future<Output = PreCompactHookResult> + Send>>
    + Send
    + Sync;

/// Result of executing PostCompact hooks after compaction.
#[derive(Debug, Clone, Default)]
pub struct PostCompactHookResult {
    /// User-facing display message from hook execution.
    pub user_display_message: Option<String>,
}

/// Callback that executes PostCompact hooks after compaction.
///
/// Mirrors `executePostCompactHooks()` from the TS reference. The callback
/// receives the trigger and the compact summary text.
pub type PostCompactHookProvider = dyn Fn(String, String) -> Pin<Box<dyn Future<Output = PostCompactHookResult> + Send>>
    + Send
    + Sync;

/// Callback that performs post-compact cache resets.
///
/// Mirrors `runPostCompactCleanup()` from the TS reference. Called after
/// compaction completes (both auto and manual) to reset caches:
/// system prompt sections, classifier approvals, speculative checks,
/// file state, session messages cache, etc.
pub type PostCompactCleanupProvider =
    dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

/// Callback for compact telemetry / analytics events.
///
/// Mirrors `logEvent('tengu_compact', ...)` from the TS reference. Called at
/// key points during compaction with an event name and structured metadata:
///
/// - `"tengu_compact"` — successful compaction
/// - `"tengu_compact_failed"` — compaction failed (`reason` in metadata)
/// - `"tengu_compact_ptl_retry"` — PTL retry attempted
/// - `"tengu_partial_compact"` — successful partial compaction
/// - `"tengu_partial_compact_failed"` — partial compaction failed
///
/// The callback is `Option`al; when `None`, no telemetry is emitted.
pub type CompactTelemetryProvider = dyn Fn(&str, serde_json::Value) + Send + Sync;

/// Configuration controlling compaction behaviour.
#[derive(Clone)]
pub struct CompactOptions {
    /// Maximum context-window size in tokens.
    pub max_tokens: u64,
    /// Target token count after compaction.
    pub target_tokens: u64,
    /// Number of recent messages to always preserve.
    pub preserve_recent_messages: usize,
    /// Whether to keep the system prompt intact.
    pub preserve_system_prompt: bool,
    /// Whether to preserve file attachments across compaction.
    pub preserve_attachments: bool,
    /// Optional custom instructions appended to the compact prompt.
    pub custom_instructions: Option<String>,
    /// Whether this is an auto-compact (vs manual /compact).
    pub is_auto_compact: bool,
    /// Optional provider for SessionStart hook messages re-fired after compact.
    ///
    /// When `Some`, the compact engine calls this after the summary is generated
    /// and includes the results in [`CompactionResult::hook_results`].
    /// Mirrors `processSessionStartHooks('compact', ...)` from the TS reference.
    pub session_start_hook_provider: Option<Arc<SessionStartHookProvider>>,
    /// Optional provider for post-compact attachment messages.
    ///
    /// When `Some`, the compact engine calls this after the summary is generated
    /// and includes the results in [`CompactionResult::attachments`].
    /// Mirrors the TS pattern of re-injecting deferred tools delta, agent listing
    /// delta, and MCP instructions delta after compaction.
    pub post_compact_attachment_provider: Option<Arc<PostCompactAttachmentProvider>>,
    /// Optional provider for PreCompact hook execution.
    ///
    /// When `Some`, fires BEFORE the compact LLM call. Hooks can inject
    /// additional custom instructions into the compact prompt.
    /// Mirrors `executePreCompactHooks()` from the TS reference.
    pub pre_compact_hook_provider: Option<Arc<PreCompactHookProvider>>,
    /// Optional provider for PostCompact hook execution.
    ///
    /// When `Some`, fires AFTER the compact summary is generated. Hooks
    /// receive the summary and can return a user-facing display message.
    /// Mirrors `executePostCompactHooks()` from the TS reference.
    pub post_compact_hook_provider: Option<Arc<PostCompactHookProvider>>,
    /// Optional provider for post-compact cache cleanup.
    ///
    /// When `Some`, called after compaction to reset caches (system prompt
    /// sections, classifier approvals, file state, etc.).
    /// Mirrors `runPostCompactCleanup()` from the TS reference.
    pub post_compact_cleanup_provider: Option<Arc<PostCompactCleanupProvider>>,
    /// Optional recompaction diagnostic context for telemetry.
    ///
    /// When `Some`, included in the `tengu_compact` telemetry event to
    /// disambiguate same-chain loops from cross-agent compactions.
    /// Mirrors `RecompactionInfo` from the TS reference.
    pub recompaction_info: Option<RecompactionInfo>,
    /// Optional telemetry callback for analytics events.
    ///
    /// When `Some`, called at key points during compaction (success, failure,
    /// PTL retry) with an event name and structured metadata.
    /// Mirrors `logEvent('tengu_compact', ...)` from the TS reference.
    pub telemetry_provider: Option<Arc<CompactTelemetryProvider>>,
    /// Optional compact lifecycle hooks.
    ///
    /// When `Some`, `pre_compact` is called before the compaction starts and
    /// `post_compact` is called after it completes. Hooks can observe or react
    /// to compaction events for logging, telemetry, or validation.
    pub compact_hooks: Option<Arc<dyn CompactHooks>>,
    /// Consecutive auto-compact failure count (circuit breaker).
    ///
    /// When this reaches [`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`](crate::auto::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES)
    /// (default 3), auto-compact is disabled for the rest of the session.
    /// Reset to 0 on any successful compaction.
    pub consecutive_compact_failures: u32,
    /// Whether auto-compact is disabled for the session due to the circuit breaker.
    ///
    /// Set to `true` when `consecutive_compact_failures` reaches the max.
    /// This flag is sticky — once set, auto-compact stays disabled.
    pub auto_compact_disabled: bool,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            max_tokens: 200_000,
            target_tokens: 50_000,
            preserve_recent_messages: 5,
            preserve_system_prompt: true,
            preserve_attachments: true,
            custom_instructions: None,
            is_auto_compact: false,
            session_start_hook_provider: None,
            post_compact_attachment_provider: None,
            pre_compact_hook_provider: None,
            post_compact_hook_provider: None,
            post_compact_cleanup_provider: None,
            recompaction_info: None,
            telemetry_provider: None,
            compact_hooks: None,
            consecutive_compact_failures: 0,
            auto_compact_disabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Progress event
// ---------------------------------------------------------------------------

/// Events emitted during a compaction run, used for UI progress reporting.
#[derive(Debug, Clone)]
pub enum CompactProgressEvent {
    /// Compaction has started with the given strategy.
    Started { strategy: CompactStrategyType },
    /// Progress update: N messages have been processed so far.
    Summarizing { messages_processed: usize },
    /// Compaction completed successfully.
    Completed(CompactionResult),
    /// Compaction failed with an error message.
    Failed(String),
}

/// Type-erased progress callback that is `Send + Sync`.
pub type ProgressCallback = dyn Fn(CompactProgressEvent) + Send + Sync;

// ---------------------------------------------------------------------------
// Summary request callback
// ---------------------------------------------------------------------------

/// Callback trait for generating a summary from a list of messages.
///
/// The compact engine does **not** depend on any specific LLM provider.
/// Instead, callers supply an implementation of [`SummaryProvider`] that
/// knows how to call the model and return the summary text.
#[async_trait::async_trait]
pub trait SummaryProvider: Send + Sync {
    /// Given `messages` to summarize and an optional `system_prompt`,
    /// return the raw summary text produced by the LLM.
    async fn generate_summary(
        &self,
        messages: &[Message],
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, anyhow::Error>;
}

/// A simple `Fn`-based [`SummaryProvider`] for convenience.
///
/// Uses `Pin<Box<dyn Future>>` to avoid complex lifetime bounds.
pub struct FnSummaryProvider<F>
where
    F: Fn(
            Vec<Message>,
            String,
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, anyhow::Error>> + Send>,
        > + Send
        + Sync,
{
    f: F,
}

impl<F> FnSummaryProvider<F>
where
    F: Fn(
            Vec<Message>,
            String,
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, anyhow::Error>> + Send>,
        > + Send
        + Sync,
{
    /// Create a new callback-based summary provider.
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

#[async_trait::async_trait]
impl<F> SummaryProvider for FnSummaryProvider<F>
where
    F: Fn(
            Vec<Message>,
            String,
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, anyhow::Error>> + Send>,
        > + Send
        + Sync,
{
    async fn generate_summary(
        &self,
        messages: &[Message],
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, anyhow::Error> {
        (self.f)(
            messages.to_vec(),
            system_prompt.to_string(),
            user_prompt.to_string(),
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Strategy trait
// ---------------------------------------------------------------------------

/// A compaction strategy reduces the token footprint of a conversation.
///
/// Each strategy decides *which* messages to keep, *how* to summarise the
/// rest, and returns a [`CompactionResult`] describing what happened.
#[async_trait::async_trait]
pub trait CompactStrategy: Send + Sync {
    /// Return the type identifier for this strategy.
    fn strategy_type(&self) -> CompactStrategyType;

    /// Execute the compaction.
    ///
    /// - `messages` — the full conversation so far.
    /// - `options`  — configuration controlling behaviour.
    /// - `provider` — callback used to generate the LLM summary.
    /// - `progress` — optional sink for progress events.
    async fn compact(
        &self,
        messages: &[Message],
        options: &CompactOptions,
        provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error>;
}

// ---------------------------------------------------------------------------
// Compact hooks trait
// ---------------------------------------------------------------------------

/// Hook points for the compaction lifecycle.
///
/// Implementations can observe or react to compaction events. The hooks are
/// called at well-defined points in the compaction flow:
///
/// 1. `pre_compact` — before the compaction starts
/// 2. `post_compact` — after the compaction completes
///
/// All methods have default no-op implementations so implementors only need
/// to override the hooks they care about.
///
/// # Example
///
/// ```ignore
/// use claude_compact::CompactHooks;
/// use serde_json::Value;
///
/// struct LoggingCompactHooks;
///
/// impl CompactHooks for LoggingCompactHooks {
///     fn pre_compact(&self, conversation: &[Value]) -> anyhow::Result<()> {
///         println!("About to compact {} messages", conversation.len());
///         Ok(())
///     }
///
///     fn post_compact(&self, conversation: &[Value], removed_count: usize) -> anyhow::Result<()> {
///         println!("Compacted: removed {} messages, {} remaining", removed_count, conversation.len());
///         Ok(())
///     }
/// }
/// ```
pub trait CompactHooks: Send + Sync {
    /// Called before compaction begins.
    ///
    /// The `conversation` parameter contains the full conversation that is
    /// about to be compacted. Implementations can use this for logging,
    /// telemetry, or pre-compaction validation.
    ///
    /// Returning an error will abort the compaction.
    fn pre_compact(&self, conversation: &[Message]) -> Result<(), anyhow::Error> {
        let _ = conversation;
        Ok(())
    }

    /// Called after compaction completes.
    ///
    /// The `conversation` parameter contains the post-compaction conversation
    /// (including the summary and any preserved messages). The `removed_count`
    /// indicates how many messages were removed during compaction.
    ///
    /// Returning an error will be logged but will not undo the compaction.
    fn post_compact(
        &self,
        conversation: &[Message],
        removed_count: usize,
    ) -> Result<(), anyhow::Error> {
        let _ = conversation;
        let _ = removed_count;
        Ok(())
    }
}
