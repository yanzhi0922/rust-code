//! Plugin manifest schema types and marketplace validation.
//!
//! Rust equivalents of the Zod schemas from `schemas.ts`. Provides
//! serde-based types for plugin manifests, marketplace configurations,
//! and validation helpers for official marketplace name protection.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Official marketplace name constants
// ---------------------------------------------------------------------------

/// Official marketplace names reserved for Anthropic/Claude official use.
/// These names are allowed ONLY for official marketplaces and blocked for
/// third parties.
pub static ALLOWED_OFFICIAL_MARKETPLACE_NAMES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("claude-code-marketplace");
    set.insert("claude-code-plugins");
    set.insert("claude-plugins-official");
    set.insert("anthropic-marketplace");
    set.insert("anthropic-plugins");
    set.insert("agent-skills");
    set.insert("life-sciences");
    set.insert("knowledge-work-plugins");
    set
});

/// Official marketplaces that should NOT auto-update by default.
pub static NO_AUTO_UPDATE_OFFICIAL_MARKETPLACES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("knowledge-work-plugins");
    set
});

/// Pattern to detect names that impersonate official Anthropic/Claude
/// marketplaces.
///
/// Matches names containing variations like:
/// - "official" combined with "anthropic" or "claude"
/// - Names starting with "anthropic" or "claude" followed by
///   official-sounding terms like "marketplace", "plugins"
pub static BLOCKED_OFFICIAL_NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:official[^a-z0-9]*(anthropic|claude)|(?:anthropic|claude)[^a-z0-9]*official|^(?:anthropic|claude)[^a-z0-9]*(marketplace|plugins|official))")
        .expect("BLOCKED_OFFICIAL_NAME_PATTERN is a valid regex")
});

/// Pattern to detect non-ASCII characters that could be used for homograph
/// attacks.
static NON_ASCII_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[^\u{0020}-\u{007E}]").expect("NON_ASCII_PATTERN is a valid regex"));

/// The official GitHub organization for Anthropic marketplaces.
pub const OFFICIAL_GITHUB_ORG: &str = "anthropics";

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Check if a marketplace name impersonates an official Anthropic/Claude
/// marketplace.
///
/// Returns `true` if the name is blocked (impersonates official), `false` if
/// allowed.
pub fn is_blocked_official_name(name: &str) -> bool {
    // If it's in the allowed list, it's not blocked
    if ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(&name.to_lowercase().as_str()) {
        return false;
    }

    // Block names with non-ASCII characters to prevent homograph attacks
    if NON_ASCII_PATTERN.is_match(name) {
        return true;
    }

    // Check if it matches the blocked pattern
    BLOCKED_OFFICIAL_NAME_PATTERN.is_match(name)
}

/// Check if auto-update is enabled for a marketplace.
///
/// Uses the stored value if set, otherwise defaults based on whether it's an
/// official Anthropic marketplace (true) or not (false). Official marketplaces
/// in `NO_AUTO_UPDATE_OFFICIAL_MARKETPLACES` are excluded from the auto-update
/// default.
pub fn is_marketplace_auto_update(marketplace_name: &str, auto_update: Option<bool>) -> bool {
    if let Some(val) = auto_update {
        return val;
    }
    let normalized = marketplace_name.to_lowercase();
    ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(normalized.as_str())
        && !NO_AUTO_UPDATE_OFFICIAL_MARKETPLACES.contains(normalized.as_str())
}

