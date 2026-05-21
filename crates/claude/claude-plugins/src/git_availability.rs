//! Git availability detection.
//!
//! Provides [`GitAvailability`] for checking whether git is installed and
//! meets the minimum version requirement. Git is required for installing
//! GitHub-based marketplaces.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;

/// Minimum git version required for plugin operations.
///
/// Git 2.0+ is required for features like `--no-tags`, sparse checkout
/// improvements, and other modern git capabilities.
pub const MIN_GIT_VERSION: (u32, u32, u32) = (2, 0, 0);

/// Regex to extract version from `git --version` output.
/// Matches patterns like "git version 2.43.0" or "git version 2.43.0.windows.1".
static GIT_VERSION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"git version (\d+)\.(\d+)(?:\.(\d+))?").expect("GIT_VERSION_RE is a valid regex")
});

/// Cached git availability state.
/// `None` = not yet checked, `Some(true)` = available, `Some(false)` = unavailable.
static GIT_AVAILABLE: AtomicBool = AtomicBool::new(false);
static GIT_CHECKED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// GitAvailability
// ---------------------------------------------------------------------------

/// Result of checking git availability on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAvailability {
    /// Git is available and meets minimum version requirements.
    Available,
    /// Git is not found on the system PATH.
    NotFound,
    /// Git is found but the version is too low.
    VersionTooLow {
        /// The detected version.
        detected: (u32, u32, u32),
        /// The minimum required version.
        minimum: (u32, u32, u32),
    },
}

impl std::fmt::Display for GitAvailability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitAvailability::Available => write!(f, "git is available"),
            GitAvailability::NotFound => write!(f, "git not found on PATH"),
            GitAvailability::VersionTooLow { detected, minimum } => {
                write!(
                    f,
                    "git version too low: detected {}.{}.{}, minimum {}.{}.{}",
                    detected.0, detected.1, detected.2, minimum.0, minimum.1, minimum.2,
                )
            }
        }
    }
}

impl std::error::Error for GitAvailability {}

// ---------------------------------------------------------------------------
// Version parsing
// ---------------------------------------------------------------------------

/// A parsed semantic version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl GitVersion {
    /// Create a new version.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version meets or exceeds the minimum.
    pub fn meets_minimum(&self, minimum: (u32, u32, u32)) -> bool {
        (self.major, self.minor, self.patch) >= minimum
    }
}

impl FromStr for GitVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.trim().split('.').collect();
        if parts.len() < 2 {
            return Err(format!("Invalid version string: {s}"));
        }

        let major = parts[0]
            .parse::<u32>()
            .map_err(|e| format!("Invalid major version: {e}"))?;
        let minor = parts[1]
            .parse::<u32>()
            .map_err(|e| format!("Invalid minor version: {e}"))?;
        let patch = parts
            .get(2)
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);

        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

// ---------------------------------------------------------------------------
// Git availability check
// ---------------------------------------------------------------------------

/// Check if git is available on the system and meets minimum version
/// requirements.
///
/// This is the primary entry point for git availability checking.
/// Results are cached for the lifetime of the process.
pub async fn check_git_availability() -> GitAvailability {
    // Check cache
    if GIT_CHECKED.load(Ordering::Acquire) {
        return if GIT_AVAILABLE.load(Ordering::Acquire) {
            GitAvailability::Available
        } else {
            GitAvailability::NotFound
        };
    }

    let result = check_git_availability_inner().await;

    // Cache the result (simplified: only cache Available/NotFound)
    match result {
        GitAvailability::Available => {
            GIT_AVAILABLE.store(true, Ordering::Release);
            GIT_CHECKED.store(true, Ordering::Release);
        }
        GitAvailability::NotFound => {
            GIT_AVAILABLE.store(false, Ordering::Release);
            GIT_CHECKED.store(true, Ordering::Release);
        }
        GitAvailability::VersionTooLow { .. } => {
            GIT_AVAILABLE.store(false, Ordering::Release);
            GIT_CHECKED.store(true, Ordering::Release);
        }
    }

    result
}

/// Force the cached git availability to return `NotFound` for the rest of
/// the session.
///
/// Call this when a git invocation fails in a way that indicates the binary
/// exists on PATH but cannot actually run (e.g., macOS xcrun shim without
/// Xcode CLT).
pub fn mark_git_unavailable() {
    GIT_AVAILABLE.store(false, Ordering::Release);
    GIT_CHECKED.store(true, Ordering::Release);
}

/// Clear the git availability cache.
/// Used for testing purposes.
pub fn clear_git_availability_cache() {
    GIT_AVAILABLE.store(false, Ordering::Release);
    GIT_CHECKED.store(false, Ordering::Release);
}

/// Inner implementation of git availability check.
async fn check_git_availability_inner() -> GitAvailability {
    // Try to find git on PATH
    let git_path = match which_git().await {
        Some(path) => path,
        None => return GitAvailability::NotFound,
    };

    // Get version
    let version_str = match get_git_version(&git_path).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to get git version: {e}");
            return GitAvailability::NotFound;
        }
    };

    let version = match parse_git_version(&version_str) {
        Some(v) => v,
        None => {
            tracing::warn!("Failed to parse git version: {version_str}");
            // If we can't parse the version, assume it's available
            return GitAvailability::Available;
        }
    };

    if version.meets_minimum(MIN_GIT_VERSION) {
        GitAvailability::Available
    } else {
        GitAvailability::VersionTooLow {
            detected: (version.major, version.minor, version.patch),
            minimum: MIN_GIT_VERSION,
        }
    }
}

