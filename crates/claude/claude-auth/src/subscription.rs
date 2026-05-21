//! Subscription tier definitions and utilities.
//!
//! Mirrors the subscription types from the OAuth profile response and
//! `utils/auth.ts` subscription handling.

use serde::{Deserialize, Serialize};

/// Subscription tier for a user's account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionTier {
    /// Free tier with limited usage.
    #[default]
    Free,
    /// Pro subscription.
    Pro,
    /// Team plan.
    Team,
    /// Enterprise plan.
    Enterprise,
    /// Claude Max subscription.
    Max,
}

impl SubscriptionTier {
    /// Parse from the `organization_type` string returned by the profile endpoint.
    pub fn from_org_type(org_type: &str) -> Option<Self> {
        match org_type {
            "claude_pro" => Some(Self::Pro),
            "claude_team" => Some(Self::Team),
            "claude_enterprise" => Some(Self::Enterprise),
            "claude_max" => Some(Self::Max),
            _ => None,
        }
    }

    /// Whether this tier requires a paid subscription.
    pub fn is_paid(&self) -> bool {
        !matches!(self, Self::Free)
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Pro => "Pro",
            Self::Team => "Team",
            Self::Enterprise => "Enterprise",
            Self::Max => "Max",
        }
    }
}

/// Rate-limit tier associated with a subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitTier {
    #[default]
    Standard,
    Elevated,
    High,
}

impl RateLimitTier {
    /// Parse from a rate-limit tier string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "standard" => Some(Self::Standard),
            "elevated" => Some(Self::Elevated),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// Billing type for a subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingType {
    #[default]
    Subscription,
    Usage,
    Hybrid,
}

impl BillingType {
    /// Parse from a billing type string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "subscription" => Some(Self::Subscription),
            "usage" => Some(Self::Usage),
            "hybrid" => Some(Self::Hybrid),
            _ => None,
        }
    }
}

/// Resolved subscription information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    /// The subscription tier.
    pub tier: SubscriptionTier,
    /// Rate-limit tier.
    pub rate_limit_tier: RateLimitTier,
    /// Billing type.
    pub billing_type: BillingType,
    /// Whether extra usage (overage) is enabled.
    pub has_extra_usage_enabled: bool,
    /// Display name of the account.
    pub display_name: Option<String>,
    /// Account creation timestamp.
    pub account_created_at: Option<String>,
    /// Subscription creation timestamp.
    pub subscription_created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_org_type() {
        assert_eq!(
            SubscriptionTier::from_org_type("claude_pro"),
            Some(SubscriptionTier::Pro)
        );
        assert_eq!(
            SubscriptionTier::from_org_type("claude_max"),
            Some(SubscriptionTier::Max)
        );
        assert_eq!(SubscriptionTier::from_org_type("unknown"), None);
    }

    #[test]
    fn is_paid() {
        assert!(!SubscriptionTier::Free.is_paid());
        assert!(SubscriptionTier::Pro.is_paid());
        assert!(SubscriptionTier::Max.is_paid());
    }

    #[test]
    fn label() {
        assert_eq!(SubscriptionTier::Free.label(), "Free");
        assert_eq!(SubscriptionTier::Enterprise.label(), "Enterprise");
    }
}