/// Validate that a marketplace with a reserved name comes from the official
/// source.
///
/// Reserved names (in `ALLOWED_OFFICIAL_MARKETPLACE_NAMES`) can only be used
/// by marketplaces from the official Anthropic GitHub organization.
///
/// Returns `Ok(())` if valid, or an error message string if validation fails.
pub fn validate_official_name_source(
    name: &str,
    source: &MarketplaceSourceKind,
) -> Result<(), String> {
    let normalized = name.to_lowercase();

    // Only validate reserved names
    if !ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(normalized.as_str()) {
        return Ok(()); // Not a reserved name, no source validation needed
    }

    match source {
        MarketplaceSourceKind::Github { repo, .. } => {
            if repo
                .to_lowercase()
                .starts_with(&format!("{OFFICIAL_GITHUB_ORG}/"))
            {
                Ok(())
            } else {
                Err(format!(
                    "The name '{name}' is reserved for official Anthropic marketplaces. \
                     Only repositories from 'github.com/{OFFICIAL_GITHUB_ORG}/' \
                     can use this name."
                ))
            }
        }
        MarketplaceSourceKind::Git { url, .. } => {
            let url_lower = url.to_lowercase();
            let is_https_anthropics = url_lower.contains("github.com/anthropics/");
            let is_ssh_anthropics = url_lower.contains("git@github.com:anthropics/");

            if is_https_anthropics || is_ssh_anthropics {
                Ok(())
            } else {
                Err(format!(
                    "The name '{name}' is reserved for official Anthropic marketplaces. \
                     Only repositories from 'github.com/{OFFICIAL_GITHUB_ORG}/' \
                     can use this name."
                ))
            }
        }
        _ => Err(format!(
            "The name '{name}' is reserved for official Anthropic marketplaces \
             and can only be used with GitHub sources from the \
             '{OFFICIAL_GITHUB_ORG}' organization."
        )),
    }
}

/// Check if a marketplace name is valid (passes all validation rules).
pub fn validate_marketplace_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Marketplace must have a name".into());
    }
    if name.contains(' ') {
        return Err(
            "Marketplace name cannot contain spaces. Use kebab-case (e.g., \"my-marketplace\")"
                .into(),
        );
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name == "." {
        return Err(
            "Marketplace name cannot contain path separators (/ or \\), \"..\" sequences, or be \".\""
                .into(),
        );
    }
    if is_blocked_official_name(name) {
        return Err(
            "Marketplace name impersonates an official Anthropic/Claude marketplace".into(),
        );
    }
    if name.eq_ignore_ascii_case("inline") {
        return Err(
            "Marketplace name \"inline\" is reserved for --plugin-dir session plugins".into(),
        );
    }
    if name.eq_ignore_ascii_case("builtin") {
        return Err("Marketplace name \"builtin\" is reserved for built-in plugins".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

/// Plugin author information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAuthorSchema {
    /// Display name of the plugin author or organization.
    pub name: String,
    /// Contact email for support or feedback.
    #[serde(default)]
    pub email: Option<String>,
    /// Website, GitHub profile, or organization URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// Plugin scope types for installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    /// Enterprise/system-wide (read-only, platform-specific paths).
    Managed,
    /// User's global settings.
    User,
    /// Shared project settings.
    Project,
    /// Personal project overrides.
    Local,
}

/// User-configurable option type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserConfigOptionType {
    String,
    Number,
    Boolean,
    Directory,
    File,
}

/// A single user-configurable option in plugin manifest `userConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginUserConfigOption {
    /// Type of the configuration value.
    #[serde(rename = "type")]
    pub option_type: UserConfigOptionType,
    /// Human-readable label shown in the config dialog.
    pub title: String,
    /// Help text shown beneath the field in the config dialog.
    pub description: String,
    /// If true, validation fails when this field is empty.
    #[serde(default)]
    pub required: Option<bool>,
    /// Default value used when the user provides nothing.
    #[serde(default)]
    pub default: Option<String>,
    /// For string type: allow an array of strings.
    #[serde(default)]
    pub multiple: Option<bool>,
    /// If true, masks dialog input and stores value in secure storage.
    #[serde(default)]
    pub sensitive: Option<bool>,
    /// Minimum value (number type only).
    #[serde(default)]
    pub min: Option<i64>,
    /// Maximum value (number type only).
    #[serde(default)]
    pub max: Option<i64>,
}

/// Plugin manifest metadata (the core fields of plugin.json).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifestMetadata {
    /// Unique identifier for the plugin (prefer kebab-case).
    pub name: String,
    /// Semantic version (e.g., 1.2.3).
    #[serde(default)]
    pub version: Option<String>,
    /// Brief, user-facing explanation of what the plugin provides.
    #[serde(default)]
    pub description: Option<String>,
    /// Information about the plugin creator or maintainer.
    #[serde(default)]
    pub author: Option<PluginAuthorSchema>,
    /// Plugin homepage or documentation URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Source code repository URL.
    #[serde(default)]
    pub repository: Option<String>,
    /// SPDX license identifier (e.g., MIT, Apache-2.0).
    #[serde(default)]
    pub license: Option<String>,
    /// Tags for plugin discovery and categorization.
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    /// Plugins that must be enabled for this plugin to function.
    #[serde(default)]
    pub dependencies: Option<Vec<String>>,
}

