//! Rate-limit tracking and HTTP header parsing.
//!
//! Provides utilities for tracking API rate limits and parsing rate-limit
//! headers from HTTP responses (X-RateLimit-* style).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// RateLimitInfo
// ---------------------------------------------------------------------------

/// Snapshot of rate-limit state for a single provider / endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    /// Remaining requests in the current window.
    pub requests_remaining: u64,
    /// Remaining tokens in the current window.
    pub tokens_remaining: u64,
    /// When the rate limit window resets (UNIX epoch seconds).
    pub reset_at: u64,
}

impl RateLimitInfo {
    /// Create a new rate-limit info with the given values.
    #[must_use]
    pub fn new(requests_remaining: u64, tokens_remaining: u64, reset_at: u64) -> Self {
        Self {
            requests_remaining,
            tokens_remaining,
            reset_at,
        }
    }

    /// Whether the request quota has been exhausted.
    #[must_use]
    pub fn is_requests_exhausted(&self) -> bool {
        self.requests_remaining == 0
    }

    /// Whether the token quota has been exhausted.
    #[must_use]
    pub fn is_tokens_exhausted(&self) -> bool {
        self.tokens_remaining == 0
    }

    /// Seconds until the rate limit resets.
    #[must_use]
    pub fn seconds_until_reset(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        self.reset_at.saturating_sub(now)
    }
}

impl Default for RateLimitInfo {
    fn default() -> Self {
        Self {
            requests_remaining: u64::MAX,
            tokens_remaining: u64::MAX,
            reset_at: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimitTracker
// ---------------------------------------------------------------------------

/// Tracks rate-limit state across multiple providers.
#[derive(Debug)]
pub struct RateLimitTracker {
    limits: Mutex<HashMap<String, RateLimitInfo>>,
    total_throttled: AtomicU64,
}

impl RateLimitTracker {
    /// Create a new empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            limits: Mutex::new(HashMap::new()),
            total_throttled: AtomicU64::new(0),
        }
    }

    /// Update the rate-limit info for a provider.
    pub fn update(&self, provider: &str, info: RateLimitInfo) {
        self.limits.lock().insert(provider.to_string(), info);
    }

    /// Get the current rate-limit info for a provider.
    #[must_use]
    pub fn get(&self, provider: &str) -> Option<RateLimitInfo> {
        self.limits.lock().get(provider).cloned()
    }

    /// Check whether a provider is currently rate-limited.
    #[must_use]
    pub fn is_limited(&self, provider: &str) -> bool {
        if let Some(info) = self.get(provider) {
            info.is_requests_exhausted() || info.is_tokens_exhausted()
        } else {
            false
        }
    }

    /// Record a throttled request (increment counter).
    pub fn record_throttle(&self) {
        self.total_throttled.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the total number of throttled requests.
    #[must_use]
    pub fn total_throttled(&self) -> u64 {
        self.total_throttled.load(Ordering::Relaxed)
    }

    /// Clear all tracked rate limits.
    pub fn clear(&self) {
        self.limits.lock().clear();
    }

    /// List all tracked provider names.
    #[must_use]
    pub fn providers(&self) -> Vec<String> {
        self.limits.lock().keys().cloned().collect()
    }
}

impl Default for RateLimitTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// check_rate_limit
// ---------------------------------------------------------------------------

/// Result of a rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitStatus {
    /// Request can proceed.
    Allowed,
    /// Request should be delayed or rejected.
    Limited,
}

/// Check whether a request to the given provider is allowed.
pub fn check_rate_limit(tracker: &RateLimitTracker, provider: &str) -> RateLimitStatus {
    if tracker.is_limited(provider) {
        tracker.record_throttle();
        RateLimitStatus::Limited
    } else {
        RateLimitStatus::Allowed
    }
}

// ---------------------------------------------------------------------------
// parse_rate_limit_headers
// ---------------------------------------------------------------------------

/// Parse rate-limit information from HTTP response headers.
///
/// Supports common header conventions:
/// - `x-ratelimit-remaining` — remaining requests
/// - `x-ratelimit-reset` — reset timestamp (UNIX seconds)
/// - `x-ratelimit-tokens-remaining` — remaining tokens (custom)
/// - `retry-after` — seconds until retry (fallback for reset)
pub fn parse_rate_limit_headers(headers: &HashMap<String, String>) -> RateLimitInfo {
    let requests_remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(u64::MAX);

    let tokens_remaining = headers
        .get("x-ratelimit-tokens-remaining")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(u64::MAX);

    let reset_at = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or_else(|| {
            // Fallback: use retry-after + current time
            headers
                .get("retry-after")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|retry| {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs();
                    now + retry
                })
                .unwrap_or(0)
        });

