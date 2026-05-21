//! Official marketplace constants and helpers.
//!
//! Defines the official Anthropic plugins marketplace and provides
//! functions to identify official marketplaces.

use serde::{Deserialize, Serialize};

use crate::schemas::ALLOWED_OFFICIAL_MARKETPLACE_NAMES;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Source configuration for the official Anthropic plugins marketplace.
pub const OFFICIAL_MARKETPLACE_SOURCE: OfficialMarketplaceSource = OfficialMarketplaceSource {
    source: "github",
    repo: "anthropics/claude-plugins-official",
};

/// Display name for the official marketplace.
pub const OFFICIAL_MARKETPLACE_NAME: &str = "claude-plugins-official";

/// Official GitHub organization for Anthropic marketplaces.
pub const OFFICIAL_GITHUB_ORG: &str = "anthropics";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Source configuration for the official marketplace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficialMarketplaceSource {
    /// Source type.
    pub source: &'static str,
    /// Repository path.
    pub repo: &'static str,
}

/// A known official marketplace definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficialMarketplace {
    /// Marketplace name.
    pub name: &'static str,
    /// Marketplace source.
    pub source: OfficialMarketplaceSource,
    /// Human-readable description.
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// Official marketplaces list
// ---------------------------------------------------------------------------

/// List of known official marketplaces.
static OFFICIAL_MARKETPLACES: &[OfficialMarketplace] = &[
    OfficialMarketplace {
        name: "claude-plugins-official",
        source: OfficialMarketplaceSource {
            source: "github",
            repo: "anthropics/claude-plugins-official",
        },
        description: "Official Anthropic plugins marketplace",
    },
    OfficialMarketplace {
        name: "claude-code-marketplace",
        source: OfficialMarketplaceSource {
            source: "github",
            repo: "anthropics/claude-code-marketplace",
        },
        description: "Claude Code plugins marketplace",
    },
    OfficialMarketplace {
        name: "agent-skills",
        source: OfficialMarketplaceSource {
            source: "github",
            repo: "anthropics/agent-skills",
        },
        description: "Agent skills marketplace",
    },
];

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Get the list of known official marketplaces.
pub fn get_official_marketplaces() -> &'static [OfficialMarketplace] {
    OFFICIAL_MARKETPLACES
}

/// Check if a marketplace name is official.
pub fn is_official_marketplace(name: &str) -> bool {
    ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(&name.to_lowercase().as_str())
        || OFFICIAL_MARKETPLACES.iter().any(|mkt| mkt.name == name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_marketplace_source() {
        assert_eq!(OFFICIAL_MARKETPLACE_SOURCE.source, "github");
        assert_eq!(
            OFFICIAL_MARKETPLACE_SOURCE.repo,
            "anthropics/claude-plugins-official"
        );
    }

    #[test]
    fn official_marketplace_name() {
        assert_eq!(OFFICIAL_MARKETPLACE_NAME, "claude-plugins-official");
    }

    #[test]
    fn get_official_marketplaces_not_empty() {
        let marketplaces = get_official_marketplaces();
        assert!(!marketplaces.is_empty());
    }

    #[test]
    fn is_official_marketplace_known() {
        assert!(is_official_marketplace("claude-plugins-official"));
        assert!(is_official_marketplace("claude-code-marketplace"));
        assert!(is_official_marketplace("agent-skills"));
    }

    #[test]
    fn is_official_marketplace_unknown() {
        assert!(!is_official_marketplace("my-custom-marketplace"));
        assert!(!is_official_marketplace("random-plugins"));
    }

    #[test]
    fn is_official_marketplace_case_insensitive() {
        assert!(is_official_marketplace("Claude-Code-Marketplace"));
    }
}