/// Marketplace source kinds — the various ways to reference marketplace
/// manifests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum MarketplaceSourceKind {
    /// Direct URL to marketplace.json file.
    Url {
        /// URL to the marketplace.json file.
        url: String,
        /// Custom HTTP headers (e.g., for authentication).
        #[serde(default)]
        headers: Option<std::collections::BTreeMap<String, String>>,
    },
    /// GitHub repository source.
    Github {
        /// GitHub repository in owner/repo format.
        repo: String,
        /// Git branch or tag to use.
        #[serde(default)]
        r#ref: Option<String>,
        /// Path to marketplace.json within repo.
        #[serde(default)]
        path: Option<String>,
        /// Directories to include via git sparse-checkout.
        #[serde(default)]
        sparse_paths: Option<Vec<String>>,
    },
    /// Generic git repository URL source.
    Git {
        /// Full git repository URL.
        url: String,
        /// Git branch or tag to use.
        #[serde(default)]
        r#ref: Option<String>,
        /// Path to marketplace.json within repo.
        #[serde(default)]
        path: Option<String>,
        /// Directories to include via git sparse-checkout.
        #[serde(default)]
        sparse_paths: Option<Vec<String>>,
    },
    /// NPM package source.
    Npm {
        /// NPM package containing marketplace.json.
        package: String,
    },
    /// Local file path to marketplace.json.
    File {
        /// Local file path.
        path: String,
    },
    /// Local directory containing .claude-plugin/marketplace.json.
    Directory {
        /// Local directory path.
        path: String,
    },
    /// Regex pattern to match host/domain.
    HostPattern {
        /// Regex pattern string.
        host_pattern: String,
    },
    /// Regex pattern matched against filesystem paths.
    PathPattern {
        /// Regex pattern string.
        path_pattern: String,
    },
    /// Inline marketplace manifest defined directly in settings.json.
    Settings {
        /// Marketplace name.
        name: String,
        /// Plugin entries declared inline.
        plugins: Vec<SettingsMarketplacePlugin>,
        /// Optional owner information.
        #[serde(default)]
        owner: Option<PluginAuthorSchema>,
    },
}

/// Narrow plugin entry for settings-sourced marketplaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsMarketplacePlugin {
    /// Plugin name as it appears in the target repository.
    pub name: String,
    /// Where to fetch the plugin from.
    pub source: PluginSourceSchema,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional version.
    #[serde(default)]
    pub version: Option<String>,
    /// Whether to require strict manifest validation.
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Plugin source schema — various ways to reference and install plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginSourceSchema {
    /// Relative path to the plugin root (starts with `./`).
    RelativePath(String),
    /// NPM package source.
    Npm {
        /// Package name or URL.
        package: String,
        /// Version or version range.
        #[serde(default)]
        version: Option<String>,
        /// Custom NPM registry URL.
        #[serde(default)]
        registry: Option<String>,
    },
    /// Python pip package source.
    Pip {
        /// Python package name.
        package: String,
        /// Version specifier.
        #[serde(default)]
        version: Option<String>,
        /// Custom PyPI registry URL.
        #[serde(default)]
        registry: Option<String>,
    },
    /// Git URL source.
    GitUrl {
        /// Full git repository URL.
        url: String,
        /// Git branch or tag.
        #[serde(default)]
        r#ref: Option<String>,
        /// Specific commit SHA.
        #[serde(default)]
        sha: Option<String>,
    },
    /// GitHub repository source.
    Github {
        /// GitHub repository in owner/repo format.
        repo: String,
        /// Git branch or tag.
        #[serde(default)]
        r#ref: Option<String>,
        /// Specific commit SHA.
        #[serde(default)]
        sha: Option<String>,
    },
    /// Plugin in a subdirectory of a monorepo.
    GitSubdir {
        /// Git repository URL.
        url: String,
        /// Subdirectory within the repo.
        path: String,
        /// Git branch or tag.
        #[serde(default)]
        r#ref: Option<String>,
        /// Specific commit SHA.
        #[serde(default)]
        sha: Option<String>,
    },
}

