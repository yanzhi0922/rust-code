//! Auto Compact strategy.
//!
//! Automatically triggers compaction when token usage exceeds a configurable
//! threshold.  Mirrors `services/compact/autoCompact.ts`.

use claude_core::Message;

use crate::engine::compact_conversation;
use crate::estimate_message_tokens;
use crate::strategy::{
    CompactOptions, CompactStrategy, CompactStrategyType, CompactionResult, ProgressCallback,
    RecompactionInfo, SummaryProvider,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Buffer tokens subtracted from the effective context window to determine
/// the auto-compact threshold.
pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;

/// Buffer for the warning threshold.
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;

/// Buffer for the error threshold.
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;

/// Buffer for the manual-compact blocking limit.
pub const MANUAL_COMPACT_BUFFER_TOKENS: u64 = 3_000;

/// Maximum consecutive auto-compact failures before circuit-breaker trips.
pub const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;

/// Reserve this many tokens for the compact summary output.
const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;

// ---------------------------------------------------------------------------
// Query source guards
// ---------------------------------------------------------------------------

/// Query sources that should **not** trigger auto-compact (would deadlock
/// because they run inside forked agents spawned by compaction itself).
const BLOCKED_QUERY_SOURCES: &[&str] = &["session_memory", "compact"];

/// Check whether a query source is allowed to trigger auto-compact.
///
/// Mirrors the recursion guards in `shouldAutoCompact()` from the TS
/// reference: `session_memory` and `compact` are forked agents that would
/// deadlock if they tried to auto-compact.
pub fn is_auto_compact_allowed_for_query_source(query_source: Option<&str>) -> bool {
    match query_source {
        Some(qs) => !BLOCKED_QUERY_SOURCES.contains(&qs),
        None => true,
    }
}

// ---------------------------------------------------------------------------
// Environment variable overrides
// ---------------------------------------------------------------------------

/// Check whether auto-compact is enabled.
///
/// Mirrors `isAutoCompactEnabled()` from the TS reference which checks:
/// 1. `DISABLE_COMPACT` env var (disables all compaction)
/// 2. `DISABLE_AUTO_COMPACT` env var (disables only auto-compact)
/// 3. User config `autoCompactEnabled` setting (defaults to `true`)
pub fn is_auto_compact_env_enabled(user_config_auto_compact: Option<bool>) -> bool {
    if std::env::var("DISABLE_COMPACT")
        .ok()
        .as_deref()
        .is_some_and(is_env_truthy)
    {
        return false;
    }
    if std::env::var("DISABLE_AUTO_COMPACT")
        .ok()
        .as_deref()
        .is_some_and(is_env_truthy)
    {
        return false;
    }
    // Check user config — default to true (auto-compact enabled) if not set
    user_config_auto_compact.unwrap_or(true)
}

/// Return the effective context window size, respecting the
/// `CLAUDE_CODE_AUTO_COMPACT_WINDOW` environment variable override.
///
/// Mirrors `getEffectiveContextWindowSize()` from the TS reference which uses
/// `Math.min(getMaxOutputTokensForModel(model), 20_000)` as the reserved amount.
pub fn get_effective_context_window(base_context_window: u64, max_output_tokens: u64) -> u64 {
    let reserved = max_output_tokens.min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    let mut context_window = base_context_window;

    if let Ok(val) = std::env::var("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
        && let Ok(parsed) = val.parse::<u64>()
        && parsed > 0
    {
        context_window = context_window.min(parsed);
    }

    context_window.saturating_sub(reserved)
}

/// Return the auto-compact threshold, respecting the
/// `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` environment variable.
///
/// Mirrors `getAutoCompactThreshold()` from the TS reference.
pub fn get_auto_compact_threshold(base_context_window: u64, max_output_tokens: u64) -> u64 {
    let effective = get_effective_context_window(base_context_window, max_output_tokens);
    let default_threshold = effective.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS);

    if let Ok(val) = std::env::var("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
        && let Ok(pct) = val.parse::<f64>()
        && pct > 0.0
        && pct <= 100.0
    {
        let pct_threshold = (effective as f64 * (pct / 100.0)) as u64;
        return pct_threshold.min(default_threshold);
    }

    default_threshold
}

/// Check whether a string value is truthy (mirrors TS `isEnvTruthy`).
fn is_env_truthy(val: &str) -> bool {
    matches!(val.to_lowercase().as_str(), "1" | "true" | "yes")
}

// ---------------------------------------------------------------------------
// Auto-compact tracking state
// ---------------------------------------------------------------------------

/// Tracks auto-compact state across turns.
#[derive(Debug, Clone)]
pub struct AutoCompactTrackingState {
    /// Whether compaction has occurred in this session.
    pub compacted: bool,
    /// Monotonically increasing turn counter.
    pub turn_counter: u64,
    /// Unique ID per turn.
    pub turn_id: String,
    /// Consecutive auto-compact failures (circuit breaker).
    pub consecutive_failures: u32,
}

impl Default for AutoCompactTrackingState {
    fn default() -> Self {
        Self {
            compacted: false,
            turn_counter: 0,
            turn_id: uuid::Uuid::new_v4().to_string(),
            consecutive_failures: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Token warning state
// ---------------------------------------------------------------------------

/// Result of checking token usage against thresholds.
#[derive(Debug, Clone)]
pub struct TokenWarningState {
    /// Percentage of context remaining.
    pub percent_left: u32,
    /// Token usage is above the warning threshold.
    pub is_above_warning_threshold: bool,
    /// Token usage is above the error threshold.
    pub is_above_error_threshold: bool,
    /// Token usage is above the auto-compact threshold.
    pub is_above_auto_compact_threshold: bool,
    /// Token usage is at the blocking limit.
    pub is_at_blocking_limit: bool,
}

// ---------------------------------------------------------------------------
// Auto compact strategy
// ---------------------------------------------------------------------------

/// Auto-compact strategy that triggers when token usage exceeds a threshold.
pub struct AutoCompactStrategy {
    /// Base context window size for the model (before effective computation).
    pub context_window_size: u64,
    /// Max output tokens for the model (used to compute reserved tokens).
    pub max_output_tokens: u64,
    /// User config override for auto-compact enabled (None = default true).
    pub auto_compact_enabled: Option<bool>,
}

impl AutoCompactStrategy {
    /// Create a new auto-compact strategy for the given context window size.
    pub fn new(context_window_size: u64) -> Self {
        Self {
            context_window_size,
            max_output_tokens: MAX_OUTPUT_TOKENS_FOR_SUMMARY,
            auto_compact_enabled: None,
        }
    }

    /// Create with explicit max output tokens.
    pub fn with_max_output_tokens(context_window_size: u64, max_output_tokens: u64) -> Self {
        Self {
            context_window_size,
            max_output_tokens,
            auto_compact_enabled: None,
        }
    }

    /// Set user config auto-compact enabled override.
    pub fn with_auto_compact_enabled(mut self, enabled: Option<bool>) -> Self {
        self.auto_compact_enabled = enabled;
        self
    }

    /// Return the effective context window size (minus reserved output tokens
    /// and respecting `CLAUDE_CODE_AUTO_COMPACT_WINDOW` env var).
    pub fn effective_context_window(&self) -> u64 {
        get_effective_context_window(self.context_window_size, self.max_output_tokens)
    }

    /// Return the auto-compact threshold (respecting env var overrides).
    pub fn auto_compact_threshold(&self) -> u64 {
        get_auto_compact_threshold(self.context_window_size, self.max_output_tokens)
    }

    /// Check if auto-compact should be triggered based on current token usage.
    pub fn should_auto_compact(
        &self,
        token_usage: u64,
        tracking: &AutoCompactTrackingState,
        query_source: Option<&str>,
    ) -> bool {
        self.should_auto_compact_with_snip(token_usage, 0, tracking, query_source)
    }

    /// Check if auto-compact should be triggered, accounting for tokens already
    /// freed by snip compaction.
    ///
    /// Mirrors `shouldAutoCompact()` from the TS reference, which subtracts
    /// `snipTokensFreed` from the token count before comparing against the
    /// threshold.
    pub fn should_auto_compact_with_snip(
        &self,
        token_usage: u64,
        snip_tokens_freed: u64,
        tracking: &AutoCompactTrackingState,
        query_source: Option<&str>,
    ) -> bool {
        // Circuit breaker: stop trying after too many consecutive failures
        if tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES {
            return false;
        }

        // Environment variable kill switches + user config
        if !is_auto_compact_env_enabled(self.auto_compact_enabled) {
            return false;
        }

        // Recursion guard: session_memory and compact are forked agents
        if !is_auto_compact_allowed_for_query_source(query_source) {
            return false;
        }

        let effective_usage = token_usage.saturating_sub(snip_tokens_freed);
        effective_usage >= self.auto_compact_threshold()
    }

    /// Calculate the token warning state for the given usage.
    pub fn calculate_token_warning_state(&self, token_usage: u64) -> TokenWarningState {
        let threshold = self.auto_compact_threshold();
        let percent_left = if threshold > 0 {
            std::cmp::max(
                0,
                ((threshold.saturating_sub(token_usage)) * 100 / threshold) as u32,
            )
        } else {
            0
        };

        let warning_threshold = threshold.saturating_sub(WARNING_THRESHOLD_BUFFER_TOKENS);
        let error_threshold = threshold.saturating_sub(ERROR_THRESHOLD_BUFFER_TOKENS);

        let default_blocking_limit = self
            .effective_context_window()
            .saturating_sub(MANUAL_COMPACT_BUFFER_TOKENS);

        // Allow override for testing (CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE)
        let blocking_limit = match std::env::var("CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE") {
            Ok(val) if val.parse::<u64>().is_ok_and(|v| v > 0) => {
                val.parse::<u64>().unwrap_or(default_blocking_limit)
            }
            _ => default_blocking_limit,
        };

        TokenWarningState {
            percent_left,
            is_above_warning_threshold: token_usage >= warning_threshold,
            is_above_error_threshold: token_usage >= error_threshold,
            is_above_auto_compact_threshold: token_usage >= threshold,
            is_at_blocking_limit: token_usage >= blocking_limit,
        }
    }
}

#[async_trait::async_trait]
impl CompactStrategy for AutoCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::Auto
    }

    async fn compact(
        &self,
        messages: &[Message],
        options: &CompactOptions,
        provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        // Delegate to the full compact engine with auto-compact flag set
        let auto_options = CompactOptions {
            is_auto_compact: true,
            ..options.clone()
        };

        let mut result = compact_conversation(messages, &auto_options, provider, progress).await?;
        result.strategy_used = CompactStrategyType::Auto;
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Standalone helper functions
// ---------------------------------------------------------------------------

/// Check whether auto-compact should trigger for the given messages.
///
/// This is a convenience wrapper that estimates token usage from the messages
/// and compares against the threshold.
pub fn should_auto_compact(
    messages: &[Message],
    context_window_size: u64,
    max_output_tokens: u64,
    tracking: &AutoCompactTrackingState,
    query_source: Option<&str>,
) -> bool {
    should_auto_compact_with_snip(
        messages,
        context_window_size,
        max_output_tokens,
        0,
        tracking,
        query_source,
    )
}

/// Like [`should_auto_compact`] but accounts for tokens already freed by snip.
pub fn should_auto_compact_with_snip(
    messages: &[Message],
    context_window_size: u64,
    max_output_tokens: u64,
    snip_tokens_freed: u64,
    tracking: &AutoCompactTrackingState,
    query_source: Option<&str>,
) -> bool {
    let strategy =
        AutoCompactStrategy::with_max_output_tokens(context_window_size, max_output_tokens);
    let token_usage = estimate_message_tokens(messages);
    strategy.should_auto_compact_with_snip(token_usage, snip_tokens_freed, tracking, query_source)
}

/// Execute auto-compact on the given messages.
///
/// Tries session-memory compaction first (no LLM call needed), falling back
/// to full LLM-based compaction if SM is unavailable or insufficient.
///
/// Mirrors `autoCompactIfNeeded()` from the TS reference. Returns `Ok(None)`
/// if auto-compact is not needed. Errors are absorbed silently (incrementing
/// the circuit breaker) rather than propagated — matching the TS behavior of
/// returning `{ wasCompacted: false }` on failure.
pub async fn auto_compact(
    messages: &[Message],
    context_window_size: u64,
    max_output_tokens: u64,
    options: &CompactOptions,
    provider: &dyn SummaryProvider,
    tracking: &mut AutoCompactTrackingState,
    query_source: Option<&str>,
    auto_compact_user_config: Option<bool>,
    snip_tokens_freed: u64,
) -> Result<Option<CompactionResult>, anyhow::Error> {
    if !is_auto_compact_env_enabled(auto_compact_user_config) {
        return Ok(None);
    }

    let strategy =
        AutoCompactStrategy::with_max_output_tokens(context_window_size, max_output_tokens)
            .with_auto_compact_enabled(auto_compact_user_config);
    let token_usage = estimate_message_tokens(messages);

    if !strategy.should_auto_compact_with_snip(
        token_usage,
        snip_tokens_freed,
        tracking,
        query_source,
    ) {
        return Ok(None);
    }

    // Build RecompactionInfo for telemetry (mirrors TS autoCompactIfNeeded)
    let recompaction_info = RecompactionInfo {
        is_recompaction_in_chain: tracking.compacted,
        turns_since_previous_compact: tracking.turn_counter as i64,
        previous_compact_turn_id: if tracking.compacted {
            Some(tracking.turn_id.clone())
        } else {
            None
        },
        auto_compact_threshold: strategy.auto_compact_threshold(),
        query_source: query_source.map(|s| s.to_owned()),
    };

    // Fire post-compact cleanup regardless of which path succeeds.
    let cleanup = options.post_compact_cleanup_provider.clone();

    // Try session-memory compaction first (mirrors TS: trySessionMemoryCompaction)
    let sm_strategy = crate::session_memory::SessionMemoryCompactStrategy::default();
    let sm_options = CompactOptions {
        max_tokens: strategy.auto_compact_threshold(),
        is_auto_compact: true,
        recompaction_info: Some(recompaction_info.clone()),
        ..options.clone()
    };
    if let Ok(sm_result) = sm_strategy
        .compact(messages, &sm_options, provider, None)
        .await
    {
        // Verify SM compact actually reduced below threshold
        if let Some(post) = sm_result.post_compact_token_count {
            if post <= strategy.auto_compact_threshold() {
                tracking.compacted = true;
                tracking.consecutive_failures = 0;
                if let Some(cleanup_fn) = cleanup {
                    cleanup_fn().await;
                }
                return Ok(Some(sm_result));
            }
        } else {
            tracking.compacted = true;
            tracking.consecutive_failures = 0;
            if let Some(cleanup_fn) = cleanup {
                cleanup_fn().await;
            }
            return Ok(Some(sm_result));
        }
    }

    // Fallback: full LLM-based compaction
    let full_options = CompactOptions {
        is_auto_compact: true,
        recompaction_info: Some(recompaction_info),
        ..options.clone()
    };
    match strategy
        .compact(messages, &full_options, provider, None)
        .await
    {
        Ok(result) => {
            tracking.compacted = true;
            tracking.consecutive_failures = 0;
            Ok(Some(result))
        }
        Err(_) => {
            // TS absorbs errors silently, only incrementing circuit breaker.
            // Do NOT propagate — return Ok(None) so the caller continues.
            tracking.consecutive_failures += 1;
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Session-memory fallback for auto-compact
// ---------------------------------------------------------------------------

/// Attempt session-memory compaction before falling back to full LLM-based
/// compaction.
///
/// Mirrors the `trySessionMemoryCompaction` call in `autoCompactIfNeeded()`
/// from the TS reference. Returns `None` if session-memory compaction is not
/// available or not applicable.
pub async fn try_session_memory_auto_compact(
    messages: &[Message],
    context_window_size: u64,
    max_output_tokens: u64,
    session_memory_strategy: &crate::session_memory::SessionMemoryCompactStrategy,
) -> Option<CompactionResult> {
    let effective = get_effective_context_window(context_window_size, max_output_tokens);
    let threshold = effective.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS);

    let options = CompactOptions {
        max_tokens: threshold,
        is_auto_compact: true,
        ..CompactOptions::default()
    };

    match session_memory_strategy
        .compact(
            messages,
            &options,
            &crate::strategy::FnSummaryProvider::new(|_, _, _| {
                Box::pin(async { Ok(String::new()) })
            }),
            None,
        )
        .await
    {
        Ok(result) => {
            // SM compact should reduce below the threshold
            if let Some(post) = result.post_compact_token_count {
                if post <= threshold {
                    return Some(result);
                }
            } else {
                return Some(result);
            }
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_compact_threshold_calculation() {
        let strategy = AutoCompactStrategy::new(200_000);
        let effective = strategy.effective_context_window();
        assert_eq!(effective, 200_000 - MAX_OUTPUT_TOKENS_FOR_SUMMARY);
        let threshold = strategy.auto_compact_threshold();
        assert_eq!(threshold, effective - AUTOCOMPACT_BUFFER_TOKENS);
    }

    #[test]
    fn should_auto_compact_below_threshold() {
        let strategy = AutoCompactStrategy::new(200_000);
        let tracking = AutoCompactTrackingState::default();
        assert!(!strategy.should_auto_compact(100_000, &tracking, None));
    }

    #[test]
    fn should_auto_compact_above_threshold() {
        let strategy = AutoCompactStrategy::new(200_000);
        let tracking = AutoCompactTrackingState::default();
        let threshold = strategy.auto_compact_threshold();
        assert!(strategy.should_auto_compact(threshold, &tracking, None));
    }

    #[test]
    fn circuit_breaker_trips() {
        let strategy = AutoCompactStrategy::new(200_000);
        let tracking = AutoCompactTrackingState {
            consecutive_failures: MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES,
            ..AutoCompactTrackingState::default()
        };
        assert!(!strategy.should_auto_compact(u64::MAX, &tracking, None));
    }

    #[test]
    fn query_source_guard_blocks_compact() {
        let strategy = AutoCompactStrategy::new(200_000);
        let tracking = AutoCompactTrackingState::default();
        assert!(!strategy.should_auto_compact(u64::MAX, &tracking, Some("compact")));
        assert!(!strategy.should_auto_compact(u64::MAX, &tracking, Some("session_memory")));
        assert!(strategy.should_auto_compact(u64::MAX, &tracking, Some("repl_main_thread")));
    }

    #[test]
    fn token_warning_state_calculation() {
        let strategy = AutoCompactStrategy::new(200_000);
        let state = strategy.calculate_token_warning_state(0);
        assert!(state.percent_left > 0);
        assert!(!state.is_above_warning_threshold);
        assert!(!state.is_at_blocking_limit);
    }
}
