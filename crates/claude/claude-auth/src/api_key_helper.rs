//! API Key Helper with time-based cache (5-minute TTL).
//!
//! Executes an external command to obtain an API key, caches the result,
//! and serves stale values while refreshing in the background (SWR pattern).
//!
//! Mirrors `utils/auth.ts` — `getApiKeyFromApiKeyHelper` and related functions.

use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use tracing::{debug, warn};

/// Default TTL for the API key helper cache (5 minutes).
pub const DEFAULT_API_KEY_HELPER_TTL: Duration = Duration::from_secs(5 * 60);

static GLOBAL_API_KEY_HELPER_CACHE: Lazy<ApiKeyHelperCache> = Lazy::new(ApiKeyHelperCache::new);

const API_KEY_HELPER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const API_KEY_HELPER_FAILURE_SENTINEL: &str = " ";

/// The source of an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySource {
    /// `ANTHROPIC_API_KEY` environment variable.
    EnvVar,
    /// apiKeyHelper command output.
    ApiKeyHelper,
    /// `/login` managed key (stored in config or keychain).
    ManagedKey,
    /// No key available.
    None,
}

/// Cached API key result.
#[derive(Debug, Clone)]
pub struct ApiKeyHelperResult {
    /// The API key value.
    pub key: String,
    /// Where the key came from.
    pub source: ApiKeySource,
    /// When the key was cached.
    pub cached_at: DateTime<Utc>,
}

/// Errors from the API key helper.
#[derive(Debug, thiserror::Error)]
pub enum ApiKeyHelperError {
    #[error("apiKeyHelper command failed: {0}")]
    CommandFailed(String),

    #[error("apiKeyHelper returned empty output")]
    EmptyOutput,

    #[error("apiKeyHelper not configured")]
    NotConfigured,

    #[error("command execution error: {0}")]
    ExecError(String),
}

/// Thread-safe API key helper cache with TTL.
pub struct ApiKeyHelperCache {
    inner: StdMutex<Option<CachedEntry>>,
    refresh_inflight: StdMutex<bool>,
    cold_inflight: tokio::sync::Mutex<()>,
    ttl: Duration,
}

#[derive(Debug)]
struct CachedEntry {
    value: String,
    cached_at: Instant,
}

impl ApiKeyHelperCache {
    /// Create a new cache with the default 5-minute TTL.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: StdMutex::new(None),
            refresh_inflight: StdMutex::new(false),
            cold_inflight: tokio::sync::Mutex::new(()),
            ttl: calculate_api_key_helper_ttl(),
        }
    }

    /// Create a new cache with a custom TTL.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: StdMutex::new(None),
            refresh_inflight: StdMutex::new(false),
            cold_inflight: tokio::sync::Mutex::new(()),
            ttl,
        }
    }

    /// Get a cached API key if it exists and is not expired.
    pub fn get_cached(&self) -> Option<String> {
        let inner = lock_or_recover(&self.inner);
        match inner.as_ref() {
            Some(entry) if entry.cached_at.elapsed() < self.ttl => Some(entry.value.clone()),
            Some(entry) => {
                // Stale — caller should refresh in background
                debug!("API key cache is stale, returning stale value");
                Some(entry.value.clone())
            }
            None => None,
        }
    }

    /// Get a cached API key only if it's fresh (within TTL).
    pub fn get_fresh(&self) -> Option<String> {
        let inner = lock_or_recover(&self.inner);
        match inner.as_ref() {
            Some(entry) if entry.cached_at.elapsed() < self.ttl => Some(entry.value.clone()),
            _ => None,
        }
    }

    /// Store a value in the cache.
    pub fn set(&self, value: String) {
        let mut inner = lock_or_recover(&self.inner);
        *inner = Some(CachedEntry {
            value,
            cached_at: Instant::now(),
        });
    }

    /// Clear the cache.
    pub fn clear(&self) {
        let mut inner = lock_or_recover(&self.inner);
        *inner = None;
        *lock_or_recover(&self.refresh_inflight) = false;
    }

    /// Check if the cache has a value (even if stale).
    pub fn is_populated(&self) -> bool {
        lock_or_recover(&self.inner).is_some()
    }

    fn refresh_timestamp_for_current_value(&self) {
        let mut inner = lock_or_recover(&self.inner);
        if let Some(entry) = inner.as_mut() {
            entry.cached_at = Instant::now();
        }
    }

    fn try_start_refresh(&self) -> bool {
        let mut inflight = lock_or_recover(&self.refresh_inflight);
        if *inflight {
            return false;
        }
        *inflight = true;
        true
    }

    fn finish_refresh(&self) {
        *lock_or_recover(&self.refresh_inflight) = false;
    }
}

/// Lock a `std::sync::Mutex`, recovering from poison by logging a warning
/// and accessing the inner value anyway. This prevents a panicked thread
/// from crashing the entire process when the lock guard is still usable.
fn lock_or_recover<'a, T>(mutex: &'a StdMutex<T>) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("Mutex poisoned — another thread panicked while holding the lock; recovering");
            poisoned.into_inner()
        }
    }
}