/// Check if a plugin source is a local path (stored in marketplace directory).
pub fn is_local_plugin_source(source: &PluginSourceSchema) -> bool {
    matches!(source, PluginSourceSchema::RelativePath(p) if p.starts_with("./"))
}

/// Check if a marketplace source points at a user-controlled local filesystem
/// path.
pub fn is_local_marketplace_source(source: &MarketplaceSourceKind) -> bool {
    matches!(
        source,
        MarketplaceSourceKind::File { .. } | MarketplaceSourceKind::Directory { .. }
    )
}

/// Plugin manifest file (plugin.json) — the full schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifestSchema {
    // --- Metadata fields ---
    /// Unique identifier for the plugin.
    pub name: String,
    /// Semantic version.
    #[serde(default)]
    pub version: Option<String>,
    /// Brief description.
    #[serde(default)]
    pub description: Option<String>,
    /// Author information.
    #[serde(default)]
    pub author: Option<PluginAuthorSchema>,
    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Repository URL.
    #[serde(default)]
    pub repository: Option<String>,
    /// License identifier.
    #[serde(default)]
    pub license: Option<String>,
    /// Keywords for discovery.
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    /// Dependencies on other plugins.
    #[serde(default)]
    pub dependencies: Option<Vec<String>>,

    // --- Hooks ---
    /// Additional hooks configuration.
    #[serde(default)]
    pub hooks: Option<serde_json::Value>,

    // --- Commands ---
    /// Additional command definitions.
    #[serde(default)]
    pub commands: Option<serde_json::Value>,

    // --- Agents ---
    /// Additional agent definitions.
    #[serde(default)]
    pub agents: Option<serde_json::Value>,

    // --- Skills ---
    /// Additional skill directories.
    #[serde(default)]
    pub skills: Option<serde_json::Value>,

    // --- Output styles ---
    /// Additional output style definitions.
    #[serde(default)]
    pub output_styles: Option<serde_json::Value>,

    // --- Channels ---
    /// Channel declarations for MCP servers.
    #[serde(default)]
    pub channels: Option<Vec<PluginChannel>>,

    // --- MCP servers ---
    /// MCP server configurations.
    #[serde(default)]
    pub mcp_servers: Option<serde_json::Value>,

    // --- LSP servers ---
    /// LSP server configurations.
    #[serde(default)]
    pub lsp_servers: Option<serde_json::Value>,

    // --- Settings ---
    /// Settings to merge when plugin is enabled.
    #[serde(default)]
    pub settings: Option<std::collections::BTreeMap<String, serde_json::Value>>,

    // --- User config ---
    /// User-configurable values the plugin needs.
    #[serde(default)]
    pub user_config: Option<std::collections::BTreeMap<String, PluginUserConfigOption>>,
}

/// A channel declaration in a plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginChannel {
    /// Name of the MCP server this channel binds to.
    pub server: String,
    /// Human-readable name shown in config dialog.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Fields to prompt the user for.
    #[serde(default)]
    pub user_config: Option<std::collections::BTreeMap<String, PluginUserConfigOption>>,
}

