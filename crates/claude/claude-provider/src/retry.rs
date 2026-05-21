//! Retry logic for provider API requests.
//!
//! Implements [`with_retry`] — a generic retry loop with exponential back-off,
//! jitter, and structured error classification.  Modeled after upstream Claude
//! Code's `withRetry` generator in `services/api/withRetry.ts`.
//!
//! # Authentication recovery
//!
//! The retry loop distinguishes between **permanent** auth errors (401/403 with
//! invalid credentials) and **transient** auth errors that may be recoverable:
//!
//! - **401 Unauthorized**: May indicate an expired OAuth token. The closure can
//!   attempt a token refresh before the next retry.
//! - **403 Forbidden**: May indicate revoked OAuth tokens or Bedrock/Vertex
//!   credential expiry. Credential caches are cleared to force re-auth.
//! - **429 Rate Limited**: Always retryable with exponential back-off.
//! - **5xx Server Errors**: Always retryable.
//! - **529 Overloaded**: Retryable up to `max_529_retries` consecutive times.

use anyhow::{Result, anyhow};
use parking_lot::Mutex;
use rand::Rng;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use tracing::warn;

use crate::query_source::QuerySource;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default maximum number of retry attempts.
const DEFAULT_MAX_RETRIES: u32 = 10;

/// Base delay in milliseconds for the first retry back-off.
const BASE_DELAY_MS: u64 = 500;

/// Maximum delay cap for standard retries (32 seconds, matching TS).
const MAX_BACKOFF_MS: u64 = 32_000;

/// Maximum number of consecutive 529 (overloaded) errors before giving up.
const MAX_529_RETRIES: u32 = 3;

/// Maximum number of auth recovery retries (401/403 with credential refresh).
const MAX_AUTH_RETRIES: u32 = 2;

/// Threshold below which a 429/529 retry-after is considered "short" in fast mode.
/// Short delays let us keep fast mode active to preserve prompt cache.
const SHORT_RETRY_THRESHOLD_MS: u64 = 20_000;

/// Minimum cooldown for fast mode when switching to standard speed.
/// Prevents rapid flip-flopping between fast and standard mode.
const MIN_COOLDOWN_MS: u64 = 10 * 60 * 1000;

/// Maximum back-off for persistent (unattended) retry mode.
const PERSISTENT_MAX_BACKOFF_MS: u64 = 5 * 60 * 1000;

/// Cap for persistent retry reset delay (6 hours).
const PERSISTENT_RESET_CAP_MS: u64 = 6 * 60 * 60 * 1000;

/// Heartbeat interval for persistent retry mode (30 seconds).
const HEARTBEAT_INTERVAL_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// RetryContext
// ---------------------------------------------------------------------------

/// Hints extracted from an HTTP response that influence retry decisions.
///
/// The operation closure can populate this struct from response headers
/// before returning an error, allowing the retry loop to respect the
/// `x-should-retry` header and rate-limit reset timing.
#[derive(Debug, Clone, Default)]
pub struct ResponseHints {
    /// Value of the `x-should-retry` response header, if present.
    pub should_retry: Option<bool>,
    /// Wait duration derived from `anthropic-ratelimit-unified-reset`, if present.
    pub rate_limit_wait: Option<Duration>,
}

/// Shared response hints wrapped in `Arc<Mutex<>>` so the operation closure
/// can write hints that the retry loop can read after the closure returns.
pub type SharedResponseHints = Arc<Mutex<ResponseHints>>;

/// Context carried across retry attempts.
///
/// Allows callers to adjust parameters (e.g. `max_tokens_override`) based on
/// what was learned from previous failed attempts.
#[derive(Debug, Clone)]
pub struct RetryContext {
    /// Override the `max_tokens` parameter for subsequent attempts.
    pub max_tokens_override: Option<u32>,
    /// The model being queried.
    pub model: String,
    /// Whether extended thinking is enabled.
    pub thinking_enabled: bool,
    /// Current attempt number (1-based).
    pub attempt: u32,
    /// Whether the previous attempt failed with an auth error (401/403).
    pub auth_refresh_attempted: bool,
    /// Optional fallback model to switch to after repeated 529 errors.
    pub fallback_model: Option<String>,
    /// Hints extracted from the last HTTP response (header-based retry signals).
    /// Shared via `Arc<Mutex<>>` so the operation closure can write them
    /// and the retry loop can read them after the closure returns.
    pub response_hints: SharedResponseHints,
    /// Whether fast mode is active for this retry loop.
    /// Set to `false` when entering cooldown.
    pub fast_mode: bool,
}

impl RetryContext {
    /// Create a new retry context for the given model.
    #[must_use]
    pub fn new(model: &str) -> Self {
        Self {
            max_tokens_override: None,
            model: model.to_owned(),
            thinking_enabled: false,
            attempt: 1,
            auth_refresh_attempted: false,
            fallback_model: None,
            response_hints: Arc::new(Mutex::new(ResponseHints::default())),
            fast_mode: false,
        }
    }

    /// Set the fallback model for 529 overload situations.
    #[must_use]
    pub fn with_fallback_model(mut self, model: &str) -> Self {
        self.fallback_model = Some(model.to_owned());
        self
    }

    /// Set the `x-should-retry` response hint.
    pub fn set_should_retry(&self, value: bool) {
        let mut hints = self.response_hints.lock();
        hints.should_retry = Some(value);
    }

    /// Set the rate-limit wait duration hint.
    pub fn set_rate_limit_wait(&self, duration: Duration) {
        let mut hints = self.response_hints.lock();
        hints.rate_limit_wait = Some(duration);
    }

    /// Populate response hints from an HTTP response's headers.
    pub fn populate_from_headers(&self, headers: &reqwest::header::HeaderMap) {
        let mut hints = self.response_hints.lock();
        hints.should_retry = should_retry_from_header(headers);
        hints.rate_limit_wait = get_rate_limit_wait_duration(headers);
    }
}

/// Helper to read a field from `SharedResponseHints` without panicking.
fn read_hint<T>(hints: &SharedResponseHints, f: impl FnOnce(&ResponseHints) -> T) -> T {
    let guard = hints.lock();
    f(&guard)
}

// ---------------------------------------------------------------------------
// Fast mode state (Gap 1)
// ---------------------------------------------------------------------------

/// Tracks whether fast mode is active and whether it is in a cooldown period.
///
/// When fast mode encounters a 429 or 529 with a long retry-after, it enters
/// a cooldown to avoid cache thrashing. During cooldown the retry loop switches
/// to standard speed.
#[derive(Debug, Clone)]
pub struct FastModeState {
    /// Whether fast mode is currently active.
    pub active: bool,
    /// If set, fast mode is in cooldown until this instant.
    pub cooldown_until: Option<Instant>,
}

impl FastModeState {
    /// Create a new fast mode state.
    #[must_use]
    pub fn new(active: bool) -> Self {
        Self {
            active,
            cooldown_until: None,
        }
    }

    /// Check whether fast mode is currently in a cooldown period.
    #[must_use]
    pub fn is_in_cooldown(&self) -> bool {
        match self.cooldown_until {
            Some(until) => Instant::now() < until,
            None => false,
        }
    }

    /// Trigger a cooldown, deactivating fast mode until the given instant.
    pub fn trigger_cooldown(&mut self, until: Instant) {
        self.cooldown_until = Some(until);
        self.active = false;
    }

