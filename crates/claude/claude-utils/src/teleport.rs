//! Session teleport utilities.
//!
//! Provides types and functions for creating session bundles that can be
//! transferred between environments, including Git bundle creation and
//! session title generation.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TeleportConfig
// ---------------------------------------------------------------------------

/// Configuration for session teleportation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeleportConfig {
    /// Whether to include git history in the bundle.
    #[serde(default = "default_true")]
    pub include_git_history: bool,
    /// Maximum bundle size in megabytes.
    #[serde(default = "default_max_size")]
    pub max_bundle_size_mb: u32,
    /// Whether to include memory files.
    #[serde(default = "default_true")]
    pub include_memory: bool,
    /// Whether to compress the bundle.
    #[serde(default = "default_true")]
    pub compress: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_size() -> u32 {
    100
}

impl Default for TeleportConfig {
    fn default() -> Self {
        Self {
            include_git_history: true,
            max_bundle_size_mb: 100,
            include_memory: true,
            compress: true,
        }
    }
}

impl TeleportConfig {
    /// Create a new teleport config with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a minimal config without git history.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            include_git_history: false,
            max_bundle_size_mb: 10,
            include_memory: false,
            compress: true,
        }
    }

    /// Check if a bundle size is within limits.
    #[must_use]
    pub fn is_within_size_limit(&self, size_bytes: u64) -> bool {
        let max_bytes = u64::from(self.max_bundle_size_mb) * 1024 * 1024;
        size_bytes <= max_bytes
    }
}

// ---------------------------------------------------------------------------
// TeleportStatus
// ---------------------------------------------------------------------------

/// The status of a teleport operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeleportStatus {
    /// The teleport is being prepared.
    Preparing,
    /// Creating the git bundle.
    Bundling,
    /// Compressing the bundle.
    Compressing,
    /// Uploading/transferring the bundle.
    Transferring,
    /// The teleport completed successfully.
    Completed,
    /// The teleport failed.
    Failed,
}

impl TeleportStatus {
    /// Return a human-readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing bundle",
            Self::Bundling => "Creating git bundle",
            Self::Compressing => "Compressing",
            Self::Transferring => "Transferring",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    /// Check if this status represents a terminal state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl std::fmt::Display for TeleportStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------
// Git bundle creation
// ---------------------------------------------------------------------------

/// Result of a git bundle creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBundleResult {
    /// Path to the created bundle file.
    pub bundle_path: String,
    /// Size of the bundle in bytes.
    pub size_bytes: u64,
    /// Number of commits included.
    pub commit_count: usize,
}

/// Create a git bundle specification (command-line arguments).
///
/// This generates the arguments for `git bundle create` without actually
/// running the command, allowing the caller to execute it in their
/// preferred manner.
///
/// # Arguments
///
/// * `output_path` — The path for the output bundle file.
/// * `base_ref` — Optional base reference (bundles commits since this ref).
///
/// # Returns
///
/// A vector of command-line arguments for `git bundle create`.
#[must_use]
pub fn create_git_bundle_args(output_path: &str, base_ref: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "bundle".to_string(),
        "create".to_string(),
        output_path.to_string(),
    ];

    if let Some(base) = base_ref {
        args.push(format!("{base}..HEAD"));
    } else {
        args.push("--all".to_string());
    }

    args
}

// ---------------------------------------------------------------------------
// Session title generation
// ---------------------------------------------------------------------------