/// Individual plugin entry in a marketplace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplaceEntrySchema {
    /// Unique identifier matching the plugin name.
    pub name: String,
    /// Where to fetch the plugin from.
    pub source: PluginSourceSchema,
    /// Category for organizing plugins.
    #[serde(default)]
    pub category: Option<String>,
    /// Tags for searchability and discovery.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Require the plugin manifest to be present in the plugin folder.
    #[serde(default = "default_true")]
    pub strict: bool,
    /// Brief description.
    #[serde(default)]
    pub description: Option<String>,
    /// Version.
    #[serde(default)]
    pub version: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Plugin marketplace configuration (marketplace.json).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePluginSchema {
    /// Marketplace name.
    pub name: String,
    /// Marketplace maintainer or curator information.
    pub owner: PluginAuthorSchema,
    /// Collection of available plugins.
    pub plugins: Vec<PluginMarketplaceEntrySchema>,
    /// When true, plugins removed from this marketplace will be automatically
    /// uninstalled.
    #[serde(default)]
    pub force_remove_deleted_plugins: Option<bool>,
    /// Optional marketplace metadata.
    #[serde(default)]
    pub metadata: Option<MarketplaceMetadata>,
    /// Marketplace names whose plugins may be auto-installed as dependencies.
    #[serde(default)]
    pub allow_cross_marketplace_dependencies_on: Option<Vec<String>>,
}

/// Optional marketplace metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceMetadata {
    /// Base path for relative plugin sources.
    #[serde(default)]
    pub plugin_root: Option<String>,
    /// Marketplace version.
    #[serde(default)]
    pub version: Option<String>,
    /// Marketplace description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Plugin settings schema — settings to merge into the settings cascade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSettingsSchema {
    /// Settings to merge when plugin is enabled.
    #[serde(default)]
    pub settings: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// Plugin hook schema — hooks configuration from hooks.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookSchema {
    /// Brief description of what these hooks provide.
    #[serde(default)]
    pub description: Option<String>,
    /// The hooks provided by the plugin.
    pub hooks: serde_json::Value,
}

/// Installed plugin metadata (V1 format).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginSchema {
    /// Currently installed version.
    pub version: String,
    /// ISO 8601 timestamp of installation.
    pub installed_at: String,
    /// ISO 8601 timestamp of last update.
    #[serde(default)]
    pub last_updated: Option<String>,
    /// Absolute path to the installed plugin directory.
    pub install_path: String,
    /// Git commit SHA for git-based plugins.
    #[serde(default)]
    pub git_commit_sha: Option<String>,
}

/// Installed plugins file (V1 format).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginsFileSchemaV1 {
    /// Schema version 1.
    pub version: u32,
    /// Map of plugin IDs to their installation metadata.
    pub plugins: std::collections::BTreeMap<String, InstalledPluginSchema>,
}

/// A single plugin installation entry (V2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallationEntrySchema {
    /// Installation scope.
    pub scope: PluginScope,
    /// Project path (required for project/local scopes).
    #[serde(default)]
    pub project_path: Option<String>,
    /// Absolute path to the versioned plugin directory.
    pub install_path: String,
    /// Currently installed version.
    #[serde(default)]
    pub version: Option<String>,
    /// ISO 8601 timestamp of installation.
    #[serde(default)]
    pub installed_at: Option<String>,
    /// ISO 8601 timestamp of last update.
    #[serde(default)]
    pub last_updated: Option<String>,
    /// Git commit SHA for git-based plugins.
    #[serde(default)]
    pub git_commit_sha: Option<String>,
}

/// Installed plugins file (V2 format).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginsFileSchemaV2 {
    /// Schema version 2.
    pub version: u32,
    /// Map of plugin IDs to arrays of installation entries.
    pub plugins: std::collections::BTreeMap<String, Vec<PluginInstallationEntrySchema>>,
}

/// Known marketplace entry — tracks metadata about a registered marketplace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownMarketplaceSchema {
    /// Where to fetch the marketplace from.
    pub source: MarketplaceSourceKind,
    /// Local cache path where marketplace manifest is stored.
    pub install_location: String,
    /// ISO 8601 timestamp of last marketplace refresh.
    pub last_updated: String,
    /// Whether to automatically update this marketplace on startup.
    #[serde(default)]
    pub auto_update: Option<bool>,
}

/// Known marketplaces file — maps marketplace names to their metadata.
pub type KnownMarketplacesFileSchema = std::collections::BTreeMap<String, KnownMarketplaceSchema>;

/// Plugin reference in settings — either a simple string ID or an extended
/// object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingsPluginEntrySchema {
    /// Simple format: "plugin@marketplace"
    Simple(String),
    /// Extended format with configuration.
    Extended(SettingsPluginEntryExtended),
}

