//! Fast Mode management.
//!
//! Provides types and functions for managing the "fast mode" feature that
//! uses a faster/cheaper model for certain requests. Includes state
//! management for cooldowns after rate limits and API rejection handling.
//!
//! Ported from `claude-code-rev/src/utils/fastMode.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── Types ───────────────────────────────────────────────────────────────

/// The display name for the fast mode model.
pub const FAST_MODE_MODEL_DISPLAY: &str = "Opus 4.6";

/// Reasons why fast mode may be disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastModeDisabledReason {
    /// Free tier user — fast mode requires a paid subscription.
    Free,
    /// Disabled by organization admin preference.
    Preference,
    /// Extra usage (overage billing) is not enabled.
    ExtraUsageDisabled,
    /// Network error when checking fast mode status.
    NetworkError,
    /// Unknown reason.
    Unknown,
}

impl std::fmt::Display for FastModeDisabledReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Free => write!(f, "free"),
            Self::Preference => write!(f, "preference"),
            Self::ExtraUsageDisabled => write!(f, "extra_usage_disabled"),
            Self::NetworkError => write!(f, "network_error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Reason for fast mode cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CooldownReason {
    /// Rate limit hit.
    RateLimit,
    /// Service overloaded.
    Overloaded,
}

/// Fast mode operational state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FastModeState {
    /// Fast mode is available and active.
    #[default]
    Available,
    /// Fast mode is disabled with a reason.
    Disabled {
        /// Why fast mode is disabled.
        reason: FastModeDisabledReason,
    },
    /// Fast mode is in cooldown after a rate limit.
    Cooldown {
        /// When the cooldown expires.
        until: DateTime<Utc>,
        /// Why the cooldown was triggered.
        reason: CooldownReason,
    },
}

/// Simplified fast mode state for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastModeSimpleState {
    /// Fast mode is on.
    On,
    /// Fast mode is in cooldown.
    Cooldown,
    /// Fast mode is off.
    Off,
}

/// Organization-level fast mode status from the API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OrgFastModeStatus {
    /// Status not yet determined.
    #[default]
    Pending,
    /// Fast mode is enabled for the organization.
    Enabled,
    /// Fast mode is disabled for the organization.
    Disabled {
        /// Why fast mode is disabled.
        reason: FastModeDisabledReason,
    },
}

/// Configuration for fast mode resolution.
///
/// Replaces the implicit dependency on environment variables and global
/// state in the original TypeScript implementation.
#[derive(Debug, Clone)]
pub struct FastModeConfig {
    /// Whether fast mode is globally enabled (not disabled by env).
    pub enabled: bool,
    /// Whether the API provider is first-party (not Bedrock/Vertex/Foundry).
    pub is_first_party: bool,
    /// Whether this is a non-interactive SDK session.
    pub is_non_interactive_sdk: bool,
    /// Whether the session has kairos active (assistant daemon mode).
    pub kairos_active: bool,
    /// Whether fast mode is opted in via flag settings (for SDK sessions).
    pub flag_fast_mode: bool,
    /// Whether per-session opt-in is required.
    pub per_session_opt_in: bool,
    /// User's fast mode setting from settings.
    pub user_fast_mode_setting: Option<bool>,
}

impl Default for FastModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            is_first_party: true,
            is_non_interactive_sdk: false,
            kairos_active: false,
            flag_fast_mode: false,
            per_session_opt_in: false,
            user_fast_mode_setting: None,
        }
    }
}

/// Thread-safe runtime state manager for fast mode.
///
/// Tracks the actual operational state: whether we're actively sending
/// fast mode requests or in cooldown after a rate limit.
#[derive(Debug, Clone)]
pub struct FastModeRuntime {
    inner: Arc<Mutex<FastModeRuntimeInner>>,
}

#[derive(Debug)]
struct FastModeRuntimeInner {
    state: FastModeState,
    org_status: OrgFastModeStatus,
}

impl Default for FastModeRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FastModeRuntimeInner {
                state: FastModeState::Available,
                org_status: OrgFastModeStatus::Pending,
            })),
        }
    }
}

fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("fast mode mutex was poisoned; recovering");
            poisoned.into_inner()
        }
    }
}

impl FastModeRuntime {
    /// Create a new runtime state manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current fast mode state.
    ///
    /// Automatically transitions from Cooldown to Available if the
    /// cooldown period has expired.
    pub fn state(&self) -> FastModeState {
        let mut inner = lock_or_recover(&self.inner);
        if let FastModeState::Cooldown { until, .. } = &inner.state
            && *until <= Utc::now()
        {
            tracing::debug!("Fast mode cooldown expired, re-enabling fast mode");
            inner.state = FastModeState::Available;
        }
        inner.state.clone()
    }

    /// Trigger a fast mode cooldown period.
    ///
    /// Called when a rate limit or overloaded response is received.
    pub fn trigger_cooldown(&self, duration: Duration, reason: CooldownReason) {
        let chrono_duration = chrono::Duration::from_std(duration).unwrap_or_else(|e| {
            tracing::error!("invalid cooldown duration: {e}; using 60s fallback");
            chrono::Duration::seconds(60)
        });
        let until = Utc::now() + chrono_duration;
        let cooldown_secs = duration.as_secs();
        tracing::debug!("Fast mode cooldown triggered ({reason:?}), duration {cooldown_secs}s");
        let mut inner = lock_or_recover(&self.inner);
        inner.state = FastModeState::Cooldown { until, reason };
    }

    /// Clear any active cooldown, returning to Available state.
    pub fn clear_cooldown(&self) {
        let mut inner = lock_or_recover(&self.inner);
        inner.state = FastModeState::Available;
    }

    /// Check if currently in cooldown.
    pub fn is_in_cooldown(&self) -> bool {
        matches!(self.state(), FastModeState::Cooldown { .. })
    }

    /// Get the organization-level fast mode status.
    pub fn org_status(&self) -> OrgFastModeStatus {
        let inner = lock_or_recover(&self.inner);
        inner.org_status.clone()
    }

    /// Set the organization-level fast mode status.
    pub fn set_org_status(&self, status: OrgFastModeStatus) {
        let mut inner = lock_or_recover(&self.inner);
        inner.org_status = status;
    }

    /// Handle an API rejection of fast mode.
    ///
    /// Called when the API returns an error indicating fast mode is not
    /// enabled for the organization. Permanently disables fast mode.
    pub fn handle_api_rejection(&self) {
        let mut inner = lock_or_recover(&self.inner);
        if let OrgFastModeStatus::Disabled { .. } = &inner.org_status {
            return; // Already disabled
        }
        inner.org_status = OrgFastModeStatus::Disabled {
            reason: FastModeDisabledReason::Preference,
        };
        tracing::debug!("Fast mode permanently disabled by API rejection");
    }

    /// Resolve org status from a cached value without making API calls.
    ///
    /// Equivalent to `resolveFastModeStatusFromCache()` in fastMode.ts.
    pub fn resolve_from_cache(&self, is_ant_user: bool, cached_enabled: bool) {
        let mut inner = lock_or_recover(&self.inner);
        if !matches!(inner.org_status, OrgFastModeStatus::Pending) {
            return;
        }
        inner.org_status = if is_ant_user || cached_enabled {
            OrgFastModeStatus::Enabled
        } else {
            OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::Unknown,
            }
        };
    }
}

// ── Pure functions ──────────────────────────────────────────────────────

/// Check if fast mode is supported by a specific model.
///
/// Equivalent to `isFastModeSupportedByModel()` in fastMode.ts.
/// Currently only Opus 4.6 supports fast mode.
pub fn is_fast_mode_supported_by_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("opus-4-6")
}