    RateLimitInfo {
        requests_remaining,
        tokens_remaining,
        reset_at,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RateLimitInfo ---------------------------------------------------------

    #[test]
    fn rate_limit_info_new() {
        let info = RateLimitInfo::new(100, 5000, 1700000000);
        assert_eq!(info.requests_remaining, 100);
        assert_eq!(info.tokens_remaining, 5000);
        assert_eq!(info.reset_at, 1700000000);
    }

    #[test]
    fn rate_limit_info_default() {
        let info = RateLimitInfo::default();
        assert_eq!(info.requests_remaining, u64::MAX);
        assert_eq!(info.tokens_remaining, u64::MAX);
        assert_eq!(info.reset_at, 0);
    }

    #[test]
    fn rate_limit_info_exhausted() {
        let info = RateLimitInfo::new(0, 5000, 0);
        assert!(info.is_requests_exhausted());
        assert!(!info.is_tokens_exhausted());
    }

    #[test]
    fn rate_limit_info_tokens_exhausted() {
        let info = RateLimitInfo::new(10, 0, 0);
        assert!(!info.is_requests_exhausted());
        assert!(info.is_tokens_exhausted());
    }

    #[test]
    fn rate_limit_info_seconds_until_reset() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let info = RateLimitInfo::new(10, 100, now + 60);
        let secs = info.seconds_until_reset();
        assert!((59..=61).contains(&secs), "expected ~60, got {secs}");
    }

    #[test]
    fn rate_limit_info_seconds_until_reset_past() {
        let info = RateLimitInfo::new(10, 100, 0);
        assert_eq!(info.seconds_until_reset(), 0);
    }

    #[test]
    fn rate_limit_info_serialization() {
        let info = RateLimitInfo::new(42, 1000, 1700000000);
        let json = serde_json::to_string(&info).expect("serialize");
        let back: RateLimitInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(info.requests_remaining, back.requests_remaining);
        assert_eq!(info.tokens_remaining, back.tokens_remaining);
        assert_eq!(info.reset_at, back.reset_at);
    }

    // -- RateLimitTracker ------------------------------------------------------

    #[test]
    fn tracker_new_is_empty() {
        let tracker = RateLimitTracker::new();
        assert!(tracker.providers().is_empty());
        assert_eq!(tracker.total_throttled(), 0);
    }

    #[test]
    fn tracker_update_and_get() {
        let tracker = RateLimitTracker::new();
        let info = RateLimitInfo::new(50, 2000, 1700000000);
        tracker.update("openai", info.clone());
        let got = tracker.get("openai").expect("should exist");
        assert_eq!(got.requests_remaining, 50);
        assert_eq!(got.tokens_remaining, 2000);
    }

    #[test]
    fn tracker_get_missing_returns_none() {
        let tracker = RateLimitTracker::new();
        assert!(tracker.get("nonexistent").is_none());
    }

    #[test]
    fn tracker_is_limited_when_exhausted() {
        let tracker = RateLimitTracker::new();
        tracker.update("test", RateLimitInfo::new(0, 100, 0));
        assert!(tracker.is_limited("test"));
    }

    #[test]
    fn tracker_not_limited_when_has_quota() {
        let tracker = RateLimitTracker::new();
        tracker.update("test", RateLimitInfo::new(10, 100, 0));
        assert!(!tracker.is_limited("test"));
    }

