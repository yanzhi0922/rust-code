//! Plugin version calculation and semver parsing.
//!
//! Rust equivalents of `pluginVersioning.ts`. Provides semantic version
//! parsing, comparison, range matching, and version resolution from
//! plugin manifests and git sources.

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during version parsing or comparison.
#[derive(Debug, Error)]
pub enum VersionError {
    /// The version string could not be parsed as valid semver.
    #[error("invalid semver: {0}")]
    InvalidSemver(String),
    /// The version range string could not be parsed.
    #[error("invalid version range: {0}")]
    InvalidRange(String),
}

// ---------------------------------------------------------------------------
// Pre-release identifier
// ---------------------------------------------------------------------------

/// A single pre-release identifier component (e.g., `"alpha"`, `"1"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreReleaseIdentifier {
    /// An alphanumeric identifier (e.g., `"alpha"`, `"beta"`).
    Alpha(String),
    /// A numeric identifier (e.g., `1` in `1.0.0-alpha.1`).
    Numeric(u64),
}

impl PartialOrd for PreReleaseIdentifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PreReleaseIdentifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            (Self::Numeric(_), Self::Alpha(_)) => Ordering::Less,
            (Self::Alpha(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Alpha(a), Self::Alpha(b)) => a.cmp(b),
        }
    }
}

// ---------------------------------------------------------------------------
// PluginVersion
// ---------------------------------------------------------------------------

/// A parsed semantic version (major.minor.patch with optional pre-release
/// and build metadata).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginVersion {
    /// Major version component.
    pub major: u64,
    /// Minor version component.
    pub minor: u64,
    /// Patch version component.
    pub patch: u64,
    /// Pre-release identifiers (e.g., `["alpha", Numeric(1)]`).
    #[serde(default)]
    pub pre_release: Vec<PreReleaseIdentifier>,
    /// Build metadata (e.g., `"001"` in `1.0.0+001`).
    #[serde(default)]
    pub build: Option<String>,
}

impl PluginVersion {
    /// Create a new version from major, minor, patch.
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre_release: Vec::new(),
            build: None,
        }
    }

    /// Create a version with pre-release identifiers.
    pub fn with_pre_release(mut self, pre: Vec<PreReleaseIdentifier>) -> Self {
        self.pre_release = pre;
        self
    }

    /// Create a version with build metadata.
    pub fn with_build(mut self, build: impl Into<String>) -> Self {
        self.build = Some(build.into());
        self
    }
}

impl fmt::Display for PluginVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre_release.is_empty() {
            write!(f, "-")?;
            let mut first = true;
            for id in &self.pre_release {
                if !first {
                    write!(f, ".")?;
                }
                match id {
                    PreReleaseIdentifier::Alpha(s) => write!(f, "{s}")?,
                    PreReleaseIdentifier::Numeric(n) => write!(f, "{n}")?,
                }
                first = false;
            }
        }
        if let Some(ref b) = self.build {
            write!(f, "+{b}")?;
        }
        Ok(())
    }
}