/// Determine the initial fast mode setting for a session.
///
/// Equivalent to `getInitialFastModeSetting()` in fastMode.ts.
///
/// Returns `false` if any of:
/// - Fast mode is globally disabled
/// - Fast mode is not available (org disabled, not first-party, etc.)
/// - The model doesn't support fast mode
/// - Per-session opt-in is required
/// - User's setting is not explicitly true
pub fn get_initial_fast_mode_setting(
    model: &str,
    config: &FastModeConfig,
    org_status: &OrgFastModeStatus,
) -> bool {
    if !config.enabled {
        return false;
    }
    if !is_fast_mode_available(config, org_status) {
        return false;
    }
    if !is_fast_mode_supported_by_model(model) {
        return false;
    }
    if config.per_session_opt_in {
        return false;
    }
    config.user_fast_mode_setting == Some(true)
}

/// Check if fast mode is available given the current configuration.
///
/// Equivalent to `isFastModeAvailable()` in fastMode.ts.
pub fn is_fast_mode_available(config: &FastModeConfig, org_status: &OrgFastModeStatus) -> bool {
    if !config.enabled {
        return false;
    }

    // Not available in SDK mode unless opted in
    if config.is_non_interactive_sdk && !config.kairos_active && !config.flag_fast_mode {
        return false;
    }

    // Only available for 1P
    if !config.is_first_party {
        return false;
    }

    // Check org status
    match org_status {
        OrgFastModeStatus::Pending | OrgFastModeStatus::Enabled => true,
        OrgFastModeStatus::Disabled { .. } => false,
    }
}

/// Get the fast mode model identifier.
///
/// Equivalent to `getFastModeModel()` in fastMode.ts.
pub fn get_fast_mode_model(enable_1m: bool) -> &'static str {
    if enable_1m { "opus[1m]" } else { "opus" }
}

/// Get the simplified fast mode state for display.
///
/// Equivalent to `getFastModeState()` in fastMode.ts.
pub fn get_fast_mode_simple_state(
    model: &str,
    fast_mode_user_enabled: bool,
    config: &FastModeConfig,
    org_status: &OrgFastModeStatus,
    runtime: &FastModeRuntime,
) -> FastModeSimpleState {
    let enabled = config.enabled
        && is_fast_mode_available(config, org_status)
        && fast_mode_user_enabled
        && is_fast_mode_supported_by_model(model);

    if enabled && runtime.is_in_cooldown() {
        return FastModeSimpleState::Cooldown;
    }
    if enabled {
        return FastModeSimpleState::On;
    }
    FastModeSimpleState::Off
}

/// Get a human-readable message for a fast mode disabled reason.
///
/// Equivalent to `getDisabledReasonMessage()` in fastMode.ts.
pub fn get_disabled_reason_message(
    reason: &FastModeDisabledReason,
    is_oauth: bool,
) -> &'static str {
    match reason {
        FastModeDisabledReason::Free => {
            if is_oauth {
                "Fast mode requires a paid subscription"
            } else {
                "Fast mode unavailable during evaluation. Please purchase credits."
            }
        }
        FastModeDisabledReason::Preference => "Fast mode has been disabled by your organization",
        FastModeDisabledReason::ExtraUsageDisabled => {
            "Fast mode requires extra usage billing · /extra-usage to enable"
        }
        FastModeDisabledReason::NetworkError => {
            "Fast mode unavailable due to network connectivity issues"
        }
        FastModeDisabledReason::Unknown => "Fast mode is currently unavailable",
    }
}