    #[test]
    fn tracker_not_limited_for_unknown() {
        let tracker = RateLimitTracker::new();
        assert!(!tracker.is_limited("unknown"));
    }

    #[test]
    fn tracker_record_throttle() {
        let tracker = RateLimitTracker::new();
        tracker.record_throttle();
        tracker.record_throttle();
        assert_eq!(tracker.total_throttled(), 2);
    }

    #[test]
    fn tracker_clear() {
        let tracker = RateLimitTracker::new();
        tracker.update("a", RateLimitInfo::new(1, 1, 1));
        tracker.update("b", RateLimitInfo::new(2, 2, 2));
        tracker.clear();
        assert!(tracker.providers().is_empty());
    }

    #[test]
    fn tracker_providers() {
        let tracker = RateLimitTracker::new();
        tracker.update("openai", RateLimitInfo::default());
        tracker.update("anthropic", RateLimitInfo::default());
        let mut provs = tracker.providers();
        provs.sort();
        assert_eq!(provs, vec!["anthropic", "openai"]);
    }

    // -- check_rate_limit ------------------------------------------------------

    #[test]
    fn check_rate_limit_allowed() {
        let tracker = RateLimitTracker::new();
        tracker.update("test", RateLimitInfo::new(10, 100, 0));
        assert_eq!(check_rate_limit(&tracker, "test"), RateLimitStatus::Allowed);
        assert_eq!(tracker.total_throttled(), 0);
    }

    #[test]
    fn check_rate_limit_limited() {
        let tracker = RateLimitTracker::new();
        tracker.update("test", RateLimitInfo::new(0, 100, 0));
        assert_eq!(check_rate_limit(&tracker, "test"), RateLimitStatus::Limited);
        assert_eq!(tracker.total_throttled(), 1);
    }

    #[test]
    fn check_rate_limit_unknown_provider() {
        let tracker = RateLimitTracker::new();
        assert_eq!(
            check_rate_limit(&tracker, "unknown"),
            RateLimitStatus::Allowed
        );
    }

    // -- parse_rate_limit_headers ----------------------------------------------

    #[test]
    fn parse_headers_full() {
        let mut headers = HashMap::new();
        headers.insert("x-ratelimit-remaining".into(), "42".into());
        headers.insert("x-ratelimit-tokens-remaining".into(), "5000".into());
        headers.insert("x-ratelimit-reset".into(), "1700000100".into());

        let info = parse_rate_limit_headers(&headers);
        assert_eq!(info.requests_remaining, 42);
        assert_eq!(info.tokens_remaining, 5000);
        assert_eq!(info.reset_at, 1700000100);
    }

    #[test]
    fn parse_headers_missing_defaults() {
        let headers = HashMap::new();
        let info = parse_rate_limit_headers(&headers);
        assert_eq!(info.requests_remaining, u64::MAX);
        assert_eq!(info.tokens_remaining, u64::MAX);
        assert_eq!(info.reset_at, 0);
    }

    #[test]
    fn parse_headers_retry_after_fallback() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".into(), "30".into());
        let info = parse_rate_limit_headers(&headers);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        assert!(info.reset_at >= now + 29 && info.reset_at <= now + 31);
    }

    #[test]
    fn parse_headers_invalid_values() {
        let mut headers = HashMap::new();
        headers.insert("x-ratelimit-remaining".into(), "not-a-number".into());
        headers.insert("x-ratelimit-reset".into(), "bad".into());
        let info = parse_rate_limit_headers(&headers);
        assert_eq!(info.requests_remaining, u64::MAX);
    }

    #[test]
    fn parse_headers_case_sensitive_keys() {
        let mut headers = HashMap::new();
        headers.insert("X-RateLimit-Remaining".into(), "10".into());
        // HashMap is case-sensitive; our parser expects lowercase
        let info = parse_rate_limit_headers(&headers);
        assert_eq!(info.requests_remaining, u64::MAX);
    }
}