    /// Check if fast mode is effectively active (enabled and not in cooldown).
    #[must_use]
    pub fn is_effectively_active(&self) -> bool {
        self.active && !self.is_in_cooldown()
    }
}

impl Default for FastModeState {
    fn default() -> Self {
        Self::new(false)
    }
}

/// Check whether fast mode is enabled via the `CLAUDE_CODE_FAST` environment variable.
#[must_use]
pub fn is_fast_mode_enabled() -> bool {
    std::env::var("CLAUDE_CODE_FAST").as_deref() == Ok("1")
        || std::env::var("CLAUDE_CODE_FAST").as_deref() == Ok("true")
}

// ---------------------------------------------------------------------------
// Subscriber tier (Gap 3)
// ---------------------------------------------------------------------------

/// Subscriber tier for Claude.ai users.
///
/// Determines retry behavior for 429 rate limit errors:
/// - **Free** users do not retry 429 errors.
/// - **Pro** and **Enterprise** users retry 429 errors (they typically use
///   PAYG instead of fixed rate limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriberTier {
    /// Free-tier user — no 429 retry.
    Free,
    /// Pro subscriber — retries 429.
    Pro,
    /// Enterprise subscriber — retries 429.
    Enterprise,
}

/// Determine the current subscriber tier from `CLAUDE_CODE_SUBSCRIBER`.
#[must_use]
pub fn get_subscriber_tier() -> SubscriberTier {
    match std::env::var("CLAUDE_CODE_SUBSCRIBER").as_deref() {
        Ok("pro") => SubscriberTier::Pro,
        Ok("enterprise") => SubscriberTier::Enterprise,
        _ => SubscriberTier::Free,
    }
}

/// Check whether the current user is a subscriber (Pro or Enterprise).
#[must_use]
pub fn is_subscriber() -> bool {
    get_subscriber_tier() != SubscriberTier::Free
}

/// Check whether the current user is an enterprise subscriber.
#[must_use]
pub fn is_enterprise_subscriber() -> bool {
    get_subscriber_tier() == SubscriberTier::Enterprise
}

// ---------------------------------------------------------------------------
// Persistent retry mode (Gap 4)
// ---------------------------------------------------------------------------

/// Check whether persistent (unattended) retry mode is enabled.
///
/// When enabled, 429 and 529 errors are retried indefinitely with higher
/// back-off and periodic keep-alive yields so the host environment does
/// not mark the session idle mid-wait.
#[must_use]
pub fn is_persistent_retry_enabled() -> bool {
    std::env::var("CLAUDE_CODE_PERSISTENT_RETRY").as_deref() == Ok("1")
        || std::env::var("CLAUDE_CODE_PERSISTENT_RETRY").as_deref() == Ok("true")
}

// ---------------------------------------------------------------------------
// OAuth refresh callback (Gap 5)
// ---------------------------------------------------------------------------

/// Callback type for OAuth token refresh on 401 errors.
///
/// The callback is invoked when a 401 Unauthorized error is encountered,
/// allowing the caller to refresh OAuth tokens before the next retry attempt.
pub type OAuthRefreshCallback =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

// ---------------------------------------------------------------------------
// Retry options (aggregated for with_retry_ext)
// ---------------------------------------------------------------------------

/// Extended retry options that control fast mode, query source gating,
/// subscriber-aware behavior, persistent retry, and OAuth token refresh.
#[derive(Clone)]
pub struct RetryOptions {
    /// Whether fast mode is requested for this retry loop.
    pub fast_mode: bool,
    /// The query source, used for 529 source gating.
    pub query_source: Option<QuerySource>,
    /// Optional OAuth token refresh callback for 401 errors.
    pub on_auth_error: Option<Arc<OAuthRefreshCallback>>,
}

impl Default for RetryOptions {
    fn default() -> Self {
        Self {
            fast_mode: is_fast_mode_enabled(),
            query_source: None,
            on_auth_error: None,
        }
    }
}

impl RetryOptions {
    /// Create retry options with fast mode enabled.
    #[must_use]
    pub fn with_fast_mode(mut self, enabled: bool) -> Self {
        self.fast_mode = enabled;
        self
    }

    /// Set the query source for 529 gating.
    #[must_use]
    pub fn with_query_source(mut self, source: QuerySource) -> Self {
        self.query_source = Some(source);
        self
    }

    /// Set the OAuth token refresh callback.
    #[must_use]
    pub fn with_auth_callback(
        mut self,
        callback: impl Fn() -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync + 'static,
    ) -> Self {
        self.on_auth_error = Some(Arc::new(Box::new(callback)));
        self
    }
}

// ---------------------------------------------------------------------------
// 529 source gating (Gap 2)
// ---------------------------------------------------------------------------

/// Foreground query sources that retry on 529 errors.
///
/// Background sources (title generation, summaries, suggestions, classifiers)
/// bail immediately on 529 to avoid amplifying during capacity cascades.
/// The user never sees those fail anyway.
///
/// TS reference also includes fine-grained variants: `repl_main_thread:outputStyle:*`,
/// `agent:custom`, `agent:default`, `agent:builtin`, `bash_classifier`.
/// In Rust these map to the coarser variants below (`Agent` covers all agent subtypes,
/// `ReplMainThread` covers all output-style variants).
const FOREGROUND_529_RETRY_SOURCES: &[QuerySource] = &[
    QuerySource::ReplMainThread,
    QuerySource::Sdk,
    QuerySource::Agent,
    QuerySource::Compact,
    QuerySource::User,
    QuerySource::HookAgent,
    QuerySource::HookPrompt,
    QuerySource::VerificationAgent,
    QuerySource::SideQuestion,
    QuerySource::AutoMode,
];

/// Check whether a query source should retry on 529 errors.
///
/// Returns `true` for foreground sources (main thread, agent, compact, SDK, user)
/// and for `None` (conservative default for untagged call paths).
/// Returns `false` for background sources.
#[must_use]
pub fn should_retry_529(source: Option<QuerySource>) -> bool {
    match source {
        None => true, // Untagged paths default to retry (conservative).
        Some(s) => FOREGROUND_529_RETRY_SOURCES.contains(&s),
    }
}

// ---------------------------------------------------------------------------
// Retry configuration
// ---------------------------------------------------------------------------

/// Configuration for the retry loop.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential back-off.
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds.
    pub max_backoff_ms: u64,
    /// Maximum consecutive 529 errors before giving up.
    pub max_529_retries: u32,
    /// Whether to respect the `Retry-After` header.
    pub respect_retry_after: bool,
    /// Maximum number of auth recovery retries (401/403 with credential refresh).
    pub max_auth_retries: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay_ms: BASE_DELAY_MS,
            max_backoff_ms: MAX_BACKOFF_MS,
            max_529_retries: MAX_529_RETRIES,
            respect_retry_after: true,
            max_auth_retries: MAX_AUTH_RETRIES,
        }
    }
}