/// Extended settings plugin entry with additional configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPluginEntryExtended {
    /// Plugin identifier.
    pub id: String,
    /// Version constraint.
    #[serde(default)]
    pub version: Option<String>,
    /// If true, cannot be disabled.
    #[serde(default)]
    pub required: Option<bool>,
    /// Plugin-specific configuration.
    #[serde(default)]
    pub config: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// LSP server configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspServerConfigSchema {
    /// Command to execute the LSP server.
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Mapping from file extension to LSP language ID.
    pub extension_to_language: std::collections::BTreeMap<String, String>,
    /// Communication transport mechanism.
    #[serde(default = "default_stdio")]
    pub transport: String,
    /// Environment variables.
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// Initialization options.
    #[serde(default)]
    pub initialization_options: Option<serde_json::Value>,
    /// Settings for workspace/didChangeConfiguration.
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    /// Workspace folder path.
    #[serde(default)]
    pub workspace_folder: Option<String>,
    /// Maximum time to wait for server startup (ms).
    #[serde(default)]
    pub startup_timeout: Option<u64>,
    /// Maximum time to wait for graceful shutdown (ms).
    #[serde(default)]
    pub shutdown_timeout: Option<u64>,
    /// Whether to restart the server if it crashes.
    #[serde(default)]
    pub restart_on_crash: Option<bool>,
    /// Maximum number of restart attempts.
    #[serde(default)]
    pub max_restarts: Option<u32>,
}

fn default_stdio() -> String {
    String::from("stdio")
}