fn calculate_api_key_helper_ttl() -> Duration {
    calculate_api_key_helper_ttl_from_lookup(|| {
        std::env::var("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").ok()
    })
}

fn calculate_api_key_helper_ttl_from_lookup<F>(mut lookup: F) -> Duration
where
    F: FnMut() -> Option<String>,
{
    lookup()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_API_KEY_HELPER_TTL)
}

impl Default for ApiKeyHelperCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute an apiKeyHelper command and cache the result.
///
/// The command is executed via the system shell. Its trimmed stdout is
/// used as the API key.
pub async fn execute_api_key_helper(
    command: &str,
    cache: &ApiKeyHelperCache,
) -> Result<ApiKeyHelperResult, ApiKeyHelperError> {
    if let Some(key) = cache.get_fresh() {
        debug!("Returning cached API key");
        return Ok(api_key_helper_result(key));
    }

    let is_cold = cache.get_cached().is_none();
    if is_cold {
        let _guard = cache.cold_inflight.lock().await;
        if let Some(key) = cache.get_fresh() {
            debug!("Returning cached API key after cold inflight wait");
            return Ok(api_key_helper_result(key));
        }
        if cache.get_cached().is_some() {
            return run_and_cache_api_key_helper(command, cache, false).await;
        }
        return run_and_cache_api_key_helper(command, cache, true).await;
    }

    run_and_cache_api_key_helper(command, cache, is_cold).await
}

/// Execute an apiKeyHelper command using the process-wide cache.
pub async fn execute_api_key_helper_cached(
    command: &str,
) -> Result<ApiKeyHelperResult, ApiKeyHelperError> {
    if let Some(key) = GLOBAL_API_KEY_HELPER_CACHE.get_fresh() {
        debug!("Returning cached API key");
        return Ok(api_key_helper_result(key));
    }

    if let Some(stale) = GLOBAL_API_KEY_HELPER_CACHE.get_cached() {
        if GLOBAL_API_KEY_HELPER_CACHE.try_start_refresh() {
            let command = command.to_owned();
            tokio::spawn(async move {
                let result =
                    run_and_cache_api_key_helper(&command, &GLOBAL_API_KEY_HELPER_CACHE, false)
                        .await;
                if let Err(error) = result {
                    warn!("apiKeyHelper background refresh failed: {error}");
                }
                GLOBAL_API_KEY_HELPER_CACHE.finish_refresh();
            });
        }
        return Ok(api_key_helper_result(stale));
    }

    execute_api_key_helper(command, &GLOBAL_API_KEY_HELPER_CACHE).await
}

/// Clear the process-wide helper cache.
pub fn clear_global_api_key_helper_cache() {
    GLOBAL_API_KEY_HELPER_CACHE.clear();
}

fn api_key_helper_result(key: String) -> ApiKeyHelperResult {
    ApiKeyHelperResult {
        key,
        source: ApiKeySource::ApiKeyHelper,
        cached_at: Utc::now(),
    }
}

async fn run_and_cache_api_key_helper(
    command: &str,
    cache: &ApiKeyHelperCache,
    is_cold: bool,
) -> Result<ApiKeyHelperResult, ApiKeyHelperError> {
    debug!("Executing apiKeyHelper command");
    match run_api_key_helper_command(command).await {
        Ok(stdout) => {
            cache.set(stdout.clone());
            Ok(api_key_helper_result(stdout))
        }
        Err(error) => {
            warn!("apiKeyHelper failed: {error}");
            if !is_cold
                && let Some(stale) = cache.get_cached()
                && stale != API_KEY_HELPER_FAILURE_SENTINEL
            {
                cache.refresh_timestamp_for_current_value();
                return Ok(api_key_helper_result(stale));
            }
            cache.set(API_KEY_HELPER_FAILURE_SENTINEL.to_owned());
            Ok(api_key_helper_result(
                API_KEY_HELPER_FAILURE_SENTINEL.to_owned(),
            ))
        }
    }
}