/// Generate a session title from the first user message.
///
/// Takes the first line or first N characters of the user's initial message
/// and creates a concise title.
///
/// # Arguments
///
/// * `first_message` — The first user message in the session.
/// * `max_length` — Maximum title length.
///
/// # Returns
///
/// A generated session title.
#[must_use]
pub fn generate_session_title(first_message: &str, max_length: usize) -> String {
    // Take the first non-empty line.
    let first_line = first_message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");

    let title = first_line.trim();

    if title.is_empty() {
        return "Untitled Session".to_string();
    }

    if title.len() <= max_length {
        return title.to_string();
    }

    // Truncate at word boundary if possible.
    let truncated = &title[..max_length];
    if let Some(last_space) = truncated.rfind(' ') {
        format!("{}...", &title[..last_space])
    } else {
        format!("{truncated}...")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- TeleportConfig ---

    #[test]
    fn teleport_config_default() {
        let config = TeleportConfig::default();
        assert!(config.include_git_history);
        assert_eq!(config.max_bundle_size_mb, 100);
        assert!(config.include_memory);
        assert!(config.compress);
    }

    #[test]
    fn teleport_config_new() {
        let config = TeleportConfig::new();
        assert_eq!(config, TeleportConfig::default());
    }

    #[test]
    fn teleport_config_minimal() {
        let config = TeleportConfig::minimal();
        assert!(!config.include_git_history);
        assert_eq!(config.max_bundle_size_mb, 10);
        assert!(!config.include_memory);
    }

    #[test]
    fn teleport_config_is_within_size_limit() {
        let config = TeleportConfig::default();
        assert!(config.is_within_size_limit(50 * 1024 * 1024));
        assert!(!config.is_within_size_limit(200 * 1024 * 1024));
    }

    #[test]
    fn teleport_config_serialization_roundtrip() {
        let config = TeleportConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: TeleportConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    // --- TeleportStatus ---

    #[test]
    fn teleport_status_labels() {
        assert_eq!(TeleportStatus::Preparing.label(), "Preparing bundle");
        assert_eq!(TeleportStatus::Bundling.label(), "Creating git bundle");
        assert_eq!(TeleportStatus::Completed.label(), "Completed");
        assert_eq!(TeleportStatus::Failed.label(), "Failed");
    }

    #[test]
    fn teleport_status_is_terminal() {
        assert!(!TeleportStatus::Preparing.is_terminal());
        assert!(!TeleportStatus::Bundling.is_terminal());
        assert!(TeleportStatus::Completed.is_terminal());
        assert!(TeleportStatus::Failed.is_terminal());
    }

    #[test]
    fn teleport_status_display() {
        assert_eq!(TeleportStatus::Preparing.to_string(), "Preparing bundle");
    }

    #[test]
    fn teleport_status_serialization_roundtrip() {
        let status = TeleportStatus::Transferring;
        let json = serde_json::to_string(&status).expect("serialize");
        let deserialized: TeleportStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, deserialized);
    }

    // --- create_git_bundle_args ---

    #[test]
    fn create_git_bundle_args_all() {
        let args = create_git_bundle_args("/tmp/bundle.gitbundle", None);
        assert_eq!(
            args,
            vec!["bundle", "create", "/tmp/bundle.gitbundle", "--all"]
        );
    }

    #[test]
    fn create_git_bundle_args_with_base() {
        let args = create_git_bundle_args("/tmp/bundle.gitbundle", Some("main"));
        assert_eq!(
            args,
            vec!["bundle", "create", "/tmp/bundle.gitbundle", "main..HEAD"]
        );
    }

    // --- generate_session_title ---

    #[test]
    fn generate_title_short() {
        let title = generate_session_title("Fix the bug", 50);
        assert_eq!(title, "Fix the bug");
    }

    #[test]
    fn generate_title_long() {
        let long_msg = "This is a very long message that exceeds the maximum length limit for session titles and should be truncated properly";
        let title = generate_session_title(long_msg, 30);
        assert!(title.len() <= 33); // 30 + "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn generate_title_multiline() {
        let msg = "First line\nSecond line\nThird line";
        let title = generate_session_title(msg, 50);
        assert_eq!(title, "First line");
    }

    #[test]
    fn generate_title_empty() {
        let title = generate_session_title("", 50);
        assert_eq!(title, "Untitled Session");
    }

    #[test]
    fn generate_title_whitespace_only() {
        let title = generate_session_title("   \n  \n  ", 50);
        assert_eq!(title, "Untitled Session");
    }

    #[test]
    fn generate_title_exact_length() {
        let msg = "1234567890";
        let title = generate_session_title(msg, 10);
        assert_eq!(title, "1234567890");
    }

    // --- GitBundleResult ---

    #[test]
    fn git_bundle_result_fields() {
        let result = GitBundleResult {
            bundle_path: "/tmp/bundle.gitbundle".to_string(),
            size_bytes: 1024,
            commit_count: 5,
        };
        assert_eq!(result.bundle_path, "/tmp/bundle.gitbundle");
        assert_eq!(result.size_bytes, 1024);
        assert_eq!(result.commit_count, 5);
    }
}