/// Get a message for overage-related fast mode disabling.
///
/// Equivalent to `getOverageDisabledMessage()` in fastMode.ts.
pub fn get_overage_disabled_message(reason: Option<&str>) -> &'static str {
    match reason {
        Some("out_of_credits") => "Fast mode disabled · extra usage credits exhausted",
        Some("org_level_disabled" | "org_service_level_disabled") => {
            "Fast mode disabled · extra usage disabled by your organization"
        }
        Some("org_level_disabled_until") => "Fast mode disabled · extra usage spending cap reached",
        Some("member_level_disabled") => {
            "Fast mode disabled · extra usage disabled for your account"
        }
        Some(
            "seat_tier_level_disabled" | "seat_tier_zero_credit_limit" | "member_zero_credit_limit",
        ) => "Fast mode disabled · extra usage not available for your plan",
        Some("overage_not_provisioned" | "no_limits_configured") => {
            "Fast mode requires extra usage billing · /extra-usage to enable"
        }
        _ => "Fast mode disabled · extra usage not available",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_fast_mode_supported_by_model() {
        assert!(is_fast_mode_supported_by_model("claude-opus-4-6"));
        assert!(!is_fast_mode_supported_by_model("claude-sonnet-4-6"));
        assert!(!is_fast_mode_supported_by_model("claude-haiku-4-5"));
    }

    #[test]
    fn test_get_initial_fast_mode_setting_disabled() {
        let config = FastModeConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!get_initial_fast_mode_setting(
            "claude-opus-4-6",
            &config,
            &OrgFastModeStatus::Enabled
        ));
    }

    #[test]
    fn test_get_initial_fast_mode_setting_enabled() {
        let config = FastModeConfig {
            user_fast_mode_setting: Some(true),
            ..Default::default()
        };
        assert!(get_initial_fast_mode_setting(
            "claude-opus-4-6",
            &config,
            &OrgFastModeStatus::Enabled
        ));
    }

    #[test]
    fn test_get_initial_fast_mode_setting_wrong_model() {
        let config = FastModeConfig {
            user_fast_mode_setting: Some(true),
            ..Default::default()
        };
        assert!(!get_initial_fast_mode_setting(
            "claude-sonnet-4-6",
            &config,
            &OrgFastModeStatus::Enabled
        ));
    }

    #[test]
    fn test_fast_mode_runtime_cooldown() {
        let runtime = FastModeRuntime::new();
        assert!(matches!(runtime.state(), FastModeState::Available));
        assert!(!runtime.is_in_cooldown());

        runtime.trigger_cooldown(Duration::from_secs(60), CooldownReason::RateLimit);
        assert!(runtime.is_in_cooldown());

        runtime.clear_cooldown();
        assert!(!runtime.is_in_cooldown());
    }

    #[test]
    fn test_fast_mode_runtime_api_rejection() {
        let runtime = FastModeRuntime::new();
        runtime.handle_api_rejection();

        let status = runtime.org_status();
        assert!(matches!(
            status,
            OrgFastModeStatus::Disabled {
                reason: FastModeDisabledReason::Preference
            }
        ));
    }

    #[test]
    fn test_fast_mode_runtime_resolve_from_cache() {
        let runtime = FastModeRuntime::new();
        runtime.resolve_from_cache(true, false);
        assert!(matches!(runtime.org_status(), OrgFastModeStatus::Enabled));
    }

    #[test]
    fn test_get_disabled_reason_message() {
        assert_eq!(
            get_disabled_reason_message(&FastModeDisabledReason::Free, true),
            "Fast mode requires a paid subscription"
        );
        assert_eq!(
            get_disabled_reason_message(&FastModeDisabledReason::Free, false),
            "Fast mode unavailable during evaluation. Please purchase credits."
        );
    }

    #[test]
    fn test_get_overage_disabled_message() {
        assert_eq!(
            get_overage_disabled_message(Some("out_of_credits")),
            "Fast mode disabled · extra usage credits exhausted"
        );
        assert_eq!(
            get_overage_disabled_message(None),
            "Fast mode disabled · extra usage not available"
        );
    }

    #[test]
    fn test_get_fast_mode_simple_state() {
        let config = FastModeConfig {
            user_fast_mode_setting: Some(true),
            ..Default::default()
        };
        let runtime = FastModeRuntime::new();
        let state = get_fast_mode_simple_state(
            "claude-opus-4-6",
            true,
            &config,
            &OrgFastModeStatus::Enabled,
            &runtime,
        );
        assert_eq!(state, FastModeSimpleState::On);
    }

    #[test]
    fn test_get_fast_mode_model() {
        assert_eq!(get_fast_mode_model(false), "opus");
        assert_eq!(get_fast_mode_model(true), "opus[1m]");
    }
}
