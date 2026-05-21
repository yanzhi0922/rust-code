//! Permission initialization setup.
//!
//! Corresponds to `src/utils/permissions/permissionSetup.ts`.
//! Initializes the permission system with default rules and mode.

use claude_core::permission_types::{PermissionBehavior, PermissionRuleSource};

use crate::handler::{InteractiveHandler, PermissionHandler};
use crate::mode::ExtendedPermissionMode;
use crate::rule::PermissionRuleV2;

/// Configuration for permission system initialization.
#[derive(Debug, Clone)]
pub struct PermissionSetupConfig {
    /// The default permission mode.
    pub mode: ExtendedPermissionMode,
    /// Whether to auto-accept edits.
    pub auto_accept_edits: bool,
    /// Additional allowed directories.
    pub additional_directories: Vec<String>,
    /// Whether to disable bypass permissions mode.
    pub disable_bypass: bool,
    /// Whether to disable auto mode.
    pub disable_auto: bool,
}

impl Default for PermissionSetupConfig {
    fn default() -> Self {
        Self {
            mode: ExtendedPermissionMode::Default,
            auto_accept_edits: false,
            additional_directories: Vec::new(),
            disable_bypass: false,
            disable_auto: false,
        }
    }
}

/// Result of permission setup.
pub struct PermissionSetup {
    /// The configured permission mode.
    pub mode: ExtendedPermissionMode,
    /// The handler to use for permission checks.
    pub handler: Box<dyn PermissionHandler>,
    /// Default rules loaded from settings.
    pub default_rules: Vec<PermissionRuleV2>,
    /// Whether bypass is disabled.
    pub bypass_disabled: bool,
}

impl std::fmt::Debug for PermissionSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionSetup")
            .field("mode", &self.mode)
            .field("handler", &"Box<dyn PermissionHandler>")
            .field("default_rules", &self.default_rules.len())
            .field("bypass_disabled", &self.bypass_disabled)
            .finish()
    }
}

impl PermissionSetup {
    /// Initialize the permission system with the given configuration.
    pub fn initialize(config: &PermissionSetupConfig) -> Self {
        let mode = if (config.disable_bypass
            && config.mode == ExtendedPermissionMode::BypassPermissions)
            || (config.disable_auto && config.mode == ExtendedPermissionMode::Auto)
        {
            ExtendedPermissionMode::Default
        } else {
            config.mode
        };

        let handler: Box<dyn PermissionHandler> =
            Box::new(InteractiveHandler::new(config.auto_accept_edits));

        let default_rules = generate_default_rules();

        Self {
            mode,
            handler,
            default_rules,
            bypass_disabled: config.disable_bypass,
        }
    }
}

/// Generate default permission rules.
fn generate_default_rules() -> Vec<PermissionRuleV2> {
    vec![
        // Read operations are always allowed
        PermissionRuleV2::new(
            PermissionRuleSource::FlagSettings,
            PermissionBehavior::Allow,
            "Read",
            None,
        ),
        PermissionRuleV2::new(
            PermissionRuleSource::FlagSettings,
            PermissionBehavior::Allow,
            "Grep",
            None,
        ),
        PermissionRuleV2::new(
            PermissionRuleSource::FlagSettings,
            PermissionBehavior::Allow,
            "Glob",
            None,
        ),
        PermissionRuleV2::new(
            PermissionRuleSource::FlagSettings,
            PermissionBehavior::Allow,
            "LS",
            None,
        ),
        // Dangerous patterns are always denied
        PermissionRuleV2::new(
            PermissionRuleSource::FlagSettings,
            PermissionBehavior::Deny,
            "Bash",
            Some("rm -rf /".to_string()),
        ),
    ]
}

/// Get the next permission mode in the cycle.
#[must_use]
pub fn get_next_permission_mode(current: ExtendedPermissionMode) -> ExtendedPermissionMode {
    match current {
        ExtendedPermissionMode::Default => ExtendedPermissionMode::Auto,
        ExtendedPermissionMode::Auto => ExtendedPermissionMode::AcceptEdits,
        ExtendedPermissionMode::AcceptEdits => ExtendedPermissionMode::Plan,
        ExtendedPermissionMode::Plan => ExtendedPermissionMode::Default,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_setup() {
        let config = PermissionSetupConfig::default();
        let setup = PermissionSetup::initialize(&config);
        assert_eq!(setup.mode, ExtendedPermissionMode::Default);
        assert!(!setup.bypass_disabled);
        assert!(!setup.default_rules.is_empty());
    }

    #[test]
    fn disable_bypass_mode() {
        let config = PermissionSetupConfig {
            mode: ExtendedPermissionMode::BypassPermissions,
            disable_bypass: true,
            ..Default::default()
        };
        let setup = PermissionSetup::initialize(&config);
        assert_eq!(setup.mode, ExtendedPermissionMode::Default);
    }

    #[test]
    fn disable_auto_mode() {
        let config = PermissionSetupConfig {
            mode: ExtendedPermissionMode::Auto,
            disable_auto: true,
            ..Default::default()
        };
        let setup = PermissionSetup::initialize(&config);
        assert_eq!(setup.mode, ExtendedPermissionMode::Default);
    }

    #[test]
    fn next_mode_cycle() {
        assert_eq!(
            get_next_permission_mode(ExtendedPermissionMode::Default),
            ExtendedPermissionMode::Auto
        );
        assert_eq!(
            get_next_permission_mode(ExtendedPermissionMode::Auto),
            ExtendedPermissionMode::AcceptEdits
        );
        assert_eq!(
            get_next_permission_mode(ExtendedPermissionMode::AcceptEdits),
            ExtendedPermissionMode::Plan
        );
        assert_eq!(
            get_next_permission_mode(ExtendedPermissionMode::Plan),
            ExtendedPermissionMode::Default
        );
    }

    #[test]
    fn default_rules_include_read() {
        let rules = generate_default_rules();
        assert!(
            rules
                .iter()
                .any(|r| r.value.tool_name == "Read" && r.behavior == PermissionBehavior::Allow)
        );
    }
}