impl PartialOrd for PluginVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PluginVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare major.minor.patch
        match self
            .major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
        {
            Ordering::Equal => {}
            ord => return ord,
        }

        // Pre-release versions have lower precedence than release.
        // A version without pre-release is greater than one with it.
        match (self.pre_release.is_empty(), other.pre_release.is_empty()) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (true, true) => Ordering::Equal,
            (false, false) => {
                // Compare identifier by identifier
                let mut it_a = self.pre_release.iter();
                let mut it_b = other.pre_release.iter();
                loop {
                    match (it_a.next(), it_b.next()) {
                        (Some(a), Some(b)) => match a.cmp(b) {
                            Ordering::Equal => continue,
                            ord => return ord,
                        },
                        (Some(_), None) => return Ordering::Greater,
                        (None, Some(_)) => return Ordering::Less,
                        (None, None) => return Ordering::Equal,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a semantic version string (e.g., `"1.2.3"`, `"1.0.0-alpha.1"`).
///
/// Supports the full semver 2.0 spec: `MAJOR.MINOR.PATCH[-prerelease][+build]`.
pub fn parse_version(input: &str) -> Result<PluginVersion, VersionError> {
    let input = input.trim();

    // Strip leading 'v' if present
    let input = input.strip_prefix('v').unwrap_or(input);

    // Separate build metadata
    let (version_part, build) = match input.split_once('+') {
        Some((v, b)) => (v, Some(b.to_string())),
        None => (input, None),
    };

    // Separate pre-release
    let (core, pre_release) = match version_part.split_once('-') {
        Some((c, p)) => (c, parse_pre_release(p)?),
        None => (version_part, Vec::new()),
    };

    // Parse core MAJOR.MINOR.PATCH
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(VersionError::InvalidSemver(format!(
            "expected MAJOR.MINOR.PATCH, got: {input}"
        )));
    }

    let major = parse_u64_part(parts[0], "major", input)?;
    let minor = parse_u64_part(parts[1], "minor", input)?;
    let patch = parse_u64_part(parts[2], "patch", input)?;

    Ok(PluginVersion {
        major,
        minor,
        patch,
        pre_release,
        build,
    })
}

fn parse_u64_part(s: &str, field: &str, full: &str) -> Result<u64, VersionError> {
    s.parse::<u64>().map_err(|_| {
        VersionError::InvalidSemver(format!("invalid {field} version component in: {full}"))
    })
}

fn parse_pre_release(input: &str) -> Result<Vec<PreReleaseIdentifier>, VersionError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    input
        .split('.')
        .map(|ident| {
            if let Ok(n) = ident.parse::<u64>() {
                Ok(PreReleaseIdentifier::Numeric(n))
            } else {
                // Validate alphanumeric + hyphens
                if ident.is_empty() || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    Err(VersionError::InvalidSemver(format!(
                        "invalid pre-release identifier: {ident}"
                    )))
                } else {
                    Ok(PreReleaseIdentifier::Alpha(ident.to_string()))
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Version range
// ---------------------------------------------------------------------------

/// A comparator in a version range (e.g., `>=1.0.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparator {
    /// Exact match: `=1.0.0`
    Exact(PluginVersion),
    /// Greater than: `>1.0.0`
    GreaterThan(PluginVersion),
    /// Greater than or equal: `>=1.0.0`
    GreaterOrEqual(PluginVersion),
    /// Less than: `<1.0.0`
    LessThan(PluginVersion),
    /// Less than or equal: `<=1.0.0`
    LessOrEqual(PluginVersion),
    /// Compatible with minor: `^1.2.3` → `>=1.2.3, <2.0.0`
    Caret(PluginVersion),
    /// Compatible with patch: `~1.2.3` → `>=1.2.3, <1.3.0`
    Tilde(PluginVersion),
}

/// A version range is a conjunction (AND) of comparators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRange {
    comparators: Vec<Comparator>,
}

impl VersionRange {
    /// Create a range from a single comparator.
    pub fn single(comp: Comparator) -> Self {
        Self {
            comparators: vec![comp],
        }
    }

    /// Create a range from multiple comparators (all must match).
    pub fn all(comparators: Vec<Comparator>) -> Self {
        Self { comparators }
    }

    /// Check if a version satisfies this range.
    pub fn matches(&self, version: &PluginVersion) -> bool {
        self.comparators
            .iter()
            .all(|comp| comp_matches(comp, version))
    }
}

fn comp_matches(comp: &Comparator, version: &PluginVersion) -> bool {
    match comp {
        Comparator::Exact(v) => version == v,
        Comparator::GreaterThan(v) => version > v,
        Comparator::GreaterOrEqual(v) => version >= v,
        Comparator::LessThan(v) => version < v,
        Comparator::LessOrEqual(v) => version <= v,
        Comparator::Caret(v) => {
            if version < v {
                return false;
            }
            // ^0.0.z → exact match on patch
            // ^0.y.z → y must match, any z >= v.patch
            // ^x.y.z → x must match, any y.z
            if v.major == 0 && v.minor == 0 {
                version.patch == v.patch && version.major == 0 && version.minor == 0
            } else if v.major == 0 {
                version.major == 0 && version.minor == v.minor && version.patch >= v.patch
            } else {
                version.major == v.major
                    && (version.minor > v.minor
                        || (version.minor == v.minor && version.patch >= v.patch))
            }
        }
        Comparator::Tilde(v) => {
            if version < v {
                return false;
            }
            // ~x.y.z → x.y must match, any z >= v.patch
            version.major == v.major && version.minor == v.minor && version.patch >= v.patch
        }
    }
}

/// Parse a version range string.
///
/// Supported formats:
/// - `"^1.2.3"` — compatible with minor
/// - `"~1.2.3"` — compatible with patch
/// - `">=1.0.0"` — greater or equal
/// - `">1.0.0"` — greater than
/// - `"<=1.0.0"` — less or equal
/// - `"<1.0.0"` — less than
/// - `"=1.0.0"` — exact
/// - `"1.0.0"` — exact (shorthand)
/// - `">=1.0.0 <2.0.0"` — range (space-separated AND)
pub fn parse_version_range(input: &str) -> Result<VersionRange, VersionError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(VersionError::InvalidRange("empty range string".into()));
    }

    // Split on whitespace to handle compound ranges like ">=1.0.0 <2.0.0"
    let parts: Vec<&str> = input.split_whitespace().collect();
    let mut comparators = Vec::with_capacity(parts.len());

    for part in parts {
        let comp = if let Some(rest) = part.strip_prefix("^") {
            Comparator::Caret(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix("~") {
            Comparator::Tilde(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix(">=") {
            Comparator::GreaterOrEqual(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix(">") {
            Comparator::GreaterThan(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix("<=") {
            Comparator::LessOrEqual(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix("<") {
            Comparator::LessThan(parse_version(rest)?)
        } else if let Some(rest) = part.strip_prefix("=") {
            Comparator::Exact(parse_version(rest)?)
        } else {
            // Bare version = exact match
            Comparator::Exact(parse_version(part)?)
        };
        comparators.push(comp);
    }

    Ok(VersionRange { comparators })
}

/// Check if a version satisfies a range string.
///
/// Convenience wrapper around [`parse_version_range`] + [`VersionRange::matches`].
pub fn satisfies_version_range(version: &PluginVersion, range: &str) -> bool {
    match parse_version_range(range) {
        Ok(r) => r.matches(version),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Version resolution helpers
// ---------------------------------------------------------------------------

/// Extract the version from a versioned cache path.
///
/// Given a path like `~/.claude/plugins/cache/marketplace/plugin/1.0.0`,
/// extracts and returns `"1.0.0"`.
pub fn get_version_from_path(install_path: &Path) -> Option<String> {
    let components: Vec<&std::ffi::OsStr> = install_path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Find "cache" index preceded by "plugins"
    let cache_idx = components.iter().enumerate().find_map(|(i, c)| {
        if c == &std::ffi::OsStr::new("cache")
            && i > 0
            && components[i - 1] == std::ffi::OsStr::new("plugins")
        {
            Some(i)
        } else {
            None
        }
    })?;

    // Versioned path has 3 components after "cache": marketplace/plugin/version
    let after_cache: Vec<&std::ffi::OsStr> =
        components.iter().skip(cache_idx + 1).copied().collect();
    if after_cache.len() >= 3 {
        after_cache[2].to_str().map(|s| s.to_string())
    } else {
        None
    }
}

/// Check if a path follows the versioned plugin cache structure.
pub fn is_versioned_path(path: &Path) -> bool {
    get_version_from_path(path).is_some()
}

/// Calculate the effective version for a plugin.
///
/// Priority order:
/// 1. Manifest version from `plugin.json`
/// 2. Provided version (e.g., from marketplace entry)
/// 3. Git commit SHA (shortened to 12 chars)
/// 4. `"unknown"` fallback
pub fn calculate_plugin_version(
    manifest_version: Option<&str>,
    provided_version: Option<&str>,
    git_sha: Option<&str>,
) -> String {
    if let Some(v) = manifest_version {
        return v.to_string();
    }
    if let Some(v) = provided_version {
        return v.to_string();
    }
    if let Some(sha) = git_sha {
        let end = sha.len().min(12);
        return sha[..end].to_string();
    }
    "unknown".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- parse_version --

    #[test]
    fn parse_simple_version() {
        let v = parse_version("1.2.3").expect("should parse");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert!(v.pre_release.is_empty());
        assert!(v.build.is_none());
    }

    #[test]
    fn parse_version_with_v_prefix() {
        let v = parse_version("v1.0.0").expect("should parse");
        assert_eq!(v.major, 1);
    }

    #[test]
    fn parse_version_with_prerelease() {
        let v = parse_version("1.0.0-alpha.1").expect("should parse");
        assert_eq!(v.pre_release.len(), 2);
        assert_eq!(
            v.pre_release[0],
            PreReleaseIdentifier::Alpha("alpha".into())
        );
        assert_eq!(v.pre_release[1], PreReleaseIdentifier::Numeric(1));
    }

    #[test]
    fn parse_version_with_build() {
        let v = parse_version("1.0.0+001").expect("should parse");
        assert_eq!(v.build.as_deref(), Some("001"));
    }

    #[test]
    fn parse_version_with_prerelease_and_build() {
        let v = parse_version("1.0.0-beta.2+build.123").expect("should parse");
        assert_eq!(v.pre_release.len(), 2);
        assert_eq!(v.build.as_deref(), Some("build.123"));
    }

    #[test]
    fn parse_version_rejects_invalid() {
        assert!(parse_version("not.a.version").is_err());
        assert!(parse_version("1.2").is_err());
        assert!(parse_version("").is_err());
    }

    // -- Display --

    #[test]
    fn display_simple() {
        let v = PluginVersion::new(1, 2, 3);
        assert_eq!(format!("{v}"), "1.2.3");
    }

    #[test]
    fn display_with_prerelease() {
        let v = PluginVersion::new(1, 0, 0).with_pre_release(vec![
            PreReleaseIdentifier::Alpha("alpha".into()),
            PreReleaseIdentifier::Numeric(1),
        ]);
        assert_eq!(format!("{v}"), "1.0.0-alpha.1");
    }

    #[test]
    fn display_with_build() {
        let v = PluginVersion::new(2, 0, 0).with_build("001");
        assert_eq!(format!("{v}"), "2.0.0+001");
    }

    // -- Ordering --

    #[test]
    fn version_ordering() {
        let v1 = PluginVersion::new(1, 0, 0);
        let v2 = PluginVersion::new(1, 0, 1);
        let v3 = PluginVersion::new(1, 1, 0);
        let v4 = PluginVersion::new(2, 0, 0);

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
    }

    #[test]
    fn prerelease_lower_than_release() {
        let release = PluginVersion::new(1, 0, 0);
        let pre = PluginVersion::new(1, 0, 0)
            .with_pre_release(vec![PreReleaseIdentifier::Alpha("alpha".into())]);
        assert!(pre < release);
    }

    #[test]
    fn numeric_prerelease_lower_than_alpha() {
        let num =
            PluginVersion::new(1, 0, 0).with_pre_release(vec![PreReleaseIdentifier::Numeric(1)]);
        let alpha = PluginVersion::new(1, 0, 0)
            .with_pre_release(vec![PreReleaseIdentifier::Alpha("alpha".into())]);
        assert!(num < alpha);
    }

    // -- VersionRange --

    #[test]
    fn exact_match() {
        let range = parse_version_range("1.2.3").expect("parse");
        let v = PluginVersion::new(1, 2, 3);
        assert!(range.matches(&v));
        let v2 = PluginVersion::new(1, 2, 4);
        assert!(!range.matches(&v2));
    }

    #[test]
    fn caret_range() {
        let range = parse_version_range("^1.2.3").expect("parse");
        assert!(range.matches(&PluginVersion::new(1, 2, 3)));
        assert!(range.matches(&PluginVersion::new(1, 2, 4)));
        assert!(range.matches(&PluginVersion::new(1, 3, 0)));
        assert!(!range.matches(&PluginVersion::new(2, 0, 0)));
        assert!(!range.matches(&PluginVersion::new(1, 2, 2)));
    }

    #[test]
    fn caret_range_zero_major() {
        let range = parse_version_range("^0.2.3").expect("parse");
        assert!(range.matches(&PluginVersion::new(0, 2, 3)));
        assert!(range.matches(&PluginVersion::new(0, 2, 5)));
        assert!(!range.matches(&PluginVersion::new(0, 3, 0)));
        assert!(!range.matches(&PluginVersion::new(1, 0, 0)));
    }

    #[test]
    fn caret_range_zero_minor() {
        let range = parse_version_range("^0.0.3").expect("parse");
        assert!(range.matches(&PluginVersion::new(0, 0, 3)));
        assert!(!range.matches(&PluginVersion::new(0, 0, 4)));
    }

    #[test]
    fn tilde_range() {
        let range = parse_version_range("~1.2.3").expect("parse");
        assert!(range.matches(&PluginVersion::new(1, 2, 3)));
        assert!(range.matches(&PluginVersion::new(1, 2, 9)));
        assert!(!range.matches(&PluginVersion::new(1, 3, 0)));
    }

    #[test]
    fn gte_range() {
        let range = parse_version_range(">=1.0.0").expect("parse");
        assert!(range.matches(&PluginVersion::new(1, 0, 0)));
        assert!(range.matches(&PluginVersion::new(2, 0, 0)));
        assert!(!range.matches(&PluginVersion::new(0, 9, 9)));
    }

    #[test]
    fn compound_range() {
        let range = parse_version_range(">=1.0.0 <2.0.0").expect("parse");
        assert!(range.matches(&PluginVersion::new(1, 0, 0)));
        assert!(range.matches(&PluginVersion::new(1, 9, 9)));
        assert!(!range.matches(&PluginVersion::new(2, 0, 0)));
        assert!(!range.matches(&PluginVersion::new(0, 9, 0)));
    }

    #[test]
    fn satisfies_version_range_convenience() {
        assert!(satisfies_version_range(
            &PluginVersion::new(1, 5, 0),
            "^1.0.0"
        ));
        assert!(!satisfies_version_range(
            &PluginVersion::new(2, 0, 0),
            "^1.0.0"
        ));
        assert!(!satisfies_version_range(
            &PluginVersion::new(1, 0, 0),
            "invalid[range"
        ));
    }

    // -- calculate_plugin_version --

    #[test]
    fn calculate_prefers_manifest() {
        assert_eq!(
            calculate_plugin_version(Some("1.0.0"), Some("2.0.0"), Some("abc123def456")),
            "1.0.0"
        );
    }

    #[test]
    fn calculate_uses_provided_when_no_manifest() {
        assert_eq!(
            calculate_plugin_version(None, Some("2.0.0"), Some("abc123def456")),
            "2.0.0"
        );
    }

    #[test]
    fn calculate_uses_git_sha_when_no_others() {
        assert_eq!(
            calculate_plugin_version(None, None, Some("abc123def456789")),
            "abc123def456"
        );
    }

    #[test]
    fn calculate_falls_back_to_unknown() {
        assert_eq!(calculate_plugin_version(None, None, None), "unknown");
    }

    // -- get_version_from_path --

    #[test]
    fn extracts_version_from_path() {
        let path = PathBuf::from("/home/user/.claude/plugins/cache/marketplace/plugin/1.0.0");
        assert_eq!(get_version_from_path(&path), Some("1.0.0".to_string()));
    }

    #[test]
    fn returns_none_for_non_versioned_path() {
        let path = PathBuf::from("/home/user/.claude/plugins/installed");
        assert_eq!(get_version_from_path(&path), None);
    }

    #[test]
    fn is_versioned_path_checks() {
        let versioned = PathBuf::from("/plugins/cache/m/p/1.0.0");
        let not_versioned = PathBuf::from("/plugins/installed");
        assert!(is_versioned_path(&versioned));
        assert!(!is_versioned_path(&not_versioned));
    }
}
