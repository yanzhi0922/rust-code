//! Path input validation and normalization for filesystem permission checks.
//!
//! This module mirrors the "raw path" stage of Claude Code's path validation:
//! strip superficial quoting, expand plain `~`, reject impossible inputs, and
//! flag path forms that require an explicit permission prompt instead of silent
//! auto-approval.

/// Result of coarse path validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathValidation {
    /// Path is structurally valid.
    Valid,
    /// Path is invalid and should be rejected before any permission prompt.
    Invalid(String),
}

const MAX_PATH_LEN: usize = 4096;

/// Remove surrounding quotes and expand a plain leading `~`.
#[must_use]
pub fn clean_path_input(path: &str) -> String {
    expand_tilde(strip_surrounding_quotes(path.trim()))
}

/// Validate a path for impossible inputs.
///
/// This intentionally stays conservative: it rejects inputs that cannot be
/// safely interpreted (`NUL`, overlong paths), while leaving prompt-worthy
/// cases like UNC paths and lexical traversal outside the working directory to
/// [`path_requires_manual_approval`].
#[must_use]
pub fn validate_path(path: &str) -> PathValidation {
    let cleaned = clean_path_input(path);

    if cleaned.contains('\0') {
        return PathValidation::Invalid("Path contains a null byte.".to_owned());
    }

    if cleaned.len() > MAX_PATH_LEN {
        return PathValidation::Invalid("Path exceeds the maximum supported length.".to_owned());
    }

    PathValidation::Valid
}

/// Return a prompt-worthy reason for paths that should require manual approval.
#[must_use]
pub fn path_requires_manual_approval(path: &str, write_semantics: bool) -> Option<String> {
    let cleaned = clean_path_input(path);

    if contains_vulnerable_unc_path(&cleaned) {
        return Some("UNC or network paths require manual approval.".to_owned());
    }

    if cleaned.starts_with('~') {
        return Some(
            "Tilde expansion variants (~user, ~+, ~-) require manual approval.".to_owned(),
        );
    }

    if contains_shell_expansion(&cleaned) {
        return Some("Shell expansion syntax in paths requires manual approval.".to_owned());
    }

    if write_semantics && has_glob_pattern(&cleaned) {
        return Some(
            "Glob patterns are not allowed for write operations; use an exact path.".to_owned(),
        );
    }

    if has_suspicious_windows_path_pattern(&cleaned) {
        return Some("Suspicious Windows path syntax requires manual approval.".to_owned());
    }

    None
}

/// Detect glob metacharacters in a path.
#[must_use]
pub fn has_glob_pattern(path: &str) -> bool {
    path.chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
}

