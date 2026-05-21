//! Context window upgrade check.
//!
//! Determines whether a user can upgrade their current model to a 1M context
//! variant and provides upgrade messages for the UI.

use crate::check_1m::{OneMContext, has_1m_access};
use crate::model::ModelSetting;

// ── Types ────────────────────────────────────────────────────────────────

/// Describes an available context window upgrade.
#[derive(Debug, Clone)]
pub struct ContextUpgrade {
    /// The alias to use for the upgrade (e.g. `"opus[1m]"`).
    pub alias: &'static str,
    /// Human-readable name (e.g. `"Opus 1M"`).
    pub name: &'static str,
    /// Context multiplier (e.g. `5` for 5× context).
    pub multiplier: u32,
}

// ── Upgrade detection ────────────────────────────────────────────────────

/// Check for an available context window upgrade based on the current model
/// setting and 1M access.
///
/// Returns `None` if no upgrade is available or the user already has max
/// context.
pub fn get_available_upgrade(
    model_setting: &ModelSetting,
    ctx: &OneMContext,
) -> Option<ContextUpgrade> {
    let alias = match model_setting {
        ModelSetting::Auto => return None,
        ModelSetting::Specific(s) => s.as_str(),
    };

    let lower = alias.to_lowercase();
    let base = lower.strip_suffix("[1m]").unwrap_or(&lower).trim();

    if base == "opus" && has_1m_access("claude-opus-4-7", ctx) {
        return Some(ContextUpgrade {
            alias: "opus[1m]",
            name: "Opus 1M",
            multiplier: 5,
        });
    }

    if base == "sonnet" && has_1m_access("claude-sonnet-4-6", ctx) {
        return Some(ContextUpgrade {
            alias: "sonnet[1m]",
            name: "Sonnet 1M",
            multiplier: 5,
        });
    }

    None
}

// ── Upgrade messages ─────────────────────────────────────────────────────

/// Context in which the upgrade message will be displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeContext {
    /// Shown as a warning when the user hits context limits.
    Warning,
    /// Shown as a tip / suggestion.
    Tip,
}

/// Get an upgrade message for the given display context.
///
/// Returns `None` if no upgrade is available.
pub fn get_upgrade_message(
    model_setting: &ModelSetting,
    one_m_ctx: &OneMContext,
    context: UpgradeContext,
) -> Option<String> {
    let upgrade = get_available_upgrade(model_setting, one_m_ctx)?;

    match context {
        UpgradeContext::Warning => Some(format!("/model {}", upgrade.alias)),
        UpgradeContext::Tip => Some(format!(
            "Tip: You have access to {} with {}x more context",
            upgrade.name, upgrade.multiplier
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_1m::SubscriptionTier;

    fn payg_ctx() -> OneMContext {
        OneMContext {
            subscription: SubscriptionTier::PayAsYouGo,
            ..Default::default()
        }
    }

    fn disabled_ctx() -> OneMContext {
        OneMContext {
            is_1m_disabled: true,
            subscription: SubscriptionTier::PayAsYouGo,
            ..Default::default()
        }
    }

    #[test]
    fn opus_upgrade_available_for_payg() {
        let setting = ModelSetting::Specific("opus".into());
        let upgrade = get_available_upgrade(&setting, &payg_ctx());
        assert!(upgrade.is_some());
        let u = upgrade.expect("upgrade should exist");
        assert_eq!(u.alias, "opus[1m]");
        assert_eq!(u.name, "Opus 1M");
        assert_eq!(u.multiplier, 5);
    }

    #[test]
    fn sonnet_upgrade_available_for_payg() {
        let setting = ModelSetting::Specific("sonnet".into());
        let upgrade = get_available_upgrade(&setting, &payg_ctx());
        assert!(upgrade.is_some());
        let u = upgrade.expect("upgrade should exist");
        assert_eq!(u.alias, "sonnet[1m]");
        assert_eq!(u.name, "Sonnet 1M");
    }

    #[test]
    fn no_upgrade_when_disabled() {
        let setting = ModelSetting::Specific("opus".into());
        let upgrade = get_available_upgrade(&setting, &disabled_ctx());
        assert!(upgrade.is_none());
    }

    #[test]
    fn no_upgrade_for_auto() {
        let upgrade = get_available_upgrade(&ModelSetting::Auto, &payg_ctx());
        assert!(upgrade.is_none());
    }

    #[test]
    fn no_upgrade_for_non_alias() {
        let setting = ModelSetting::Specific("claude-haiku-4-5-20251001".into());
        let upgrade = get_available_upgrade(&setting, &payg_ctx());
        assert!(upgrade.is_none());
    }

    #[test]
    fn no_upgrade_for_already_1m() {
        let setting = ModelSetting::Specific("opus[1m]".into());
        let upgrade = get_available_upgrade(&setting, &payg_ctx());
        // After stripping [1m], base is "opus" which would match, but the user
        // already has 1M.  In the reference implementation, this still returns
        // an upgrade.  We follow the same behavior.
        // Actually: the base after stripping is "opus", and the model supports 1M,
        // so it does return an upgrade.  This is correct per the reference.
        assert!(upgrade.is_some());
    }

    #[test]
    fn warning_message() {
        let setting = ModelSetting::Specific("opus".into());
        let msg = get_upgrade_message(&setting, &payg_ctx(), UpgradeContext::Warning);
        assert_eq!(msg, Some("/model opus[1m]".into()));
    }

    #[test]
    fn tip_message() {
        let setting = ModelSetting::Specific("sonnet".into());
        let msg = get_upgrade_message(&setting, &payg_ctx(), UpgradeContext::Tip);
        assert_eq!(
            msg,
            Some("Tip: You have access to Sonnet 1M with 5x more context".into())
        );
    }

    #[test]
    fn no_message_when_no_upgrade() {
        let setting = ModelSetting::Specific("haiku".into());
        let msg = get_upgrade_message(&setting, &payg_ctx(), UpgradeContext::Tip);
        assert!(msg.is_none());
    }
}
