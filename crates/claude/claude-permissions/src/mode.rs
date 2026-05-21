//! Permission mode definitions and configuration.
//!
//! Corresponds to `src/utils/permissions/PermissionMode.ts` (142 lines).
//! Supports 7 permission modes: default, plan, acceptEdits, bypassPermissions,
//! dontAsk, auto, bubble.

use claude_core::PermissionMode;
use serde::{Deserialize, Serialize};

/// Extended permission mode including internal-only modes.
/// The base [`PermissionMode`] in rc-core covers the 5 external modes;
/// this adds `Auto` and `Bubble` for internal use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ExtendedPermissionMode {
    Default,
    Plan,
    AcceptEdits,
    BypassPermissions,
    DontAsk,
    /// Auto mode — classifier-based automatic approval (internal only).
    Auto,
    /// Bubble mode — permission decisions bubble up to parent (internal only).
    Bubble,
}

impl From<PermissionMode> for ExtendedPermissionMode {
    fn from(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Default => Self::Default,
            PermissionMode::Plan => Self::Plan,
            PermissionMode::AcceptEdits => Self::AcceptEdits,
            PermissionMode::BypassPermissions => Self::BypassPermissions,
            PermissionMode::DontAsk => Self::DontAsk,
            PermissionMode::Auto => Self::Auto,
        }
    }
}

impl ExtendedPermissionMode {
    /// Convert to the legacy string representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::AcceptEdits => "acceptEdits",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Auto => "auto",
            Self::Bubble => "bubble",
        }
    }

    /// Try to parse from string.
    #[must_use]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "plan" => Some(Self::Plan),
            "acceptEdits" => Some(Self::AcceptEdits),
            "bypassPermissions" => Some(Self::BypassPermissions),
            "dontAsk" => Some(Self::DontAsk),
            "auto" => Some(Self::Auto),
            "bubble" => Some(Self::Bubble),
            _ => None,
        }
    }

    /// Whether this mode is externally user-addressable.
    #[must_use]
    pub fn is_external(self) -> bool {
        !matches!(self, Self::Auto | Self::Bubble)
    }

    /// Human-readable title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Plan => "Plan Mode",
            Self::AcceptEdits => "Accept edits",
            Self::BypassPermissions => "Bypass Permissions",
            Self::DontAsk => "Don't Ask",
            Self::Auto => "Auto mode",
            Self::Bubble => "Bubble",
        }
    }

    /// Short title for display.
    #[must_use]
    pub fn short_title(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Plan => "Plan",
            Self::AcceptEdits => "Accept",
            Self::BypassPermissions => "Bypass",
            Self::DontAsk => "DontAsk",
            Self::Auto => "Auto",
            Self::Bubble => "Bubble",
        }
    }

    /// Symbol for TUI display.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Plan => "⏸",
            Self::AcceptEdits | Self::BypassPermissions | Self::DontAsk | Self::Auto => "⏵⏵",
            Self::Bubble => "↗",
        }
    }

    /// All external permission modes.
    pub const EXTERNAL_MODES: &[ExtendedPermissionMode] = &[
        Self::Default,
        Self::Plan,
        Self::AcceptEdits,
        Self::BypassPermissions,
        Self::DontAsk,
    ];

    /// All internal permission modes (external + auto + bubble).
    pub const ALL_MODES: &[ExtendedPermissionMode] = &[
        Self::Default,
        Self::Plan,
        Self::AcceptEdits,
        Self::BypassPermissions,
        Self::DontAsk,
        Self::Auto,
        Self::Bubble,
    ];
}

/// Configuration for a permission mode's display properties.
#[derive(Debug, Clone)]
pub struct PermissionModeConfig {
    pub title: &'static str,
    pub short_title: &'static str,
    pub symbol: &'static str,
    pub color_key: ModeColorKey,
}

/// Color key for permission mode display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeColorKey {
    Text,
    PlanMode,
    Permission,
    AutoAccept,
    Error,
    Warning,
}

impl ExtendedPermissionMode {
    /// Get the display configuration for this mode.
    #[must_use]
    pub fn config(self) -> PermissionModeConfig {
        match self {
            Self::Default => PermissionModeConfig {
                title: "Default",
                short_title: "Default",
                symbol: "",
                color_key: ModeColorKey::Text,
            },
            Self::Plan => PermissionModeConfig {
                title: "Plan Mode",
                short_title: "Plan",
                symbol: "⏸",
                color_key: ModeColorKey::PlanMode,
            },
            Self::AcceptEdits => PermissionModeConfig {
                title: "Accept edits",
                short_title: "Accept",
                symbol: "⏵⏵",
                color_key: ModeColorKey::AutoAccept,
            },
            Self::BypassPermissions => PermissionModeConfig {
                title: "Bypass Permissions",
                short_title: "Bypass",
                symbol: "⏵⏵",
                color_key: ModeColorKey::Error,
            },
            Self::DontAsk => PermissionModeConfig {
                title: "Don't Ask",
                short_title: "DontAsk",
                symbol: "⏵⏵",
                color_key: ModeColorKey::Error,
            },
            Self::Auto => PermissionModeConfig {
                title: "Auto mode",
                short_title: "Auto",
                symbol: "⏵⏵",
                color_key: ModeColorKey::Warning,
            },
            Self::Bubble => PermissionModeConfig {
                title: "Bubble",
                short_title: "Bubble",
                symbol: "↗",
                color_key: ModeColorKey::Permission,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_mode_from_base() {
        assert_eq!(
            ExtendedPermissionMode::from(PermissionMode::Default),
            ExtendedPermissionMode::Default
        );
        assert_eq!(
            ExtendedPermissionMode::from(PermissionMode::BypassPermissions),
            ExtendedPermissionMode::BypassPermissions
        );
    }

    #[test]
    fn round_trip_str() {
        for mode in ExtendedPermissionMode::ALL_MODES {
            assert_eq!(
                ExtendedPermissionMode::from_str_opt(mode.as_str()),
                Some(*mode)
            );
        }
    }

    #[test]
    fn external_modes_exclude_auto_bubble() {
        for mode in ExtendedPermissionMode::EXTERNAL_MODES {
            assert!(mode.is_external());
        }
        assert!(!ExtendedPermissionMode::Auto.is_external());
        assert!(!ExtendedPermissionMode::Bubble.is_external());
    }

    #[test]
    fn config_has_consistent_titles() {
        for mode in ExtendedPermissionMode::ALL_MODES {
            let cfg = mode.config();
            assert!(!cfg.title.is_empty());
            assert!(!cfg.short_title.is_empty());
        }
    }
}
