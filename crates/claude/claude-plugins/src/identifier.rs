//! Plugin identifier parsing and formatting.
//!
//! Provides [`PluginIdentifier`] for parsing and representing plugin IDs
//! in the format `"plugin-name@marketplace-name"`, along with helpers
//! for building, comparing, and validating identifiers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::schemas::ALLOWED_OFFICIAL_MARKETPLACE_NAMES;

// ---------------------------------------------------------------------------
// PluginIdentifier
// ---------------------------------------------------------------------------

/// Parsed plugin identifier with name and optional marketplace.
///
/// Plugin IDs follow the format `"plugin-name@marketplace-name"`.
/// Both parts allow alphanumeric characters, hyphens, dots, and underscores.
///
/// # Examples
///
/// ```
/// use claude_plugins::identifier::PluginIdentifier;
///
/// let id = PluginIdentifier::parse("code-formatter@anthropic-tools");
/// assert_eq!(id.name, "code-formatter");
/// assert_eq!(id.marketplace.as_deref(), Some("anthropic-tools"));
///
/// let id = PluginIdentifier::parse("my-plugin");
/// assert_eq!(id.name, "my-plugin");
/// assert_eq!(id.marketplace, None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginIdentifier {
    /// Plugin name component.
    pub name: String,
    /// Optional marketplace name component.
    pub marketplace: Option<String>,
}

impl PluginIdentifier {
    /// Parse a plugin identifier string into name and marketplace components.
    ///
    /// Only the first `@` is used as separator. If the input contains
    /// multiple `@` symbols (e.g., `"plugin@market@place"`), only the
    /// first two parts are used.
    pub fn parse(plugin: &str) -> Self {
        if let Some(at_pos) = plugin.find('@') {
            let name = &plugin[..at_pos];
            let rest = &plugin[at_pos + 1..];
            // Only take the first segment after @ as marketplace
            let marketplace = if let Some(second_at) = rest.find('@') {
                &rest[..second_at]
            } else {
                rest
            };
            Self {
                name: name.to_string(),
                marketplace: if marketplace.is_empty() {
                    None
                } else {
                    Some(marketplace.to_string())
                },
            }
        } else {
            Self {
                name: plugin.to_string(),
                marketplace: None,
            }
        }
    }

    /// Build a plugin ID string from name and optional marketplace.
    pub fn build_id(name: &str, marketplace: Option<&str>) -> String {
        match marketplace {
            Some(mkt) => format!("{name}@{mkt}"),
            None => name.to_string(),
        }
    }

    /// Check if a marketplace name is an official (Anthropic-controlled)
    /// marketplace.
    ///
    /// Used for telemetry redaction — official plugin identifiers are safe to
    /// log to general-access metadata; third-party identifiers go only to
    /// PII-tagged columns.
    pub fn is_official_marketplace(marketplace: Option<&str>) -> bool {
        marketplace.is_some_and(|m| {
            ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(&m.to_lowercase().as_str())
        })
    }

    /// Returns the full plugin ID string (`"name@marketplace"` or `"name"`).
    pub fn to_id_string(&self) -> String {
        Self::build_id(&self.name, self.marketplace.as_deref())
    }
}

impl fmt::Display for PluginIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.marketplace {
            Some(mkt) => write!(f, "{}@{}", self.name, mkt),
            None => write!(f, "{}", self.name),
        }
    }
}

impl FromStr for PluginIdentifier {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

// ---------------------------------------------------------------------------
// Extended scope helpers
// ---------------------------------------------------------------------------

/// Extended scope type that includes 'flag' for session-only plugins.
/// 'flag' scope is NOT persisted to installed_plugins.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtendedPluginScope {
    /// Enterprise/system-wide.
    Managed,
    /// User's global settings.
    User,
    /// Shared project settings.
    Project,
    /// Personal project overrides.
    Local,
    /// Session-only (from --settings), not persisted.
    Flag,
}

/// Scopes that are persisted to installed_plugins.json.
/// Excludes 'flag' which is session-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PersistablePluginScope {
    /// Enterprise/system-wide.
    Managed,
    /// User's global settings.
    User,
    /// Shared project settings.
    Project,
    /// Personal project overrides.
    Local,
}

impl TryFrom<ExtendedPluginScope> for PersistablePluginScope {
    type Error = &'static str;

