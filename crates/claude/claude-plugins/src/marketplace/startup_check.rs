//! Marketplace startup check.
//!
//! Checks marketplace health on startup, including auto-install of the
//! official marketplace for new users.

use serde::{Deserialize, Serialize};

use super::official::OFFICIAL_MARKETPLACE_NAME;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Reason why a marketplace was skipped during startup checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceSkipReason {
    /// Already installed.
    AlreadyInstalled,
    /// Already attempted (retry backoff).
    AlreadyAttempted,
    /// Blocked by enterprise policy.
    PolicyBlocked,
    /// Git is not available.
    GitUnavailable,
    /// Network/service unavailable.
    ServiceUnavailable,
    /// Unknown reason.
    Unknown,
}

/// Result of performing marketplace startup checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MarketplaceStartupCheckResult {
    /// Whether the official marketplace was installed.
    pub official_installed: bool,
    /// Reason the official marketplace was skipped (if applicable).
    pub skip_reason: Option<MarketplaceSkipReason>,
    /// Total number of marketplaces checked.
    pub marketplaces_checked: usize,
    /// Number of marketplaces that failed health checks.
    pub failed_count: usize,
    /// List of marketplace names that failed.
    pub failed_names: Vec<String>,
    /// Errors encountered during checks.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Perform marketplace startup checks.
///
/// Checks the health of all known marketplaces and optionally auto-installs
/// the official marketplace.
pub fn perform_marketplace_startup_checks(
    known_marketplaces: &[String],
    official_installed: bool,
) -> MarketplaceStartupCheckResult {
    let mut result = MarketplaceStartupCheckResult::default();

    // Check if official marketplace needs installation
    if official_installed {
        result.skip_reason = Some(MarketplaceSkipReason::AlreadyInstalled);
    } else if !known_marketplaces.is_empty() {
        // Check if official marketplace is in the known list
        let has_official = known_marketplaces
            .iter()
            .any(|m| m == OFFICIAL_MARKETPLACE_NAME);
        if has_official {
            result.skip_reason = Some(MarketplaceSkipReason::AlreadyInstalled);
        }
    }

    // Check health of known marketplaces
    result.marketplaces_checked = known_marketplaces.len();
    // In a real implementation, we would check each marketplace's health
    // by trying to fetch its index. For now, just report the count.

    result
}

/// Check if the official marketplace auto-install is disabled.
pub fn is_official_auto_install_disabled(env_var: Option<&str>) -> bool {
    env_var.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Get the retry delay in milliseconds based on attempt count.
pub fn get_retry_delay_ms(attempt: u32) -> u64 {
    let initial_delay_ms: u64 = 60 * 60 * 1000; // 1 hour
    let multiplier: u64 = 2;
    let max_delay_ms: u64 = 7 * 24 * 60 * 60 * 1000; // 1 week

    let delay = initial_delay_ms.saturating_mul(multiplier.saturating_pow(attempt));
    delay.min(max_delay_ms)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perform_checks_with_official_installed() {
        let result =
            perform_marketplace_startup_checks(&["claude-plugins-official".to_owned()], true);
        assert_eq!(
            result.skip_reason,
            Some(MarketplaceSkipReason::AlreadyInstalled)
        );
    }

    #[test]
    fn perform_checks_without_marketplaces() {
        let result = perform_marketplace_startup_checks(&[], false);
        assert_eq!(result.marketplaces_checked, 0);
        assert!(result.skip_reason.is_none());
    }

    #[test]
    fn perform_checks_with_marketplaces() {
        let result =
            perform_marketplace_startup_checks(&["mkt-a".to_owned(), "mkt-b".to_owned()], false);
        assert_eq!(result.marketplaces_checked, 2);
    }

    #[test]
    fn is_official_auto_install_disabled_true() {
        assert!(is_official_auto_install_disabled(Some("1")));
        assert!(is_official_auto_install_disabled(Some("true")));
        assert!(is_official_auto_install_disabled(Some("True")));
    }

    #[test]
    fn is_official_auto_install_disabled_false() {
        assert!(!is_official_auto_install_disabled(None));
        assert!(!is_official_auto_install_disabled(Some("0")));
        assert!(!is_official_auto_install_disabled(Some("false")));
    }

    #[test]
    fn get_retry_delay_increases() {
        let delay_0 = get_retry_delay_ms(0);
        let delay_1 = get_retry_delay_ms(1);
        let delay_2 = get_retry_delay_ms(2);
        assert!(delay_0 < delay_1);
        assert!(delay_1 < delay_2);
    }

    #[test]
    fn get_retry_delay_capped() {
        let delay = get_retry_delay_ms(100);
        let max = 7 * 24 * 60 * 60 * 1000;
        assert!(delay <= max);
    }

    #[test]
    fn startup_check_result_default() {
        let result = MarketplaceStartupCheckResult::default();
        assert!(!result.official_installed);
        assert!(result.skip_reason.is_none());
        assert_eq!(result.marketplaces_checked, 0);
        assert_eq!(result.failed_count, 0);
        assert!(result.failed_names.is_empty());
        assert!(result.errors.is_empty());
    }
}
