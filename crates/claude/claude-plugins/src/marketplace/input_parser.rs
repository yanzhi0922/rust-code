//! Marketplace input parser.
//!
//! Parses user input strings into marketplace references, handling
//! various formats: names, URLs, GitHub shorthand, SSH URLs, and local paths.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Parsed marketplace input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MarketplaceInput {
    /// A marketplace name (e.g., `"claude-plugins-official"`).
    Name { name: String },
    /// A URL (HTTP/HTTPS or git SSH).
    Url { url: String },
    /// A local file path.
    Path { path: String },
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Parse user input into a marketplace reference.
///
/// Handles:
/// - Git SSH URLs (`user@host:path`)
/// - HTTP/HTTPS URLs
/// - GitHub shorthand (`owner/repo`)
/// - Local file paths (`.json` files)
/// - Local directory paths
/// - Plain marketplace names
pub fn parse_marketplace_input(input: &str) -> MarketplaceInput {
    let trimmed = input.trim();

    // Handle local paths (check before GitHub shorthand so ~/x isn't
    // misclassified as owner/repo).
    if trimmed.starts_with('.') || trimmed.starts_with('/') || trimmed.starts_with('~') {
        return MarketplaceInput::Path {
            path: trimmed.to_owned(),
        };
    }

    // Handle git SSH URLs (user@host:path or git@host:path)
    if let Some(at_pos) = trimmed.find('@') {
        let prefix = &trimmed[..at_pos];
        let suffix = &trimmed[at_pos + 1..];
        if prefix.contains('.') || prefix == "git" || suffix.contains(':') {
            // Looks like a git SSH URL (user@host:path format)
            return MarketplaceInput::Url {
                url: trimmed.to_owned(),
            };
        }
    }

    // Handle URLs
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return MarketplaceInput::Url {
            url: trimmed.to_owned(),
        };
    }

    // Handle GitHub shorthand (owner/repo)
    if is_github_shorthand(trimmed) {
        return MarketplaceInput::Url {
            url: format!("https://github.com/{trimmed}"),
        };
    }

    // Default: treat as a name
    MarketplaceInput::Name {
        name: trimmed.to_owned(),
    }
}

/// Check if the input looks like a GitHub shorthand (`owner/repo`).
fn is_github_shorthand(input: &str) -> bool {
    if let Some(slash_pos) = input.find('/') {
        let owner = &input[..slash_pos];
        let repo = &input[slash_pos + 1..];
        // owner should be non-empty, repo should be non-empty and not contain
        // further slashes or dots (which would indicate a path)
        !owner.is_empty()
            && !repo.is_empty()
            && !repo.contains('/')
            && !owner.contains('.')
            && !owner.contains(' ')
            && !repo.contains(' ')
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name() {
        let result = parse_marketplace_input("claude-plugins-official");
        assert_eq!(
            result,
            MarketplaceInput::Name {
                name: "claude-plugins-official".to_owned()
            }
        );
    }

    #[test]
    fn parse_https_url() {
        let result = parse_marketplace_input("https://example.com/marketplace.json");
        assert_eq!(
            result,
            MarketplaceInput::Url {
                url: "https://example.com/marketplace.json".to_owned()
            }
        );
    }

    #[test]
    fn parse_git_ssh_url() {
        let result = parse_marketplace_input("git@github.com:org/repo.git");
        assert_eq!(
            result,
            MarketplaceInput::Url {
                url: "git@github.com:org/repo.git".to_owned()
            }
        );
    }

    #[test]
    fn parse_github_shorthand() {
        let result = parse_marketplace_input("anthropics/claude-plugins-official");
        assert_eq!(
            result,
            MarketplaceInput::Url {
                url: "https://github.com/anthropics/claude-plugins-official".to_owned()
            }
        );
    }

    #[test]
    fn parse_local_path() {
        let result = parse_marketplace_input("./my-plugins");
        assert_eq!(
            result,
            MarketplaceInput::Path {
                path: "./my-plugins".to_owned()
            }
        );
    }

    #[test]
    fn parse_absolute_path() {
        let result = parse_marketplace_input("/home/user/plugins");
        assert_eq!(
            result,
            MarketplaceInput::Path {
                path: "/home/user/plugins".to_owned()
            }
        );
    }

    #[test]
    fn parse_tilde_path() {
        let result = parse_marketplace_input("~/plugins");
        assert_eq!(
            result,
            MarketplaceInput::Path {
                path: "~/plugins".to_owned()
            }
        );
    }

    #[test]
    fn parse_trims_whitespace() {
        let result = parse_marketplace_input("  my-marketplace  ");
        assert_eq!(
            result,
            MarketplaceInput::Name {
                name: "my-marketplace".to_owned()
            }
        );
    }

    #[test]
    fn parse_deploy_ssh_url() {
        let result = parse_marketplace_input("deploy@gitlab.com:group/project.git");
        assert_eq!(
            result,
            MarketplaceInput::Url {
                url: "deploy@gitlab.com:group/project.git".to_owned()
            }
        );
    }

    #[test]
    fn not_github_shorthand_with_spaces() {
        let result = parse_marketplace_input("not a shorthand");
        assert!(matches!(result, MarketplaceInput::Name { .. }));
    }

    #[test]
    fn not_github_shorthand_with_dots() {
        let result = parse_marketplace_input("file.txt");
        assert!(matches!(result, MarketplaceInput::Name { .. }));
    }
}
