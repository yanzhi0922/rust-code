//! Deep link support for desktop app integration.
//!
//! Corresponds to `.research/cc-haha/src/utils/desktopDeepLink.ts`.
//! Provides URL-based deep linking to resume sessions, open files, etc.

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;

/// Minimum desktop app version required for deep link support.
pub const MIN_DESKTOP_VERSION: &str = "1.0.0";

/// Build a deep link URL for resuming a session.
///
/// # Arguments
/// * `session_id` - The session UUID to resume.
/// * `cwd` - The working directory for the session.
/// * `dev_mode` - If true, use the `remote-code-dev://` scheme.
///
/// # Returns
/// A formatted deep link URL string.
pub fn build_deep_link(session_id: &str, cwd: &str, dev_mode: bool) -> String {
    let scheme = if dev_mode {
        "remote-code-dev"
    } else {
        "remote-code"
    };
    let encoded_cwd = url_encode(cwd);
    format!("{scheme}://resume?session={session_id}&cwd={encoded_cwd}")
}

/// Check if the desktop app is installed on the current platform.
///
/// - **Windows**: Checks registry for `HKEY_CLASSES_ROOT\remote-code` protocol handler.
/// - **macOS**: Checks if `/Applications/Remote Code.app` exists.
/// - **Linux**: Checks xdg-mime for `x-scheme-handler/remote-code`.
#[cfg(target_os = "windows")]
pub fn is_desktop_installed() -> bool {
    // Check if the protocol handler is registered in Windows registry
    std::process::Command::new("reg")
        .args(["query", r"HKCR\remote-code", "/ve"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn is_desktop_installed() -> bool {
    Path::new("/Applications/Remote Code.app").exists()
}

#[cfg(target_os = "linux")]
pub fn is_desktop_installed() -> bool {
    std::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/remote-code"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Fallback for unsupported platforms.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn is_desktop_installed() -> bool {
    false
}

/// Get the installed desktop app version, if available.
///
/// - **Windows**: Parses version from install directory.
/// - **macOS**: Reads `CFBundleShortVersionString` from `Info.plist`.
/// - **Linux**: Returns `None` (not supported).
#[cfg(target_os = "windows")]
pub fn get_desktop_version() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCR\remote-code\DefaultIcon", "/ve"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Try to extract the install path and parse version from it
    for line in stdout.lines() {
        if let Some(idx) = line.find("Remote Code") {
            let path_part = &line[idx..];
            // Look for version-like patterns (e.g., "1.2.3")
            for part in path_part.split(&['\\', '/', '"']) {
                if looks_like_version(part) {
                    return Some(part.to_owned());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub fn get_desktop_version() -> Option<String> {
    let plist_path = "/Applications/Remote Code.app/Contents/Info.plist";
    if !Path::new(plist_path).exists() {
        return None;
    }

    let output = std::process::Command::new("defaults")
        .args(["read", plist_path, "CFBundleShortVersionString"])
        .output()
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !version.is_empty() {
            return Some(version);
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub fn get_desktop_version() -> Option<String> {
    // On Linux, we try to find the .desktop file and parse the version
    let home = std::env::var("HOME").ok()?;
    let desktop_file = format!("{home}/.local/share/applications/remote-code.desktop");
    if !Path::new(&desktop_file).exists() {
        return None;
    }
    // Best-effort: read the desktop file and look for Version key
    let content = std::fs::read_to_string(&desktop_file).ok()?;
    for line in content.lines() {
        if line.starts_with("Version=") {
            let version = line.trim_start_matches("Version=").trim();
            if !version.is_empty() {
                return Some(version.to_owned());
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn get_desktop_version() -> Option<String> {
    None
}

/// Check if a version string meets the minimum version requirement.
///
/// Performs semantic version comparison (major.minor.patch).
pub fn is_version_supported(version: &str) -> bool {
    let min = parse_version(MIN_DESKTOP_VERSION);
    let ver = parse_version(version);
    ver >= min
}

/// Parse a version string into a tuple of (major, minor, patch).
fn parse_version(version: &str) -> (u32, u32, u32) {
    let parts: Vec<&str> = version.trim().split('.').collect();
    let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// Check if a string looks like a semantic version.
#[cfg(any(target_os = "windows", test))]
fn looks_like_version(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u32>().is_ok())
}

/// Simple URL-encoding for deep link parameters.
fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_deep_link_production() {
        let link = build_deep_link("abc-123", "/home/user/project", false);
        assert_eq!(
            link,
            "remote-code://resume?session=abc-123&cwd=%2Fhome%2Fuser%2Fproject"
        );
    }

    #[test]
    fn build_deep_link_dev_mode() {
        let link = build_deep_link("abc-123", "/tmp/test", true);
        assert!(link.starts_with("remote-code-dev://"));
        assert!(link.contains("session=abc-123"));
    }

    #[test]
    fn build_deep_link_encodes_special_chars() {
        let link = build_deep_link("s1", "/path/with spaces/and&special", false);
        assert!(link.contains("%20")); // space encoded
        assert!(link.contains("%26")); // & encoded
    }

    #[test]
    fn is_version_supported_exact_min() {
        assert!(is_version_supported("1.0.0"));
    }

    #[test]
    fn is_version_supported_newer_version() {
        assert!(is_version_supported("1.2.3"));
        assert!(is_version_supported("2.0.0"));
        assert!(is_version_supported("1.0.1"));
    }

    #[test]
    fn is_version_supported_older_version() {
        assert!(!is_version_supported("0.9.9"));
        assert!(!is_version_supported("0.1.0"));
    }

    #[test]
    fn is_version_supported_partial_version() {
        // "1.0" should be treated as "1.0.0"
        assert!(is_version_supported("1.0"));
    }

    #[test]
    fn parse_version_handles_standard() {
        assert_eq!(parse_version("1.2.3"), (1, 2, 3));
    }

    #[test]
    fn parse_version_handles_partial() {
        assert_eq!(parse_version("1.2"), (1, 2, 0));
        assert_eq!(parse_version("1"), (1, 0, 0));
    }

    #[test]
    fn parse_version_handles_empty() {
        assert_eq!(parse_version(""), (0, 0, 0));
    }

    #[test]
    fn looks_like_version_valid() {
        assert!(looks_like_version("1.2.3"));
        assert!(looks_like_version("10.20.30"));
    }

    #[test]
    fn looks_like_version_invalid() {
        assert!(!looks_like_version("abc"));
        assert!(!looks_like_version("1"));
        assert!(!looks_like_version(""));
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("/path/to/file"), "%2Fpath%2Fto%2Ffile");
    }

    #[test]
    fn url_encode_special_chars() {
        let encoded = url_encode("hello world&foo=bar");
        assert_eq!(encoded, "hello%20world%26foo%3Dbar");
    }

    #[test]
    fn min_desktop_version_is_valid() {
        assert!(looks_like_version(MIN_DESKTOP_VERSION));
        assert!(is_version_supported(MIN_DESKTOP_VERSION));
    }
}