/// Find git on the system PATH.
async fn which_git() -> Option<String> {
    #[cfg(windows)]
    let git_name = "git.exe";
    #[cfg(not(windows))]
    let git_name = "git";

    // Try using `which` (Unix) or `where` (Windows)
    #[cfg(windows)]
    let output = tokio::process::Command::new("where")
        .arg(git_name)
        .output()
        .await
        .ok()?;

    #[cfg(not(windows))]
    let output = tokio::process::Command::new("which")
        .arg(git_name)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout);
    let first_line = path.lines().next()?;
    let trimmed = first_line.trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Get the git version string from `git --version`.
async fn get_git_version(git_path: &str) -> Result<String> {
    let output = tokio::process::Command::new(git_path)
        .arg("--version")
        .output()
        .await
        .context("Failed to execute git --version")?;

    if !output.status.success() {
        anyhow::bail!(
            "git --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    Ok(version_output.trim().to_string())
}

/// Parse the git version from output like "git version 2.43.0".
fn parse_git_version(output: &str) -> Option<GitVersion> {
    let caps = GIT_VERSION_RE.captures(output)?;
    let major = caps.get(1)?.as_str().parse::<u32>().ok()?;
    let minor = caps.get(2)?.as_str().parse::<u32>().ok()?;
    let patch = caps
        .get(3)
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(0);

    Some(GitVersion::new(major, minor, patch))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_version_from_str() {
        let v: GitVersion = "2.43.0".parse().expect("parse");
        assert_eq!(v, GitVersion::new(2, 43, 0));
    }

    #[test]
    fn test_git_version_from_str_two_parts() {
        let v: GitVersion = "2.43".parse().expect("parse");
        assert_eq!(v, GitVersion::new(2, 43, 0));
    }

    #[test]
    fn test_git_version_from_str_invalid() {
        assert!("abc".parse::<GitVersion>().is_err());
        assert!("1".parse::<GitVersion>().is_err());
    }

    #[test]
    fn test_git_version_display() {
        let v = GitVersion::new(2, 43, 0);
        assert_eq!(format!("{v}"), "2.43.0");
    }

    #[test]
    fn test_git_version_meets_minimum_exact() {
        let v = GitVersion::new(2, 0, 0);
        assert!(v.meets_minimum(MIN_GIT_VERSION));
    }

    #[test]
    fn test_git_version_meets_minimum_higher() {
        let v = GitVersion::new(2, 43, 0);
        assert!(v.meets_minimum(MIN_GIT_VERSION));
    }

    #[test]
    fn test_git_version_meets_minimum_lower() {
        let v = GitVersion::new(1, 99, 99);
        assert!(!v.meets_minimum(MIN_GIT_VERSION));
    }

    #[test]
    fn test_git_version_ordering() {
        let v1 = GitVersion::new(2, 0, 0);
        let v2 = GitVersion::new(2, 0, 1);
        let v3 = GitVersion::new(2, 1, 0);
        let v4 = GitVersion::new(3, 0, 0);

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
    }

    #[test]
    fn test_parse_git_version_standard() {
        let v = parse_git_version("git version 2.43.0").expect("parse");
        assert_eq!(v, GitVersion::new(2, 43, 0));
    }

    #[test]
    fn test_parse_git_version_windows() {
        let v = parse_git_version("git version 2.43.0.windows.1").expect("parse");
        assert_eq!(v, GitVersion::new(2, 43, 0));
    }

    #[test]
    fn test_parse_git_version_no_patch() {
        let v = parse_git_version("git version 2.43").expect("parse");
        assert_eq!(v, GitVersion::new(2, 43, 0));
    }

    #[test]
    fn test_parse_git_version_invalid() {
        assert!(parse_git_version("not a version").is_none());
        assert!(parse_git_version("").is_none());
    }

    #[test]
    fn test_git_availability_display() {
        assert_eq!(
            format!("{}", GitAvailability::Available),
            "git is available"
        );
        assert_eq!(
            format!("{}", GitAvailability::NotFound),
            "git not found on PATH"
        );
        assert_eq!(
            format!(
                "{}",
                GitAvailability::VersionTooLow {
                    detected: (1, 8, 0),
                    minimum: (2, 0, 0)
                }
            ),
            "git version too low: detected 1.8.0, minimum 2.0.0"
        );
    }

    #[test]
    fn test_clear_git_availability_cache() {
        // Should not panic
        clear_git_availability_cache();
        assert!(!GIT_CHECKED.load(Ordering::Acquire));
    }

    #[test]
    fn test_mark_git_unavailable() {
        clear_git_availability_cache();
        mark_git_unavailable();
        assert!(GIT_CHECKED.load(Ordering::Acquire));
        assert!(!GIT_AVAILABLE.load(Ordering::Acquire));
        clear_git_availability_cache();
    }

    #[test]
    fn test_min_git_version() {
        assert_eq!(MIN_GIT_VERSION, (2, 0, 0));
    }
}