/// Command metadata for plugin commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandMetadataSchema {
    /// Path to command markdown file, relative to plugin root.
    #[serde(default)]
    pub source: Option<String>,
    /// Inline markdown content for the command.
    #[serde(default)]
    pub content: Option<String>,
    /// Command description override.
    #[serde(default)]
    pub description: Option<String>,
    /// Hint for command arguments.
    #[serde(default)]
    pub argument_hint: Option<String>,
    /// Default model for this command.
    #[serde(default)]
    pub model: Option<String>,
    /// Tools allowed when command runs.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_official_names_contains_expected() {
        assert!(ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains("claude-code-marketplace"));
        assert!(ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains("anthropic-marketplace"));
        assert!(ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains("agent-skills"));
        assert!(ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains("knowledge-work-plugins"));
    }

    #[test]
    fn test_blocked_official_name_allows_official_names() {
        assert!(!is_blocked_official_name("claude-code-marketplace"));
        assert!(!is_blocked_official_name("anthropic-plugins"));
        assert!(!is_blocked_official_name("agent-skills"));
    }

    #[test]
    fn test_blocked_official_name_blocks_impersonation() {
        assert!(is_blocked_official_name("official-claude-plugins"));
        assert!(is_blocked_official_name("anthropic-official"));
        assert!(is_blocked_official_name("claude-marketplace-new"));
        assert!(is_blocked_official_name("anthropic-plugins-v2"));
    }

    #[test]
    fn test_blocked_official_name_allows_legitimate() {
        assert!(!is_blocked_official_name("my-cool-marketplace"));
        assert!(!is_blocked_official_name("company-tools"));
        assert!(!is_blocked_official_name("dev-plugins"));
    }

    #[test]
    fn test_blocked_official_name_blocks_non_ascii() {
        assert!(is_blocked_official_name("аnthropic-plugins")); // Cyrillic 'а'
        assert!(is_blocked_official_name("clаude-marketplace")); // Cyrillic 'а'
    }

    #[test]
    fn test_validate_marketplace_name_rejects_empty() {
        assert!(validate_marketplace_name("").is_err());
    }

    #[test]
    fn test_validate_marketplace_name_rejects_spaces() {
        assert!(validate_marketplace_name("my marketplace").is_err());
    }

    #[test]
    fn test_validate_marketplace_name_rejects_path_separators() {
        assert!(validate_marketplace_name("my/marketplace").is_err());
        assert!(validate_marketplace_name("my\\marketplace").is_err());
        assert!(validate_marketplace_name("..").is_err());
        assert!(validate_marketplace_name(".").is_err());
    }

    #[test]
    fn test_validate_marketplace_name_rejects_inline() {
        assert!(validate_marketplace_name("inline").is_err());
        assert!(validate_marketplace_name("Inline").is_err());
        assert!(validate_marketplace_name("INLINE").is_err());
    }

    #[test]
    fn test_validate_marketplace_name_rejects_builtin() {
        assert!(validate_marketplace_name("builtin").is_err());
    }

    #[test]
    fn test_validate_marketplace_name_accepts_valid() {
        assert!(validate_marketplace_name("my-marketplace").is_ok());
        assert!(validate_marketplace_name("company-plugins").is_ok());
    }

    #[test]
    fn test_is_marketplace_auto_update_default_for_official() {
        assert!(is_marketplace_auto_update("claude-code-marketplace", None));
        assert!(is_marketplace_auto_update("anthropic-marketplace", None));
    }

    #[test]
    fn test_is_marketplace_auto_update_no_default_for_knowledge_work() {
        assert!(!is_marketplace_auto_update("knowledge-work-plugins", None));
    }

    #[test]
    fn test_is_marketplace_auto_update_default_for_third_party() {
        assert!(!is_marketplace_auto_update("my-marketplace", None));
    }

    #[test]
    fn test_is_marketplace_auto_update_explicit_overrides() {
        assert!(!is_marketplace_auto_update(
            "claude-code-marketplace",
            Some(false)
        ));
        assert!(is_marketplace_auto_update("my-marketplace", Some(true)));
    }

    #[test]
    fn test_validate_official_name_source_non_reserved() {
        let source = MarketplaceSourceKind::Github {
            repo: "someone/repo".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(validate_official_name_source("my-marketplace", &source).is_ok());
    }

    #[test]
    fn test_validate_official_name_source_github_official() {
        let source = MarketplaceSourceKind::Github {
            repo: "anthropics/claude-plugins".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(validate_official_name_source("claude-code-marketplace", &source).is_ok());
    }

    #[test]
    fn test_validate_official_name_source_github_unofficial() {
        let source = MarketplaceSourceKind::Github {
            repo: "someone/claude-plugins".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(validate_official_name_source("claude-code-marketplace", &source).is_err());
    }

    #[test]
    fn test_validate_official_name_source_git_https_official() {
        let source = MarketplaceSourceKind::Git {
            url: "https://github.com/anthropics/claude-plugins".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(validate_official_name_source("claude-code-marketplace", &source).is_ok());
    }

    #[test]
    fn test_validate_official_name_source_git_ssh_official() {
        let source = MarketplaceSourceKind::Git {
            url: "git@github.com:anthropics/claude-plugins".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(validate_official_name_source("claude-code-marketplace", &source).is_ok());
    }

    #[test]
    fn test_validate_official_name_source_rejects_non_github() {
        let source = MarketplaceSourceKind::Url {
            url: "https://example.com/marketplace.json".into(),
            headers: None,
        };
        assert!(validate_official_name_source("claude-code-marketplace", &source).is_err());
    }

    #[test]
    fn test_is_local_plugin_source() {
        let local = PluginSourceSchema::RelativePath("./my-plugin".into());
        let remote = PluginSourceSchema::Github {
            repo: "org/repo".into(),
            r#ref: None,
            sha: None,
        };
        assert!(is_local_plugin_source(&local));
        assert!(!is_local_plugin_source(&remote));
    }

    #[test]
    fn test_is_local_marketplace_source() {
        let file = MarketplaceSourceKind::File {
            path: "/path/to/marketplace.json".into(),
        };
        let dir = MarketplaceSourceKind::Directory {
            path: "/path/to/dir".into(),
        };
        let github = MarketplaceSourceKind::Github {
            repo: "org/repo".into(),
            r#ref: None,
            path: None,
            sparse_paths: None,
        };
        assert!(is_local_marketplace_source(&file));
        assert!(is_local_marketplace_source(&dir));
        assert!(!is_local_marketplace_source(&github));
    }

    #[test]
    fn test_plugin_manifest_schema_serde_roundtrip() {
        let manifest = PluginManifestSchema {
            name: "test-plugin".into(),
            version: Some("1.0.0".into()),
            description: Some("A test plugin".into()),
            author: Some(PluginAuthorSchema {
                name: "Test Author".into(),
                email: Some("test@example.com".into()),
                url: None,
            }),
            homepage: None,
            repository: None,
            license: Some("MIT".into()),
            keywords: Some(vec!["test".into()]),
            dependencies: None,
            hooks: None,
            commands: None,
            agents: None,
            skills: None,
            output_styles: None,
            channels: None,
            mcp_servers: None,
            lsp_servers: None,
            settings: None,
            user_config: None,
        };
        let json = serde_json::to_string(&manifest).expect("serialize");
        let deserialized: PluginManifestSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(manifest, deserialized);
    }

    #[test]
    fn test_marketplace_source_kind_serde_roundtrip() {
        let source = MarketplaceSourceKind::Github {
            repo: "anthropics/claude-plugins".into(),
            r#ref: Some("main".into()),
            path: None,
            sparse_paths: None,
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let deserialized: MarketplaceSourceKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(source, deserialized);
    }

    #[test]
    fn test_marketplace_source_kind_tagged_serde() {
        let json = r#"{"source":"github","repo":"org/repo"}"#;
        let parsed: MarketplaceSourceKind = serde_json::from_str(json).expect("parse github");
        assert!(matches!(parsed, MarketplaceSourceKind::Github { repo, .. } if repo == "org/repo"));
    }

    #[test]
    fn test_installed_plugin_schema_serde() {
        let plugin = InstalledPluginSchema {
            version: "1.0.0".into(),
            installed_at: "2024-01-15T10:30:00Z".into(),
            last_updated: None,
            install_path: "/home/user/.claude/plugins/test".into(),
            git_commit_sha: None,
        };
        let json = serde_json::to_string(&plugin).expect("serialize");
        let back: InstalledPluginSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plugin, back);
    }

    #[test]
    fn test_plugin_scope_serde() {
        let json = "\"user\"";
        let scope: PluginScope = serde_json::from_str(json).expect("parse");
        assert_eq!(scope, PluginScope::User);

        let json = "\"managed\"";
        let scope: PluginScope = serde_json::from_str(json).expect("parse");
        assert_eq!(scope, PluginScope::Managed);
    }

    #[test]
    fn test_settings_plugin_entry_simple() {
        let json = r#""code-formatter@anthropic-tools""#;
        let entry: SettingsPluginEntrySchema = serde_json::from_str(json).expect("parse");
        assert!(
            matches!(entry, SettingsPluginEntrySchema::Simple(s) if s == "code-formatter@anthropic-tools")
        );
    }

    #[test]
    fn test_settings_plugin_entry_extended() {
        let json = r#"{"id":"formatter@tools","version":"^2.0.0","required":true}"#;
        let entry: SettingsPluginEntrySchema = serde_json::from_str(json).expect("parse");
        match entry {
            SettingsPluginEntrySchema::Extended(ext) => {
                assert_eq!(ext.id, "formatter@tools");
                assert_eq!(ext.version.as_deref(), Some("^2.0.0"));
                assert_eq!(ext.required, Some(true));
            }
            SettingsPluginEntrySchema::Simple(_) => panic!("expected extended"),
        }
    }

    #[test]
    fn test_command_metadata_schema() {
        let meta = CommandMetadataSchema {
            source: Some("./README.md".into()),
            content: None,
            description: Some("About command".into()),
            argument_hint: None,
            model: None,
            allowed_tools: None,
        };
        let json = serde_json::to_string(&meta).expect("serialize");
        let back: CommandMetadataSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(meta, back);
    }

    #[test]
    fn test_known_marketplace_schema_serde() {
        let km = KnownMarketplaceSchema {
            source: MarketplaceSourceKind::Github {
                repo: "anthropics/claude-plugins".into(),
                r#ref: None,
                path: None,
                sparse_paths: None,
            },
            install_location: "/home/user/.claude/plugins/cached/marketplaces/test".into(),
            last_updated: "2024-01-15T10:30:00Z".into(),
            auto_update: Some(true),
        };
        let json = serde_json::to_string(&km).expect("serialize");
        let back: KnownMarketplaceSchema = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(km, back);
    }
}