/// Detect Windows path forms that are easy to mis-handle securely.
#[must_use]
pub fn has_suspicious_windows_path_pattern(path: &str) -> bool {
    let normalized = path.replace('/', "\\");

    if normalized.starts_with(r"\\?\")
        || normalized.starts_with(r"\\.\")
        || normalized.starts_with("//?/")
        || normalized.starts_with("//./")
    {
        return true;
    }

    if has_alternate_data_stream(&normalized) {
        return true;
    }

    let lowered = normalized.to_ascii_lowercase();
    for segment in lowered.split('\\') {
        if segment.is_empty() {
            continue;
        }

        if !matches!(segment, "." | "..") && (segment.ends_with('.') || segment.ends_with(' ')) {
            return true;
        }

        if contains_short_name_component(segment) {
            return true;
        }
    }

    if has_windows_device_suffix(&lowered) {
        return true;
    }

    if has_three_dot_component(&lowered) {
        return true;
    }

    false
}

fn strip_surrounding_quotes(path: &str) -> &str {
    if path.len() >= 2 {
        let bytes = path.as_bytes();
        let first = bytes[0];
        let last = bytes[path.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &path[1..path.len() - 1];
        }
    }
    path
}

fn expand_tilde(path: &str) -> String {
    if (path == "~" || path.starts_with("~/") || path.starts_with("~\\"))
        && let Some(home) = home_dir()
    {
        return format!("{home}{}", &path[1..]);
    }
    path.to_owned()
}

fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
}

fn contains_vulnerable_unc_path(path: &str) -> bool {
    path.starts_with(r"\\") || path.starts_with("//")
}

fn contains_shell_expansion(path: &str) -> bool {
    path.contains('$') || path.contains('%') || path.starts_with('=')
}

fn has_alternate_data_stream(path: &str) -> bool {
    let rest = if path.len() >= 2 && path.as_bytes()[1] == b':' {
        &path[2..]
    } else {
        path
    };

    rest.contains(':')
}

fn contains_short_name_component(segment: &str) -> bool {
    let Some(tilde_index) = segment.find('~') else {
        return false;
    };

    let suffix = &segment[tilde_index + 1..];
    let digits = suffix.chars().take_while(|ch| ch.is_ascii_digit()).count();

    digits > 0
}

fn has_windows_device_suffix(path: &str) -> bool {
    const DEVICES: &[&str] = &[
        ".con", ".prn", ".aux", ".nul", ".com1", ".com2", ".com3", ".com4", ".com5", ".com6",
        ".com7", ".com8", ".com9", ".lpt1", ".lpt2", ".lpt3", ".lpt4", ".lpt5", ".lpt6", ".lpt7",
        ".lpt8", ".lpt9",
    ];

    DEVICES.iter().any(|suffix| path.ends_with(suffix))
}

fn has_three_dot_component(path: &str) -> bool {
    path.split('\\')
        .any(|segment| segment.chars().all(|ch| ch == '.') && segment.len() >= 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_paths() {
        assert!(matches!(
            validate_path("src/main.rs"),
            PathValidation::Valid
        ));
        assert!(matches!(
            validate_path("/home/user/file.txt"),
            PathValidation::Valid
        ));
        assert!(matches!(validate_path("a/../b"), PathValidation::Valid));
    }

    #[test]
    fn clean_path_input_strips_quotes() {
        assert_eq!(clean_path_input("\"src/main.rs\""), "src/main.rs");
        assert_eq!(clean_path_input("'src/main.rs'"), "src/main.rs");
    }

    #[test]
    fn null_byte_rejected() {
        assert!(matches!(
            validate_path("file\0.txt"),
            PathValidation::Invalid(_)
        ));
    }

    #[test]
    fn traversal_above_root_is_left_for_permission_checks() {
        assert!(matches!(
            validate_path("../../../etc/passwd"),
            PathValidation::Valid
        ));
    }

    #[test]
    fn overly_long_path_rejected() {
        let long_path = "a".repeat(5000);
        assert!(matches!(
            validate_path(&long_path),
            PathValidation::Invalid(_)
        ));
    }

    #[test]
    fn manual_approval_detects_unc_and_shell_expansion() {
        assert!(path_requires_manual_approval(r"\\server\share\file.txt", false).is_some());
        assert!(path_requires_manual_approval("$HOME/file.txt", false).is_some());
        assert!(path_requires_manual_approval("%TEMP%\\file.txt", false).is_some());
    }

    #[test]
    fn write_globs_require_manual_approval() {
        assert!(path_requires_manual_approval("src/*.rs", true).is_some());
        assert!(path_requires_manual_approval("src/*.rs", false).is_none());
    }

    #[test]
    fn suspicious_windows_paths_require_manual_approval() {
        assert!(has_suspicious_windows_path_pattern(r"\\?\C:\repo\file.txt"));
        assert!(has_suspicious_windows_path_pattern(
            r"C:\repo\file.txt:stream"
        ));
        assert!(has_suspicious_windows_path_pattern(r"C:\repo\GIT~1\config"));
        assert!(has_suspicious_windows_path_pattern(
            r"C:\repo\dir. \file.txt"
        ));
        assert!(has_suspicious_windows_path_pattern(
            r"C:\repo\settings.json.PRN"
        ));
        assert!(has_suspicious_windows_path_pattern(r"C:\repo\...\file.txt"));
    }
}