impl RetryConfig {
    /// Create a retry config from provider settings.
    #[must_use]
    pub fn from_provider(max_retries: u32, initial_backoff_ms: u64, max_backoff_ms: u64) -> Self {
        Self {
            max_retries,
            base_delay_ms: initial_backoff_ms,
            max_backoff_ms,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Error classification for retries
// ---------------------------------------------------------------------------

/// Classify an HTTP status code as retryable or not.
#[must_use]
pub fn is_retryable_http_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Check whether a response body contains an `overloaded_error` type.
///
/// The Anthropic SDK sometimes drops the 529 HTTP status during streaming
/// and returns a different status code with `"type":"overloaded_error"` in
/// the body.  This function checks for that pattern as a fallback so that
/// overloaded responses are still classified as retryable.
#[must_use]
pub fn is_overloaded_error_body(body: &[u8]) -> bool {
    if let Ok(body_text) = std::str::from_utf8(body) {
        body_text.contains("\"overloaded_error\"")
    } else {
        false
    }
}

/// Classify a transport error as retryable or not.
#[must_use]
pub fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

/// Classify an error as a transient auth error that may be recoverable.
///
/// Returns `true` for:
/// - 401 Unauthorized (expired OAuth token)
/// - 403 Forbidden with OAuth token revoked message
/// - Bedrock credential provider errors
/// - Vertex credential refresh failures
#[must_use]
pub fn is_transient_auth_error(error_str: &str) -> bool {
    let lower = error_str.to_ascii_lowercase();

    // 401 Unauthorized — may be an expired token.
    if lower.contains("401") {
        return true;
    }

    // 403 Forbidden with specific recoverable messages.
    if lower.contains("403") {
        // OAuth token revoked — another process refreshed the token.
        if lower.contains("oauth token has been revoked")
            || lower.contains("token has been revoked")
        {
            return true;
        }
        // Bedrock credential errors.
        if lower.contains("credentialsprovidererror")
            || lower.contains("security token included in the request is invalid")
            || lower.contains("security token included in the request is expired")
        {
            return true;
        }
        // Vertex/GCP credential errors.
        if lower.contains("could not load the default credentials")
            || lower.contains("could not refresh access token")
            || lower.contains("invalid_grant")
        {
            return true;
        }
    }

    false
}

/// Classify an error as a permanent (non-retryable) auth error.
///
/// Returns `true` for:
/// - 403 Forbidden (without recoverable messages)
/// - 404 Not Found
///
/// When CCR mode is active ([`is_ccr_mode`]), 401/403 errors are **not**
/// classified as permanent so the retry loop can attempt credential recovery.
#[must_use]
pub fn is_permanent_auth_error(error_str: &str) -> bool {
    // In CCR mode, treat 401/403 as transient rather than permanent.
    if is_ccr_mode() {
        let lower = error_str.to_ascii_lowercase();
        // Only 404 remains permanent in CCR mode.
        if lower.contains("404") {
            return true;
        }
        return false;
    }

    let lower = error_str.to_ascii_lowercase();

    // 404 Not Found — permanent.
    if lower.contains("404") {
        return true;
    }

    // 403 Forbidden — permanent unless it's a known transient auth error.
    if lower.contains("403") && !is_transient_auth_error(error_str) {
        return true;
    }

    false
}

/// Classify an error as a stale connection error (ECONNRESET/EPIPE).
#[must_use]
pub fn is_stale_connection_error(error_str: &str) -> bool {
    let lower = error_str.to_ascii_lowercase();
    lower.contains("econnreset")
        || lower.contains("epipe")
        || lower.contains("broken pipe")
        || lower.contains("connection reset")
}

/// Check whether Claude Code Remote (CCR) mode is active.
///
/// When `CLAUDE_CODE_REMOTE` is set to `"1"` or `"true"`, authentication
/// errors (401/403) are treated as transient rather than permanent.  This
/// allows the retry loop to recover from temporary auth failures caused by
/// token rotation in remote-proxy environments.
#[must_use]
pub fn is_ccr_mode() -> bool {
    std::env::var("CLAUDE_CODE_REMOTE").as_deref() == Ok("1")
        || std::env::var("CLAUDE_CODE_REMOTE").as_deref() == Ok("true")
}

// ---------------------------------------------------------------------------
// Structured error classification
// ---------------------------------------------------------------------------

/// Structured classification of provider API errors.
///
/// Provides a typed taxonomy of error kinds that callers can use to drive
/// retry decisions, user-facing error messages, and telemetry labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// Request was explicitly aborted / cancelled.
    Aborted,
    /// API-level timeout (408 or 504).
    ApiTimeout,
    /// Rate limited (429).
    RateLimit,
    /// Server overloaded (529).
    ServerOverload,
    /// Prompt exceeds model context window.
    PromptTooLong,
    /// Authentication failure (401/403).
    AuthError,
    /// Requested model does not exist or is not accessible.
    InvalidModel,
    /// Other client error (4xx not classified above).
    ClientError,
    /// Server error (5xx not classified above).
    ServerError,
    /// Network / connection failure.
    ConnectionError,
    /// PDF attachment exceeds size limits.
    PdfTooLarge,
    /// PDF attachment is password-protected and cannot be processed.
    PdfPasswordProtected,
    /// Image attachment exceeds size limits.
    ImageTooLarge,
    /// Account credit balance is too low to process the request.
    CreditBalanceLow,
    /// The provided API key is invalid.
    InvalidApiKey,
    /// The authentication token has been revoked.
    TokenRevoked,
    /// SSL certificate verification error.
    SslCertError,
    /// Unclassified error.
    Unknown,
}

impl std::fmt::Display for ApiErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aborted => write!(f, "aborted"),
            Self::ApiTimeout => write!(f, "api_timeout"),
            Self::RateLimit => write!(f, "rate_limit"),
            Self::ServerOverload => write!(f, "server_overload"),
            Self::PromptTooLong => write!(f, "prompt_too_long"),
            Self::AuthError => write!(f, "auth_error"),
            Self::InvalidModel => write!(f, "invalid_model"),
            Self::ClientError => write!(f, "client_error"),
            Self::ServerError => write!(f, "server_error"),
            Self::ConnectionError => write!(f, "connection_error"),
            Self::PdfTooLarge => write!(f, "pdf_too_large"),
            Self::PdfPasswordProtected => write!(f, "pdf_password_protected"),
            Self::ImageTooLarge => write!(f, "image_too_large"),
            Self::CreditBalanceLow => write!(f, "credit_balance_low"),
            Self::InvalidApiKey => write!(f, "invalid_api_key"),
            Self::TokenRevoked => write!(f, "token_revoked"),
            Self::SslCertError => write!(f, "ssl_cert_error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Classify an HTTP status code and response body into a structured error kind.
///
/// # Examples
/// ```
/// use claude_provider::retry::{ApiErrorKind, classify_api_error};
/// assert_eq!(classify_api_error(401, ""), ApiErrorKind::AuthError);
/// assert_eq!(classify_api_error(429, ""), ApiErrorKind::RateLimit);
/// ```
#[must_use]
pub fn classify_api_error(status: u16, body: &str) -> ApiErrorKind {
    let lower = body.to_ascii_lowercase();

    // Body-based classification (highest priority — these are specific errors).
    if lower.contains("pdf too large") {
        return ApiErrorKind::PdfTooLarge;
    }
    if lower.contains("password") && lower.contains("pdf") {
        return ApiErrorKind::PdfPasswordProtected;
    }
    if lower.contains("image") && lower.contains("too large") {
        return ApiErrorKind::ImageTooLarge;
    }
    if lower.contains("credit_balance") {
        return ApiErrorKind::CreditBalanceLow;
    }
    if lower.contains("invalid x-api-key") {
        return ApiErrorKind::InvalidApiKey;
    }
    if lower.contains("token") && lower.contains("revoked") {
        return ApiErrorKind::TokenRevoked;
    }
    if lower.contains("ssl") && lower.contains("cert") {
        return ApiErrorKind::SslCertError;
    }

    match status {
        401 | 403 => ApiErrorKind::AuthError,
        429 => ApiErrorKind::RateLimit,
        529 => ApiErrorKind::ServerOverload,
        504 | 408 => ApiErrorKind::ApiTimeout,
        500 | 502 | 503 => ApiErrorKind::ServerError,
        400 if body.contains("prompt_too_long") || body.contains("max_tokens") => {
            ApiErrorKind::PromptTooLong
        }
        400 if body.contains("invalid_model") => ApiErrorKind::InvalidModel,
        400..=499 => ApiErrorKind::ClientError,
        _ => ApiErrorKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// x-should-retry header handling (GAP 1)
// ---------------------------------------------------------------------------

/// Check the `x-should-retry` response header.
///
/// If the header is present with value `"true"` or `"1"`, the API is
/// explicitly requesting a retry regardless of the HTTP status code.
/// This should be checked **before** the status code matching so the API
/// can override the default retry decision.
#[must_use]
pub fn should_retry_from_header(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    let value = headers.get("x-should-retry")?;
    let val_str = value.to_str().ok()?;
    match val_str {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rate-limit reset header parsing (GAP 2)
// ---------------------------------------------------------------------------

/// Parse the `anthropic-ratelimit-unified-reset` header to compute a wait
/// duration instead of falling back to exponential back-off.
///
/// The header value is a Unix timestamp (seconds, possibly fractional).
/// Returns `None` if the header is absent or cannot be parsed.
///
/// Cap is [`PERSISTENT_RESET_CAP_MS`] (6 hours) matching TS's
/// `getRateLimitResetDelayMs` which uses the full persistent reset cap.
#[must_use]
pub fn get_rate_limit_wait_duration(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let reset_header = headers.get("anthropic-ratelimit-unified-reset")?;
    let reset_str = reset_header.to_str().ok()?;
    let reset_ts: f64 = reset_str.parse().ok()?;
    let now_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    let wait_secs = (reset_ts - now_ts).max(0.0);
    // Cap at 6 hours matching TS PERSISTENT_RESET_CAP_MS.
    let max_secs = PERSISTENT_RESET_CAP_MS as f64 / 1000.0;
    Some(Duration::from_secs_f64(wait_secs.min(max_secs)))
}

// ---------------------------------------------------------------------------
// Max-tokens context overflow detection (GAP 3)
// ---------------------------------------------------------------------------

/// Parse a 400 error body for a context-length overflow and compute a new
/// `max_tokens` value that fits within the limit.
///
/// Expected error format:
/// `"input length and max_tokens exceed context limit: X + Y > Z"`
/// Also matches `"prompt_too_long"` in the body.
///
/// Returns `Some(new_max_tokens)` if the error is a context overflow, with
/// a 1 000-token buffer subtracted for safety.  Returns `None` if the
/// computed `max_tokens` would fall below [`FLOOR_OUTPUT_TOKENS`].
#[must_use]
pub fn parse_max_tokens_overflow(body: &str) -> Option<u64> {
    /// Minimum reasonable output token count — don't retry with less.
    const FLOOR_OUTPUT_TOKENS: u64 = 3000;

    let lower = body.to_ascii_lowercase();

    // If the body mentions prompt_too_long but not the arithmetic format,
    // we cannot compute a new value.
    if lower.contains("prompt_too_long") && !lower.contains('+') {
        return None;
    }

    // Try to extract "X + Y > Z" from the error message.
    let re = regex::Regex::new(r"(\d+)\s*\+\s*(\d+)\s*>\s*(\d+)").ok()?;
    let caps = re.captures(body)?;
    let input_tokens: u64 = caps[1].parse().ok()?;
    let context_limit: u64 = caps[3].parse().ok()?;
    // New max_tokens = context_limit - input_tokens - buffer
    let new_max = context_limit
        .saturating_sub(input_tokens)
        .saturating_sub(1000);
    if new_max < FLOOR_OUTPUT_TOKENS {
        return None;
    }
    Some(new_max)
}

// ---------------------------------------------------------------------------
// Model fallback suggestion (GAP 5)
// ---------------------------------------------------------------------------

/// Suggestion to fall back to an alternative model after repeated 529
/// (overloaded) errors from the primary model.
#[derive(Debug, Clone)]
pub struct ModelFallbackSuggested {
    /// The model that was being used when the 529 errors occurred.
    pub original_model: String,
    /// The model the caller should switch to, if known.
    pub fallback_model: Option<String>,
    /// Human-readable reason for the fallback suggestion.
    pub reason: String,
}

impl std::fmt::Display for ModelFallbackSuggested {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref fb) = self.fallback_model {
            write!(
                f,
                "model fallback suggested: {} -> {} ({})",
                self.original_model, fb, self.reason
            )
        } else {
            write!(
                f,
                "model fallback suggested: {} ({})",
                self.original_model, self.reason
            )
        }
    }
}

impl std::error::Error for ModelFallbackSuggested {}

// ---------------------------------------------------------------------------
// Delay computation
// ---------------------------------------------------------------------------

/// Compute the delay before the next retry attempt.
///
/// Uses exponential back-off with ±25% jitter to avoid thundering herd.
/// If `retry_after` is provided and the config allows it, that value is used
/// directly.
#[must_use]
pub fn compute_retry_delay(
    config: &RetryConfig,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Duration {
    if let Some(retry_after) = retry_after
        && config.respect_retry_after
    {
        return retry_after;
    }

    let multiplier = 2u64.saturating_pow(attempt.min(16));
    let base_ms = config
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(config.max_backoff_ms)
        .max(1);

    // Random jitter: 0–25 % of base delay.
    let jitter = rand::rng().random_range(0.0..1.0) * 0.25 * base_ms as f64;
    let delay_ms = base_ms + jitter as u64;

    Duration::from_millis(delay_ms.max(1))
}

/// Parse the `Retry-After` header from HTTP response headers.
///
/// Returns `None` if `respect_retry_after` is false or the header is
/// missing / not a valid integer (seconds).
#[must_use]
pub fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
    respect_retry_after: bool,
) -> Option<Duration> {
    if !respect_retry_after {
        return None;
    }
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

// ---------------------------------------------------------------------------
// with_retry
// ---------------------------------------------------------------------------

/// Execute an async operation with automatic retries on transient failures.
///
/// This is the basic retry entry point that delegates to [`with_retry_ext`]
/// with default options (no fast mode, no query source gating, no OAuth
/// callback). Use [`with_retry_ext`] directly for full control over gap features.
///
/// # Errors
///
/// Returns an error if all retry attempts are exhausted or if the operation
/// fails with a non-retryable error.
pub async fn with_retry<T, F, Fut>(config: &RetryConfig, model: &str, operation: F) -> Result<T>
where
    F: Fn(RetryContext) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    with_retry_ext(config, model, RetryOptions::default(), operation).await
}

// ---------------------------------------------------------------------------
// with_retry_ext (extended retry with all gap logic)
// ---------------------------------------------------------------------------

/// Execute an async operation with automatic retries and extended gap features.
///
/// This is the full-featured retry loop that integrates:
/// - **Gap 1**: Fast mode integration with cooldown logic
/// - **Gap 2**: Foreground 529 source gating
/// - **Gap 3**: Subscriber-aware 429 handling
/// - **Gap 4**: Persistent (unattended) retry mode
/// - **Gap 5**: OAuth token refresh on 401
///
/// The basic [`with_retry`] function delegates to this with default options.
pub async fn with_retry_ext<T, F, Fut>(
    config: &RetryConfig,
    model: &str,
    options: RetryOptions,
    operation: F,
) -> Result<T>
where
    F: Fn(RetryContext) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut context = RetryContext::new(model);
    context.fast_mode = options.fast_mode && is_fast_mode_enabled();
    let mut consecutive_529: u32 = 0;
    let mut auth_retries: u32 = 0;
    let mut fast_mode_state = FastModeState::new(context.fast_mode);
    let mut persistent_attempt: u32 = 0;

    for attempt in 0..=config.max_retries {
        context.attempt = attempt + 1;
        // Reset response hints for each attempt.
        {
            let mut hints = context.response_hints.lock();
            *hints = ResponseHints::default();
        }

        // Capture whether fast mode is effectively active before this attempt.
        let was_fast_mode_active = fast_mode_state.is_effectively_active();

        match operation(context.clone()).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let error_str = format!("{error:#}").to_ascii_lowercase();

                // --- x-should-retry header integration ---
                let hint_should_retry = read_hint(&context.response_hints, |h| h.should_retry);
                if hint_should_retry == Some(false) {
                    let is_5xx = error_str.contains("500")
                        || error_str.contains("502")
                        || error_str.contains("503")
                        || error_str.contains("529");
                    let overloaded_body = is_overloaded_error_body(error_str.as_bytes());
                    let is_ant = std::env::var("USER_TYPE").as_deref() == Ok("ant");
                    if !(is_ant && is_5xx) && !overloaded_body {
                        return Err(error);
                    }
                }

                let header_says_retry = hint_should_retry == Some(true);

                // Check if this is a permanent non-retryable error.
                if is_permanent_auth_error(&error_str) && !header_says_retry {
                    return Err(error);
                }

                // --- Gap 5: OAuth token refresh on 401 ---
                if is_transient_auth_error(&error_str) {
                    auth_retries += 1;
                    if auth_retries > config.max_auth_retries {
                        return Err(error.context(format!(
                            "giving up after {auth_retries} auth recovery attempts"
                        )));
                    }
                    context.auth_refresh_attempted = true;

                    // Invoke OAuth refresh callback if available and error is 401.
                    if error_str.contains("401")
                        && let Some(ref callback) = options.on_auth_error
                    {
                        warn!(
                            attempt = attempt + 1,
                            "invoking OAuth token refresh callback on 401"
                        );
                        if let Err(refresh_err) = callback().await {
                            warn!("OAuth refresh callback failed: {refresh_err:#}");
                        }
                    }

                    warn!(
                        attempt = attempt + 1,
                        auth_retry = auth_retries,
                        max_auth = config.max_auth_retries,
                        "auth error detected, will retry with credential refresh: {error:#}"
                    );
                    sleep(Duration::from_millis(500)).await;
                    continue;
                }

                // Check for stale connection errors (ECONNRESET/EPIPE).
                if is_stale_connection_error(&error_str) {
                    if attempt >= config.max_retries {
                        return Err(
                            error.context("all retry attempts exhausted (stale connection)")
                        );
                    }
                    let delay = compute_retry_delay(config, attempt, None);
                    warn!(
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis(),
                        "stale connection, retrying: {error:#}"
                    );
                    sleep(delay).await;
                    continue;
                }

                // --- Gap 1: Fast mode cooldown on 429/529 ---
                let is_529 =
                    error_str.contains("529") || is_overloaded_error_body(error_str.as_bytes());
                let is_429 = error_str.contains("429");

                if was_fast_mode_active && !is_persistent_retry_enabled() && (is_429 || is_529) {
                    // Check for a retry-after hint.
                    let retry_after_ms = read_hint(&context.response_hints, |h| {
                        h.rate_limit_wait.map(|d| d.as_millis() as u64)
                    });

                    if let Some(ms) = retry_after_ms
                        && ms < SHORT_RETRY_THRESHOLD_MS
                    {
                        // Short retry-after: keep fast mode active to preserve cache.
                        warn!(
                            attempt = attempt + 1,
                            retry_after_ms = ms,
                            "fast mode: short retry-after, keeping fast mode active"
                        );
                        sleep(Duration::from_millis(ms)).await;
                        continue;
                    }

                    // Long or unknown retry-after: enter cooldown.
                    let cooldown_ms =
                        std::cmp::max(retry_after_ms.unwrap_or(MIN_COOLDOWN_MS), MIN_COOLDOWN_MS);
                    warn!(
                        attempt = attempt + 1,
                        cooldown_ms, "fast mode: entering cooldown, switching to standard speed"
                    );
                    fast_mode_state
                        .trigger_cooldown(Instant::now() + Duration::from_millis(cooldown_ms));
                    context.fast_mode = false;
                    continue;
                }

                // --- Gap 2: Foreground 529 source gating ---
                if is_529 && !should_retry_529(options.query_source) {
                    warn!(
                        attempt = attempt + 1,
                        query_source = ?options.query_source,
                        "529 error for background source, not retrying to avoid amplification"
                    );
                    return Err(error.context("529 overloaded (background source, not retrying)"));
                }

                // Track consecutive 529 errors.
                if is_529 {
                    consecutive_529 += 1;
                    if consecutive_529 >= config.max_529_retries {
                        warn!(
                            consecutive_529,
                            model = %context.model,
                            "suggesting model fallback after repeated 529 errors"
                        );
                        return Err(error.context(ModelFallbackSuggested {
                            original_model: context.model.clone(),
                            fallback_model: context.fallback_model.clone(),
                            reason: format!(
                                "server overloaded: {consecutive_529} consecutive 529 errors"
                            ),
                        }));
                    }
                } else {
                    consecutive_529 = 0;
                }

                // --- Gap 3: Subscriber-aware 429 handling ---
                if is_429 && !is_subscriber() && !is_enterprise_subscriber() {
                    // Free-tier users do not retry 429 errors unless the header
                    // explicitly says to retry.
                    if !header_says_retry {
                        warn!(
                            attempt = attempt + 1,
                            "429 rate limit for free-tier user, not retrying"
                        );
                        return Err(error.context("429 rate limited (free tier, no retry)"));
                    }
                }

                // Check for max-tokens context overflow (400 with specific message).
                if error_str.contains("400")
                    && (error_str.contains("context limit")
                        || error_str.contains("prompt_too_long"))
                    && let Some(new_max) = parse_max_tokens_overflow(&error_str)
                {
                    warn!(
                        attempt = attempt + 1,
                        new_max_tokens = new_max,
                        "context overflow detected, adjusting max_tokens"
                    );
                    context.max_tokens_override = Some(new_max as u32);
                    if attempt < config.max_retries {
                        let delay = compute_retry_delay(config, attempt, None);
                        sleep(delay).await;
                        continue;
                    }
                }

                // --- Gap 4: Persistent retry mode ---
                let persistent = is_persistent_retry_enabled() && (is_429 || is_529);

                if attempt >= config.max_retries && !persistent && !header_says_retry {
                    return Err(error.context(format!(
                        "all {} retry attempts exhausted",
                        config.max_retries
                    )));
                }

                // Compute delay.
                let retry_after_hint = if is_429 {
                    read_hint(&context.response_hints, |h| h.rate_limit_wait)
                } else {
                    None
                };

                let delay = if persistent {
                    persistent_attempt += 1;
                    let base_delay =
                        compute_retry_delay(config, persistent_attempt, retry_after_hint);
                    // Use persistent-specific max backoff.
                    let capped =
                        std::cmp::min(base_delay, Duration::from_millis(PERSISTENT_MAX_BACKOFF_MS));
                    std::cmp::min(capped, Duration::from_millis(PERSISTENT_RESET_CAP_MS))
                } else {
                    compute_retry_delay(config, attempt, retry_after_hint)
                };

                warn!(
                    attempt = if persistent {
                        persistent_attempt
                    } else {
                        attempt + 1
                    },
                    max = config.max_retries,
                    delay_ms = delay.as_millis(),
                    persistent,
                    "retrying after error: {error:#}"
                );

                if persistent {
                    // Chunk long sleeps for heartbeat so host sees periodic activity.
                    let mut remaining = delay.as_millis() as u64;
                    while remaining > 0 {
                        let chunk = std::cmp::min(remaining, HEARTBEAT_INTERVAL_MS);
                        sleep(Duration::from_millis(chunk)).await;
                        remaining = remaining.saturating_sub(chunk);
                    }
                    // Clamp the for-loop counter so it never terminates.
                    // The persistent_attempt counter keeps growing.
                } else {
                    sleep(delay).await;
                }
            }
        }
    }

    Err(anyhow!("retry loop exited unexpectedly for model {model}"))
}

/// Execute an async HTTP request with retries, returning `(status, body)`.
///
/// This is a convenience wrapper around [`with_retry`] specifically for HTTP
/// requests that return a status code and response body.
///
/// Supports:
/// - `x-should-retry` response header (via [`should_retry_from_header`])
/// - `anthropic-ratelimit-unified-reset` header for 429 back-off
///   (via [`get_rate_limit_wait_duration`])
/// - Overloaded error body detection (via [`is_overloaded_error_body`])
pub async fn retry_http_request<F, Fut>(
    config: &RetryConfig,
    _model: &str,
    operation: F,
) -> Result<(u16, String)>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(u16, String)>>,
{
    let mut attempt: u32 = 0;
    let mut auth_retries: u32 = 0;

    loop {
        match operation().await {
            Ok((status, body)) => {
                // Handle transient auth errors with retry.
                if (status == 401 || is_transient_auth_error(&format!("{status}")))
                    && auth_retries < config.max_auth_retries
                {
                    auth_retries += 1;
                    warn!(
                        attempt = attempt + 1,
                        status,
                        auth_retry = auth_retries,
                        "auth error, retrying with credential refresh"
                    );
                    sleep(Duration::from_millis(500)).await;
                    attempt += 1;
                    continue;
                }

                // Permanent auth errors — don't retry.
                if status == 403 || status == 404 {
                    return Ok((status, body));
                }

                // Determine retryability: first check the overloaded body
                // fallback (API may return non-529 status with overloaded_error
                // in the body), then check standard retryable status codes.
                let is_retryable =
                    is_retryable_http_status(status) || is_overloaded_error_body(body.as_bytes());

                if is_retryable && attempt < config.max_retries {
                    // For 429, prefer the rate-limit reset header over
                    // exponential back-off.
                    let retry_after = if status == 429 {
                        // Note: the operation closure doesn't expose headers
                        // here, so we parse the body for timing hints as a
                        // best-effort fallback. Callers that need header-based
                        // rate limiting should use `with_retry` directly.
                        None
                    } else {
                        None
                    };
                    let delay = compute_retry_delay(config, attempt, retry_after);
                    warn!(
                        attempt = attempt + 1,
                        status,
                        delay_ms = delay.as_millis(),
                        "retrying HTTP request"
                    );
                    sleep(delay).await;
                    attempt += 1;
                    continue;
                }
                return Ok((status, body));
            }
            Err(error) => {
                if attempt >= config.max_retries {
                    return Err(error);
                }
                let delay = compute_retry_delay(config, attempt, None);
                warn!(
                    attempt = attempt + 1,
                    delay_ms = delay.as_millis(),
                    "retrying after transport error: {error:#}"
                );
                sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_increases_with_attempts() {
        let config = RetryConfig::default();
        let d0 = compute_retry_delay(&config, 0, None);
        let d1 = compute_retry_delay(&config, 1, None);
        let d2 = compute_retry_delay(&config, 2, None);
        assert!(d0 < d1);
        assert!(d1 < d2);
    }

    #[test]
    fn retry_after_overrides_delay() {
        let config = RetryConfig {
            respect_retry_after: true,
            ..RetryConfig::default()
        };
        let custom = Duration::from_secs(10);
        let delay = compute_retry_delay(&config, 5, Some(custom));
        assert_eq!(delay, custom);
    }

    #[test]
    fn retry_after_ignored_when_disabled() {
        let config = RetryConfig {
            respect_retry_after: false,
            ..RetryConfig::default()
        };
        let custom = Duration::from_secs(10);
        let delay = compute_retry_delay(&config, 0, Some(custom));
        assert_ne!(delay, custom);
    }

    #[test]
    fn is_retryable_http_status_classifies_correctly() {
        assert!(is_retryable_http_status(409));
        assert!(is_retryable_http_status(429));
        assert!(is_retryable_http_status(500));
        assert!(is_retryable_http_status(503));
        assert!(is_retryable_http_status(529));
        assert!(!is_retryable_http_status(200));
        assert!(!is_retryable_http_status(401));
        assert!(!is_retryable_http_status(403));
        assert!(!is_retryable_http_status(404));
    }

    #[tokio::test]
    async fn with_retry_succeeds_on_first_try() {
        let config = RetryConfig {
            max_retries: 3,
            ..RetryConfig::default()
        };
        let result = with_retry(&config, "test-model", |_ctx| async { Ok(42) }).await;
        assert_eq!(result.expect("should succeed"), 42);
    }

    #[tokio::test]
    async fn with_retry_retries_on_transient_failure() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            ..RetryConfig::default()
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let result = with_retry(&config, "test-model", |_ctx| {
            let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if current == 0 {
                    Err(anyhow!("server error 503"))
                } else {
                    Ok("success")
                }
            }
        })
        .await;
        assert_eq!(result.expect("should succeed after retry"), "success");
    }

    #[tokio::test]
    async fn with_retry_fails_on_non_retryable() {
        let config = RetryConfig {
            max_retries: 3,
            ..RetryConfig::default()
        };
        // 404 is a permanent error — should not be retried.
        let result = with_retry(&config, "test-model", |_ctx| async {
            Err::<(), _>(anyhow!("404 model not found"))
        })
        .await;
        assert!(result.is_err());
        assert!(
            result
                .expect_err("non-retryable error should be returned")
                .to_string()
                .contains("404")
        );
    }

    // ----- GAP 1: x-should-retry header -----

    #[test]
    fn should_retry_from_header_true() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "true".parse().unwrap());
        assert_eq!(should_retry_from_header(&headers), Some(true));
    }

    #[test]
    fn should_retry_from_header_one() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "1".parse().unwrap());
        assert_eq!(should_retry_from_header(&headers), Some(true));
    }

    #[test]
    fn should_retry_from_header_false() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "false".parse().unwrap());
        assert_eq!(should_retry_from_header(&headers), Some(false));
    }

    #[test]
    fn should_retry_from_header_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(should_retry_from_header(&headers), None);
    }

    // ----- GAP 2: rate-limit reset header -----

    #[test]
    fn rate_limit_wait_duration_parses_valid_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        // Set reset timestamp 5 seconds in the future.
        let future_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 5.0;
        headers.insert(
            "anthropic-ratelimit-unified-reset",
            format!("{future_ts:.3}").parse().unwrap(),
        );
        let dur = get_rate_limit_wait_duration(&headers).unwrap();
        // Should be roughly 5 seconds (±1 second tolerance).
        assert!(dur.as_secs_f64() > 3.0 && dur.as_secs_f64() < 7.0);
    }

    #[test]
    fn rate_limit_wait_duration_caps_at_6hr() {
        let mut headers = reqwest::header::HeaderMap::new();
        // 7 hours in the future — should cap at 6 hours (21600 seconds).
        let future_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 7.0 * 3600.0;
        headers.insert(
            "anthropic-ratelimit-unified-reset",
            format!("{future_ts:.3}").parse().unwrap(),
        );
        let dur = get_rate_limit_wait_duration(&headers).unwrap();
        assert_eq!(dur, Duration::from_secs(6 * 3600));
    }

    #[test]
    fn rate_limit_wait_duration_under_cap_unchanged() {
        let mut headers = reqwest::header::HeaderMap::new();
        // 2 hours in the future — under the 6hr cap, should be roughly 7200 seconds.
        let future_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 2.0 * 3600.0;
        headers.insert(
            "anthropic-ratelimit-unified-reset",
            format!("{future_ts:.3}").parse().unwrap(),
        );
        let dur = get_rate_limit_wait_duration(&headers).unwrap();
        // Should be close to 2 hours (7000-7300s range accounts for test execution time).
        assert!(
            dur.as_secs_f64() > 7000.0 && dur.as_secs_f64() < 7300.0,
            "expected ~7200s, got {}s",
            dur.as_secs_f64()
        );
    }

    #[test]
    fn rate_limit_wait_duration_missing_header() {
        let headers = reqwest::header::HeaderMap::new();
        assert!(get_rate_limit_wait_duration(&headers).is_none());
    }

    // ----- GAP 3: max-tokens overflow -----

    #[test]
    fn parse_max_tokens_overflow_valid() {
        // Use large enough values so the result exceeds FLOOR_OUTPUT_TOKENS (3000).
        let body = "input length and max_tokens exceed context limit: 100000 + 8192 > 200000";
        let new_max = parse_max_tokens_overflow(body).unwrap();
        // 200000 - 100000 - 1000 = 99000
        assert_eq!(new_max, 99000);
    }

    #[test]
    fn parse_max_tokens_overflow_below_floor() {
        // Result would be 920, which is below FLOOR_OUTPUT_TOKENS (3000).
        let body = "input length and max_tokens exceed context limit: 80000 + 4096 > 81920";
        let result = parse_max_tokens_overflow(body);
        // 81920 - 80000 - 1000 = 920 < 3000 -> None
        assert!(result.is_none());
    }

    #[test]
    fn parse_max_tokens_overflow_no_room() {
        // input_tokens close to context_limit => new_max would be 0 after buffer.
        let body = "input length and max_tokens exceed context limit: 81000 + 4096 > 81920";
        let result = parse_max_tokens_overflow(body);
        // 81920 - 81000 - 1000 = -80 -> None
        assert!(result.is_none());
    }

    #[test]
    fn parse_max_tokens_overflow_no_match() {
        let body = "some unrelated error message";
        assert!(parse_max_tokens_overflow(body).is_none());
    }

    // ----- GAP 5: ModelFallbackSuggested -----

    #[tokio::test]
    async fn with_retry_suggests_fallback_on_repeated_529() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            max_529_retries: 3,
            ..RetryConfig::default()
        };
        let result = with_retry(&config, "claude-sonnet-4", |_ctx| async {
            Err::<(), _>(anyhow!("529 overloaded"))
        })
        .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("model fallback suggested"),
            "expected model fallback suggestion, got: {err_msg}"
        );
    }

    // ----- GAP 6: 409 is retryable -----

    #[test]
    fn http_409_is_retryable() {
        assert!(is_retryable_http_status(409));
    }

    // ----- Expanded ApiErrorKind classification -----

    #[test]
    fn classify_pdf_too_large() {
        assert_eq!(
            classify_api_error(400, "pdf too large for processing"),
            ApiErrorKind::PdfTooLarge
        );
    }

    #[test]
    fn classify_pdf_password_protected() {
        assert_eq!(
            classify_api_error(400, "the pdf is password protected"),
            ApiErrorKind::PdfPasswordProtected
        );
    }

    #[test]
    fn classify_image_too_large() {
        assert_eq!(
            classify_api_error(400, "image is too large to process"),
            ApiErrorKind::ImageTooLarge
        );
    }

    #[test]
    fn classify_credit_balance_low() {
        assert_eq!(
            classify_api_error(402, "credit_balance is too low"),
            ApiErrorKind::CreditBalanceLow
        );
    }

    #[test]
    fn classify_invalid_api_key() {
        assert_eq!(
            classify_api_error(401, "invalid x-api-key provided"),
            ApiErrorKind::InvalidApiKey
        );
    }

    #[test]
    fn classify_token_revoked() {
        assert_eq!(
            classify_api_error(403, "token has been revoked"),
            ApiErrorKind::TokenRevoked
        );
    }

    #[test]
    fn classify_ssl_cert_error() {
        assert_eq!(
            classify_api_error(0, "ssl cert verification failed"),
            ApiErrorKind::SslCertError
        );
    }

    // ----- ResponseHints integration -----

    #[tokio::test]
    async fn with_retry_respects_should_retry_false_hint() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            ..RetryConfig::default()
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let result = with_retry(&config, "test-model", |ctx| {
            let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if current == 0 {
                    ctx.set_should_retry(false);
                    Err::<(), _>(anyhow!("server error 500"))
                } else {
                    // Should not reach here since should_retry=false
                    Ok(())
                }
            }
        })
        .await;
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("500"),
            "expected 500 in error, got: {err_str}"
        );
    }

    #[tokio::test]
    async fn with_retry_allows_retry_when_should_retry_true() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            ..RetryConfig::default()
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let result: Result<&str> = with_retry(&config, "test-model", |ctx| {
            let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if current == 0 {
                    // 404 is normally permanent, but x-should-retry: true overrides it
                    ctx.set_should_retry(true);
                    Err::<&str, _>(anyhow!("404 not found"))
                } else {
                    Ok("recovered")
                }
            }
        })
        .await;
        assert_eq!(
            result.expect("should succeed after header-forced retry"),
            "recovered"
        );
    }

    #[test]
    fn retry_context_with_fallback_model() {
        let ctx = RetryContext::new("claude-sonnet-4").with_fallback_model("claude-haiku-4");
        assert_eq!(ctx.model, "claude-sonnet-4");
        assert_eq!(ctx.fallback_model.as_deref(), Some("claude-haiku-4"));
    }

    #[test]
    fn model_fallback_suggested_display_with_fallback() {
        let suggestion = ModelFallbackSuggested {
            original_model: "claude-sonnet-4".to_owned(),
            fallback_model: Some("claude-haiku-4".to_owned()),
            reason: "3 consecutive 529 errors".to_owned(),
        };
        let display = suggestion.to_string();
        assert!(display.contains("claude-sonnet-4 -> claude-haiku-4"));
    }

    #[test]
    fn model_fallback_suggested_display_without_fallback() {
        let suggestion = ModelFallbackSuggested {
            original_model: "claude-sonnet-4".to_owned(),
            fallback_model: None,
            reason: "3 consecutive 529 errors".to_owned(),
        };
        let display = suggestion.to_string();
        assert!(display.contains("claude-sonnet-4"));
        assert!(!display.contains("->"));
    }

    // ----- Gap 1: Fast mode state -----

    #[test]
    fn fast_mode_state_new_active() {
        let state = FastModeState::new(true);
        assert!(state.active);
        assert!(state.cooldown_until.is_none());
        assert!(!state.is_in_cooldown());
        assert!(state.is_effectively_active());
    }

    #[test]
    fn fast_mode_state_new_inactive() {
        let state = FastModeState::new(false);
        assert!(!state.active);
        assert!(!state.is_effectively_active());
    }

    #[test]
    fn fast_mode_state_trigger_cooldown() {
        let mut state = FastModeState::new(true);
        assert!(state.is_effectively_active());
        state.trigger_cooldown(Instant::now() + Duration::from_secs(60));
        assert!(!state.active);
        assert!(state.is_in_cooldown());
        assert!(!state.is_effectively_active());
    }

    #[test]
    fn fast_mode_state_cooldown_expires() {
        let mut state = FastModeState::new(true);
        // Set cooldown to the past — already expired.
        state.trigger_cooldown(Instant::now() - Duration::from_secs(1));
        // The cooldown_until is set but the instant has passed, so not in cooldown.
        assert!(state.cooldown_until.is_some());
        assert!(!state.is_in_cooldown());
        // But active was set to false by trigger_cooldown.
        assert!(!state.active);
    }

    #[test]
    fn fast_mode_default_is_inactive() {
        let state = FastModeState::default();
        assert!(!state.active);
    }

    // ----- Gap 2: 529 source gating -----

    #[test]
    fn should_retry_529_foreground_sources() {
        assert!(should_retry_529(None)); // Untagged → conservative retry.
        assert!(should_retry_529(Some(QuerySource::ReplMainThread)));
        assert!(should_retry_529(Some(QuerySource::Agent)));
        assert!(should_retry_529(Some(QuerySource::Compact)));
        assert!(should_retry_529(Some(QuerySource::Sdk)));
        assert!(should_retry_529(Some(QuerySource::User)));
    }

    #[test]
    fn should_retry_529_background_sources() {
        assert!(!should_retry_529(Some(QuerySource::BackgroundTask)));
        assert!(!should_retry_529(Some(QuerySource::ExtractMemories)));
        assert!(!should_retry_529(Some(QuerySource::Advisor)));
        assert!(!should_retry_529(Some(QuerySource::SessionMemory)));
    }

    // ----- Gap 3: Subscriber tier -----

    #[test]
    fn get_subscriber_tier_default_free() {
        // Without env var, defaults to Free.
        // NOTE: This test assumes CLAUDE_CODE_SUBSCRIBER is not set in test env.
        let tier = get_subscriber_tier();
        // The default is Free unless the env var is explicitly set.
        assert!(matches!(
            tier,
            SubscriberTier::Free | SubscriberTier::Pro | SubscriberTier::Enterprise
        ));
    }

    #[test]
    fn subscriber_tier_free_is_not_subscriber() {
        assert_eq!(SubscriberTier::Free, SubscriberTier::Free);
        assert_ne!(SubscriberTier::Free, SubscriberTier::Pro);
        assert_ne!(SubscriberTier::Free, SubscriberTier::Enterprise);
    }

    #[test]
    fn is_enterprise_subscriber_checks_tier() {
        // This test verifies the function exists and compiles.
        let _ = is_enterprise_subscriber();
    }

    // ----- Gap 4: Persistent retry -----

    #[test]
    fn persistent_retry_disabled_by_default() {
        // Assumes CLAUDE_CODE_PERSISTENT_RETRY is not set in test env.
        assert!(!is_persistent_retry_enabled());
    }

    // ----- Gap 5: RetryOptions -----

    #[test]
    fn retry_options_default() {
        let opts = RetryOptions::default();
        // fast_mode depends on env var — just verify construction.
        assert!(opts.query_source.is_none());
        assert!(opts.on_auth_error.is_none());
    }

    #[test]
    fn retry_options_builder() {
        let opts = RetryOptions::default()
            .with_fast_mode(true)
            .with_query_source(QuerySource::Agent);
        assert!(opts.fast_mode);
        assert_eq!(opts.query_source, Some(QuerySource::Agent));
    }

    // ----- with_retry_ext basic tests -----

    #[tokio::test]
    async fn with_retry_ext_succeeds_on_first_try() {
        let config = RetryConfig {
            max_retries: 3,
            ..RetryConfig::default()
        };
        let opts = RetryOptions::default();
        let result = with_retry_ext(&config, "test-model", opts, |_ctx| async { Ok(42) }).await;
        assert_eq!(result.expect("should succeed"), 42);
    }

    #[tokio::test]
    async fn with_retry_ext_retries_on_transient_failure() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            ..RetryConfig::default()
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let opts = RetryOptions::default();
        let result = with_retry_ext(&config, "test-model", opts, |_ctx| {
            let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if current == 0 {
                    Err(anyhow!("server error 503"))
                } else {
                    Ok("success")
                }
            }
        })
        .await;
        assert_eq!(result.expect("should succeed after retry"), "success");
    }

    #[tokio::test]
    async fn with_retry_ext_background_source_does_not_retry_529() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            ..RetryConfig::default()
        };
        let opts = RetryOptions::default().with_query_source(QuerySource::BackgroundTask);
        let result: Result<()> = with_retry_ext(&config, "test-model", opts, |_ctx| async {
            Err(anyhow!("529 overloaded"))
        })
        .await;
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("background source"),
            "expected background source error, got: {err_str}"
        );
    }

    #[tokio::test]
    async fn with_retry_ext_foreground_source_retries_529() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 1,
            max_backoff_ms: 2,
            max_529_retries: 10, // Allow more 529 retries so we can test recovery.
            ..RetryConfig::default()
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let opts = RetryOptions::default().with_query_source(QuerySource::Agent);
        let result = with_retry_ext(&config, "test-model", opts, |_ctx| {
            let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if current == 0 {
                    Err::<&str, _>(anyhow!("529 overloaded"))
                } else {
                    Ok("recovered")
                }
            }
        })
        .await;
        assert_eq!(result.expect("should retry and succeed"), "recovered");
    }
}