async fn run_api_key_helper_command(command: &str) -> Result<String, ApiKeyHelperError> {
    let output = tokio::time::timeout(API_KEY_HELPER_TIMEOUT, shell_command(command).output())
        .await
        .map_err(|_| ApiKeyHelperError::CommandFailed("timed out".to_owned()))?
        .map_err(|e| ApiKeyHelperError::ExecError(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let message = if detail.is_empty() {
            output.status.code().map_or_else(
                || "terminated by signal".to_owned(),
                |code| format!("exited {code}"),
            )
        } else {
            output.status.code().map_or_else(
                || detail.to_owned(),
                |code| format!("exited {code}: {detail}"),
            )
        };
        return Err(ApiKeyHelperError::CommandFailed(message));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stdout.is_empty() {
        return Err(ApiKeyHelperError::EmptyOutput);
    }

    Ok(stdout)
}

fn shell_command(command: &str) -> tokio::process::Command {
    tracing::warn!(
        "Executing apiKeyHelper command from config — ensure this is trusted: {command}"
    );
    #[cfg(windows)]
    {
        let mut process = tokio::process::Command::new("cmd");
        process.args(["/C", command]);
        process
    }

    #[cfg(not(windows))]
    {
        let mut process = tokio::process::Command::new("sh");
        process.args(["-c", command]);
        process
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn cache_fresh() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_secs(60));
        cache.set("test-key".to_owned());
        assert_eq!(cache.get_fresh().as_deref(), Some("test-key"));
    }

    #[test]
    fn cache_expired_returns_none_fresh() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_nanos(1));
        cache.set("test-key".to_owned());
        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(1));
        assert!(cache.get_fresh().is_none());
    }

    #[test]
    fn cache_expired_returns_stale() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_nanos(1));
        cache.set("test-key".to_owned());
        std::thread::sleep(Duration::from_millis(1));
        // get_cached returns stale values
        assert_eq!(cache.get_cached().as_deref(), Some("test-key"));
    }

    #[test]
    fn cache_clear() {
        let cache = ApiKeyHelperCache::new();
        cache.set("test-key".to_owned());
        assert!(cache.is_populated());
        cache.clear();
        assert!(!cache.is_populated());
    }

    #[test]
    fn cache_default_ttl_uses_claude_env_override() {
        let ttl = calculate_api_key_helper_ttl_from_lookup(|| Some("42".to_owned()));
        assert_eq!(ttl, Duration::from_millis(42));
    }

    #[test]
    fn cache_default_ttl_ignores_invalid_claude_env_override() {
        let ttl = calculate_api_key_helper_ttl_from_lookup(|| Some("not-a-number".to_owned()));
        assert_eq!(ttl, DEFAULT_API_KEY_HELPER_TTL);
    }

    #[tokio::test]
    async fn helper_success_trims_shell_output() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_secs(60));
        let result = execute_api_key_helper("echo helper-key", &cache)
            .await
            .expect("helper should run");
        assert_eq!(result.key, "helper-key");
        assert_eq!(cache.get_fresh().as_deref(), Some("helper-key"));
    }

    #[tokio::test]
    async fn helper_cold_failure_caches_sentinel_without_error() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_secs(60));
        let result = execute_api_key_helper("exit 7", &cache)
            .await
            .expect("cold helper failure should not fall through to other auth");
        assert_eq!(result.key, API_KEY_HELPER_FAILURE_SENTINEL);
        assert_eq!(
            cache.get_fresh().as_deref(),
            Some(API_KEY_HELPER_FAILURE_SENTINEL)
        );
    }

    #[tokio::test]
    async fn helper_stale_failure_returns_stale_and_refreshes_timestamp() {
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_secs(1));
        cache.set("stale-key".to_owned());
        cache
            .inner
            .lock()
            .expect("cache lock")
            .as_mut()
            .expect("cached value")
            .cached_at = Instant::now() - Duration::from_secs(2);
        assert!(cache.get_fresh().is_none());

        let result = execute_api_key_helper("exit 7", &cache)
            .await
            .expect("stale helper failure should return stale key");

        assert_eq!(result.key, "stale-key");
        assert_eq!(cache.get_fresh().as_deref(), Some("stale-key"));
    }

    #[tokio::test]
    async fn helper_cold_calls_are_deduplicated() {
        let tempdir = tempdir().expect("tempdir");
        let marker = tempdir.path().join("helper-runs.txt");
        let command = helper_count_command(tempdir.path(), &marker);
        let cache = ApiKeyHelperCache::with_ttl(Duration::from_secs(60));

        let (first, second) = tokio::join!(
            execute_api_key_helper(&command, &cache),
            execute_api_key_helper(&command, &cache)
        );

        assert_eq!(first.expect("first helper").key, "dedupe-key");
        assert_eq!(second.expect("second helper").key, "dedupe-key");
        let contents = fs::read_to_string(marker).expect("marker should exist");
        assert_eq!(contents.lines().count(), 1);
    }

    fn helper_count_command(tempdir: &std::path::Path, marker: &std::path::Path) -> String {
        #[cfg(windows)]
        {
            let script = tempdir.join("helper.cmd");
            fs::write(
                &script,
                format!(
                    "@echo off\r\nping -n 2 127.0.0.1 >NUL\r\n>>\"{}\" echo hit\r\necho dedupe-key\r\n",
                    marker.display()
                ),
            )
            .expect("write helper script");
            script.display().to_string()
        }

        #[cfg(not(windows))]
        {
            let path = marker.display().to_string().replace('\'', "'\\''");
            format!("sleep 0.2; printf 'hit\\n' >> '{path}'; echo dedupe-key")
        }
    }
}