    fn try_from(value: ExtendedPluginScope) -> Result<Self, Self::Error> {
        match value {
            ExtendedPluginScope::Managed => Ok(PersistablePluginScope::Managed),
            ExtendedPluginScope::User => Ok(PersistablePluginScope::User),
            ExtendedPluginScope::Project => Ok(PersistablePluginScope::Project),
            ExtendedPluginScope::Local => Ok(PersistablePluginScope::Local),
            ExtendedPluginScope::Flag => Err("Cannot convert 'flag' scope to persistable scope"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_name() {
        let id = PluginIdentifier::parse("my-plugin");
        assert_eq!(id.name, "my-plugin");
        assert_eq!(id.marketplace, None);
    }

    #[test]
    fn test_parse_name_at_marketplace() {
        let id = PluginIdentifier::parse("code-formatter@anthropic-tools");
        assert_eq!(id.name, "code-formatter");
        assert_eq!(id.marketplace, Some("anthropic-tools".to_string()));
    }

    #[test]
    fn test_parse_multiple_at_signs() {
        let id = PluginIdentifier::parse("plugin@market@place");
        assert_eq!(id.name, "plugin");
        assert_eq!(id.marketplace, Some("market".to_string()));
    }

    #[test]
    fn test_parse_empty_after_at() {
        let id = PluginIdentifier::parse("plugin@");
        assert_eq!(id.name, "plugin");
        assert_eq!(id.marketplace, None);
    }

    #[test]
    fn test_parse_at_start() {
        let id = PluginIdentifier::parse("@marketplace");
        assert_eq!(id.name, "");
        assert_eq!(id.marketplace, Some("marketplace".to_string()));
    }

    #[test]
    fn test_build_id_with_marketplace() {
        assert_eq!(
            PluginIdentifier::build_id("my-plugin", Some("my-market")),
            "my-plugin@my-market"
        );
    }

    #[test]
    fn test_build_id_without_marketplace() {
        assert_eq!(PluginIdentifier::build_id("my-plugin", None), "my-plugin");
    }

    #[test]
    fn test_display_with_marketplace() {
        let id = PluginIdentifier {
            name: "plugin".into(),
            marketplace: Some("market".into()),
        };
        assert_eq!(format!("{id}"), "plugin@market");
    }

    #[test]
    fn test_display_without_marketplace() {
        let id = PluginIdentifier {
            name: "plugin".into(),
            marketplace: None,
        };
        assert_eq!(format!("{id}"), "plugin");
    }

    #[test]
    fn test_from_str() {
        let id: PluginIdentifier = "my-plugin@my-market".parse().expect("parse");
        assert_eq!(id.name, "my-plugin");
        assert_eq!(id.marketplace, Some("my-market".to_string()));
    }

    #[test]
    fn test_to_id_string() {
        let id = PluginIdentifier {
            name: "plugin".into(),
            marketplace: Some("market".into()),
        };
        assert_eq!(id.to_id_string(), "plugin@market");
    }

    #[test]
    fn test_to_id_string_no_marketplace() {
        let id = PluginIdentifier {
            name: "plugin".into(),
            marketplace: None,
        };
        assert_eq!(id.to_id_string(), "plugin");
    }

    #[test]
    fn test_is_official_marketplace_true() {
        assert!(PluginIdentifier::is_official_marketplace(Some(
            "claude-code-marketplace"
        )));
        assert!(PluginIdentifier::is_official_marketplace(Some(
            "Claude-Code-Marketplace"
        )));
        assert!(PluginIdentifier::is_official_marketplace(Some(
            "agent-skills"
        )));
    }

    #[test]
    fn test_is_official_marketplace_false() {
        assert!(!PluginIdentifier::is_official_marketplace(Some(
            "my-marketplace"
        )));
        assert!(!PluginIdentifier::is_official_marketplace(None));
    }

    #[test]
    fn test_equality() {
        let a = PluginIdentifier {
            name: "plugin".into(),
            marketplace: Some("market".into()),
        };
        let b = PluginIdentifier {
            name: "plugin".into(),
            marketplace: Some("market".into()),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_inequality() {
        let a = PluginIdentifier {
            name: "plugin-a".into(),
            marketplace: Some("market".into()),
        };
        let b = PluginIdentifier {
            name: "plugin-b".into(),
            marketplace: Some("market".into()),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_serde_roundtrip() {
        let id = PluginIdentifier {
            name: "code-formatter".into(),
            marketplace: Some("anthropic-tools".into()),
        };
        let json = serde_json::to_string(&id).expect("serialize");
        let back: PluginIdentifier = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn test_extended_plugin_scope_serde() {
        let json = "\"flag\"";
        let scope: ExtendedPluginScope = serde_json::from_str(json).expect("parse");
        assert_eq!(scope, ExtendedPluginScope::Flag);

        let json = "\"user\"";
        let scope: ExtendedPluginScope = serde_json::from_str(json).expect("parse");
        assert_eq!(scope, ExtendedPluginScope::User);
    }

    #[test]
    fn test_persistable_scope_try_from() {
        assert!(PersistablePluginScope::try_from(ExtendedPluginScope::User).is_ok());
        assert!(PersistablePluginScope::try_from(ExtendedPluginScope::Flag).is_err());
    }
}
