//! 1M context-window access checks.
//!
//! Determines whether a user / subscription tier is permitted to use the
//! extended 1M-token context window for models that support it.

use crate::capabilities::model_supports_1m;

/// Subscription tier for the current user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionTier {
    /// Free / anonymous API user.
    Free,
    /// Claude.ai Pro subscriber.
    Pro,
    /// Claude.ai Max subscriber.
    Max,
    /// Claude.ai Team (standard) subscriber.
    TeamStandard,
    /// Claude.ai Team Premium subscriber.
    TeamPremium,
    /// Enterprise customer.
    Enterprise,
    /// Pay-as-you-go API user (first-party or third-party).
    PayAsYouGo,
}

/// Extra-usage state cached from the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraUsageState {
    /// State not yet fetched from the API — treat conservatively.
    Unknown,
    /// No disabled reason — extra usage is provisioned.
    Enabled,
    /// Provisioned but credits depleted — still counts as enabled.
    OutOfCredits,
    /// Not provisioned or actively disabled.
    Disabled,
}

/// Context for 1M access checks.
#[derive(Debug, Clone)]
pub struct OneMContext {
    /// Whether the 1M feature is globally disabled (e.g. via feature flag).
    pub is_1m_disabled: bool,
    /// The user's subscription tier.
    pub subscription: SubscriptionTier,
    /// Cached extra-usage state from the control plane.
    pub extra_usage: ExtraUsageState,
}

impl Default for OneMContext {
    fn default() -> Self {
        Self {
            is_1m_disabled: false,
            subscription: SubscriptionTier::PayAsYouGo,
            extra_usage: ExtraUsageState::Unknown,
        }
    }
}

/// Returns `true` when the user is allowed to use the 1M context window on
/// the given model.
///
/// Checks:
/// 1. The model itself must support 1M context.
/// 2. The 1M feature must not be globally disabled.
/// 3. The subscription tier must have access.
pub fn has_1m_access(model_id: &str, ctx: &OneMContext) -> bool {
    // Model must support 1M.
    if !model_supports_1m(model_id) {
        return false;
    }

    // Feature must not be globally disabled.
    if ctx.is_1m_disabled {
        return false;
    }

    // Subscription-based access check.
    match ctx.subscription {
        // PAYG / API users always have access.
        SubscriptionTier::PayAsYouGo | SubscriptionTier::Free => true,
        // Subscribers need extra-usage to be provisioned.
        SubscriptionTier::Pro
        | SubscriptionTier::Max
        | SubscriptionTier::TeamStandard
        | SubscriptionTier::TeamPremium
        | SubscriptionTier::Enterprise => {
            matches!(
                ctx.extra_usage,
                ExtraUsageState::Enabled | ExtraUsageState::OutOfCredits
            )
        }
    }
}

/// Returns `true` when the given model string contains the `[1m]` tag.
pub fn has_1m_tag(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.ends_with("[1m]")
}

/// Strip the `[1m]` tag from a model string.
pub fn strip_1m_tag(model: &str) -> &str {
    if has_1m_tag(model) {
        &model[..model.len() - 4]
    } else {
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payg_has_1m_access() {
        let ctx = OneMContext {
            subscription: SubscriptionTier::PayAsYouGo,
            ..Default::default()
        };
        assert!(has_1m_access("claude-opus-4-7", &ctx));
    }

    #[test]
    fn model_without_1m_support() {
        let ctx = OneMContext {
            subscription: SubscriptionTier::PayAsYouGo,
            ..Default::default()
        };
        assert!(!has_1m_access("claude-haiku-4-5-20251001", &ctx));
    }

    #[test]
    fn disabled_feature_blocks_access() {
        let ctx = OneMContext {
            is_1m_disabled: true,
            subscription: SubscriptionTier::PayAsYouGo,
            ..Default::default()
        };
        assert!(!has_1m_access("claude-opus-4-7", &ctx));
    }

    #[test]
    fn subscriber_needs_extra_usage() {
        let ctx = OneMContext {
            subscription: SubscriptionTier::Pro,
            extra_usage: ExtraUsageState::Enabled,
            ..Default::default()
        };
        assert!(has_1m_access("claude-opus-4-7", &ctx));

        let ctx_disabled = OneMContext {
            subscription: SubscriptionTier::Pro,
            extra_usage: ExtraUsageState::Disabled,
            ..Default::default()
        };
        assert!(!has_1m_access("claude-opus-4-7", &ctx_disabled));
    }

    #[test]
    fn out_of_credits_still_enabled() {
        let ctx = OneMContext {
            subscription: SubscriptionTier::Max,
            extra_usage: ExtraUsageState::OutOfCredits,
            ..Default::default()
        };
        assert!(has_1m_access("claude-opus-4-7", &ctx));
    }

    #[test]
    fn tag_detection() {
        assert!(has_1m_tag("claude-opus-4-7[1m]"));
        assert!(has_1m_tag("claude-opus-4-7[1M]"));
        assert!(!has_1m_tag("claude-opus-4-7"));
    }

    #[test]
    fn tag_stripping() {
        assert_eq!(strip_1m_tag("claude-opus-4-7[1m]"), "claude-opus-4-7");
        assert_eq!(strip_1m_tag("claude-opus-4-7"), "claude-opus-4-7");
    }
}
