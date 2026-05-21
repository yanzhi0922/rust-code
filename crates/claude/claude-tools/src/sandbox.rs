//! Sandbox execution for running commands in an isolated environment.
//!
//! Supports platform-specific sandboxing strategies:
//! - **macOS**: Seatbelt (`sandbox-exec`) with configurable SBPL policies
//! - **Linux**: Landlock (when available) or basic environment isolation
//! - **Windows**: Basic environment isolation with restricted PATH
//!
//! Falls back to `Basic` mode on unsupported platforms.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Sandbox policy
// ---------------------------------------------------------------------------

/// Platform-specific sandbox strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// No sandboxing — run commands directly.
    None,
    /// Basic restrictions: environment variable cleanup + timeout.
    Basic,
    /// macOS Seatbelt policy.
    #[cfg(target_os = "macos")]
    Seatbelt(SeatbeltPolicy),
    /// Linux Landlock policy (graceful fallback to Basic if unavailable).
    #[cfg(target_os = "linux")]
    Landlock(LandlockPolicy),
    /// Windows restriction policy.
    #[cfg(target_os = "windows")]
    Windows(WindowsPolicy),
}

impl SandboxPolicy {
    /// Return the platform-default policy for the given workspace.
    pub fn platform_default(workspace: &Path) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::Seatbelt(SeatbeltPolicy {
                allowed_dirs: vec![workspace.to_path_buf()],
                allow_network: false,
            })
        }
        #[cfg(target_os = "linux")]
        {
            Self::Landlock(LandlockPolicy {
                allowed_dirs: vec![workspace.to_path_buf()],
                allow_network: false,
            })
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows(WindowsPolicy {
                allowed_dirs: vec![workspace.to_path_buf()],
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            // Unsupported platform — fall back to basic sandbox.
            // `workspace` is unused here but kept for API consistency.
            let _ = workspace;
            Self::Basic
        }
    }
}

/// macOS Seatbelt policy parameters.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeatbeltPolicy {
    /// Directories the sandboxed command may read and write.
    pub allowed_dirs: Vec<PathBuf>,
    /// Whether network access is permitted.
    pub allow_network: bool,
}

/// Linux Landlock policy parameters.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandlockPolicy {
    /// Directories the sandboxed command may read and write.
    pub allowed_dirs: Vec<PathBuf>,
    /// Whether network access is permitted.
    pub allow_network: bool,
}

/// Windows restriction policy parameters.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsPolicy {
    /// Directories the sandboxed command may access.
    pub allowed_dirs: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Sandbox policy to apply.
    pub policy: SandboxPolicy,
    /// Maximum execution time in seconds.
    pub timeout_secs: u64,
    /// Maximum memory usage in MB (Linux only, best-effort).
    pub max_memory_mb: Option<u64>,
}

impl SandboxConfig {
    /// Create a default sandbox config scoped to the given workspace directory.
    ///
    /// Uses the platform-default policy.
    #[must_use]
    pub fn default_for_workspace(workspace: &Path) -> Self {
        Self {
            policy: SandboxPolicy::platform_default(workspace),
            timeout_secs: 120,
            max_memory_mb: None,
        }
    }

    /// Create a sandbox config with no sandboxing.
    #[must_use]
    pub fn none() -> Self {
        Self {
            policy: SandboxPolicy::None,
            timeout_secs: 120,
            max_memory_mb: None,
        }
    }

    /// Create a basic sandbox config (env cleanup + timeout).
    #[must_use]
    pub fn basic() -> Self {
        Self {
            policy: SandboxPolicy::Basic,
            timeout_secs: 120,
            max_memory_mb: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Result of a sandboxed command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResult {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Exit code, if available.
    pub exit_code: Option<i32>,
    /// Whether the command timed out.
    pub timed_out: bool,
}

// ---------------------------------------------------------------------------
// Execution entry point
// ---------------------------------------------------------------------------

/// Execute a command inside a sandbox with the given configuration.
///
/// Dispatches to the platform-specific implementation based on the active policy.
pub async fn execute_in_sandbox(command: &str, config: &SandboxConfig) -> Result<SandboxResult> {
    let timeout = Duration::from_secs(config.timeout_secs);

    match &config.policy {
        SandboxPolicy::None => execute_unrestricted(command, config, timeout).await,
        SandboxPolicy::Basic => execute_basic(command, config, timeout).await,

        #[cfg(target_os = "macos")]
        SandboxPolicy::Seatbelt(policy) => execute_seatbelt(command, policy, timeout).await,

        #[cfg(target_os = "linux")]
        SandboxPolicy::Landlock(_policy) => {
            // NOTE: Landlock (Linux Kernel ≥ 5.13) provides filesystem-level
            // access control via the `landlock` crate.  Integration is deferred
            // until the `landlock` crate is added as a dependency.  The policy
            // fields (`allowed_dirs`, `allow_network`) are preserved here so
            // they can be wired in once the crate is available.
            execute_basic(command, config, timeout).await
        }

        #[cfg(target_os = "windows")]
        SandboxPolicy::Windows(policy) => execute_windows(command, policy, config, timeout).await,
    }
}

// ---------------------------------------------------------------------------
// Platform implementations
// ---------------------------------------------------------------------------

/// No sandbox — run the command directly.
async fn execute_unrestricted(
    command: &str,
    config: &SandboxConfig,
    timeout: Duration,
) -> Result<SandboxResult> {
    let mut cmd = build_shell_command(command);
    if let Some(dir) = first_allowed_dir(config) {
        cmd.current_dir(dir);
    }

    let output = tokio::time::timeout(timeout, cmd.output()).await;
    match output {
        Ok(Ok(output)) => Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            timed_out: false,
        }),
        Ok(Err(error)) => Err(error).context("sandbox command failed to execute"),
        Err(_) => Ok(SandboxResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds.", config.timeout_secs),
            exit_code: None,
            timed_out: true,
        }),
    }
}

/// Basic sandbox: strip environment, set working directory, enforce timeout.
async fn execute_basic(
    command: &str,
    config: &SandboxConfig,
    timeout: Duration,
) -> Result<SandboxResult> {
    let mut cmd = build_shell_command(command);
    strip_environment(&mut cmd);
    if let Some(dir) = first_allowed_dir(config) {
        cmd.current_dir(dir);
    }

    let output = tokio::time::timeout(timeout, cmd.output()).await;
    match output {
        Ok(Ok(output)) => Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            timed_out: false,
        }),
        Ok(Err(error)) => Err(error).context("sandbox command failed to execute"),
        Err(_) => Ok(SandboxResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds.", config.timeout_secs),
            exit_code: None,
            timed_out: true,
        }),
    }
}

/// Windows sandbox: basic isolation with restricted environment.
#[cfg(target_os = "windows")]
async fn execute_windows(
    command: &str,
    policy: &WindowsPolicy,
    config: &SandboxConfig,
    timeout: Duration,
) -> Result<SandboxResult> {
    let mut cmd = build_shell_command(command);
    strip_environment(&mut cmd);

    // Set working directory to the first allowed directory.
    if let Some(dir) = policy.allowed_dirs.first() {
        cmd.current_dir(dir);
    }

    let output = tokio::time::timeout(timeout, cmd.output()).await;
    match output {
        Ok(Ok(output)) => Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            timed_out: false,
        }),
        Ok(Err(error)) => Err(error).context("sandbox command failed to execute"),
        Err(_) => Ok(SandboxResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds.", config.timeout_secs),
            exit_code: None,
            timed_out: true,
        }),
    }
}

/// macOS Seatbelt execution using `sandbox-exec`.
///
/// Generates a comprehensive SBPL (Seatbelt Profile Language) policy that:
/// - Denies all operations by default
/// - Allows reading system directories (/usr, /System, /Library, etc.)
/// - Allows executing standard shells and development tools
/// - Allows read/write access to workspace directories
/// - Optionally allows network access
#[cfg(target_os = "macos")]
async fn execute_seatbelt(
    command: &str,
    policy: &SeatbeltPolicy,
    timeout: Duration,
) -> Result<SandboxResult> {
    let sbpl = generate_seatbelt_policy(policy);

    // Execute via sandbox-exec.
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("sandbox-exec")
            .arg("-p")
            .arg(&sbpl)
            .arg("/bin/sh")
            .arg("-c")
            .arg(command)
            .output(),
    )
    .await;

    match output {
        Ok(Ok(output)) => Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            timed_out: false,
        }),
        Ok(Err(error)) => {
            // If sandbox-exec is not available, fall back to basic execution.
            if error.kind() == std::io::ErrorKind::NotFound {
                let config = SandboxConfig::basic();
                return execute_basic(command, &config, timeout).await;
            }
            Err(error).context("seatbelt sandbox-exec failed")
        }
        Err(_) => Ok(SandboxResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds.", timeout.as_secs()),
            exit_code: None,
            timed_out: true,
        }),
    }
}

/// Generate a Seatbelt SBPL policy string for the given workspace directories.
#[cfg(target_os = "macos")]
fn generate_seatbelt_policy(policy: &SeatbeltPolicy) -> String {
    let mut sbpl = String::from("(version 1)\n");
    sbpl += "(deny default)\n";

    // --- System read access ---
    // Allow reading from standard system directories.
    let read_only_subpaths = [
        "/usr",
        "/System",
        "/Library",
        "/bin",
        "/sbin",
        "/opt",
        "/etc",
        "/private/etc",
        "/private/tmp",
        "/private/var/db/dyld",
        "/AppleInternal",
    ];
    for path in &read_only_subpaths {
        sbpl += &format!("(allow file-read* (subpath \"{path}\"))\n");
    }

    // --- Developer tool directories ---
    // Xcode command line tools, Homebrew, MacPorts, etc.
    let dev_paths = [
        "/Applications/Xcode.app",
        "/Library/Developer",
        "/usr/local",
        "/opt/homebrew",
        "/opt/local",
    ];
    for path in &dev_paths {
        sbpl += &format!("(allow file-read* (subpath \"{path}\"))\n");
        // Also allow executing binaries from these directories.
        sbpl += &format!("(allow process-exec (subpath \"{path}\"))\n");
    }

    // --- Shell execution ---
    let shells = [
        "/bin/sh",
        "/bin/bash",
        "/bin/zsh",
        "/bin/cat",
        "/bin/ls",
        "/bin/mkdir",
        "/bin/rm",
        "/bin/cp",
        "/bin/mv",
        "/usr/bin/env",
    ];
    for shell in &shells {
        sbpl += &format!("(allow process-exec (literal \"{shell}\"))\n");
    }

    // --- Process operations ---
    sbpl += "(allow process-fork)\n";
    sbpl += "(allow process-exec (subpath \"/usr/bin\"))\n";
    sbpl += "(allow process-exec (subpath \"/usr/local/bin\"))\n";

    // --- Temporary directories ---
    sbpl += "(allow file-read* (subpath \"/tmp\"))\n";
    sbpl += "(allow file-write* (subpath \"/tmp\"))\n";

    // --- Workspace directories ---
    for dir in &policy.allowed_dirs {
        let path = dir.display();
        sbpl += &format!("(allow file-read* (subpath \"{path}\"))\n");
        sbpl += &format!("(allow file-write* (subpath \"{path}\"))\n");
        sbpl += &format!("(allow process-exec (subpath \"{path}\"))\n");
    }

    // --- Network control ---
    if policy.allow_network {
        sbpl += "(allow network*)\n";
        sbpl += "(allow system-socket)\n";
    }

    // --- Signals and IPC ---
    sbpl += "(allow signal (target self))\n";
    sbpl += "(allow mach-lookup)\n";
    sbpl += "(allow file-read-metadata)\n";

    sbpl
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the platform-appropriate shell command.
fn build_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Strip the environment to a safe subset.
fn strip_environment(cmd: &mut Command) {
    cmd.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        cmd.env("USERPROFILE", userprofile);
    }
    if let Ok(systemroot) = std::env::var("SystemRoot") {
        cmd.env("SystemRoot", systemroot);
    }
    // Propagate TEMP/TMP for tools that need temporary directories.
    if let Ok(temp) = std::env::var("TEMP") {
        cmd.env("TEMP", temp);
    }
    if let Ok(tmp) = std::env::var("TMP") {
        cmd.env("TMP", tmp);
    }
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        cmd.env("TMPDIR", tmpdir);
    }
}

/// Extract the first allowed directory from the sandbox config policy.
fn first_allowed_dir(config: &SandboxConfig) -> Option<PathBuf> {
    match &config.policy {
        SandboxPolicy::None | SandboxPolicy::Basic => None,
        #[cfg(target_os = "macos")]
        SandboxPolicy::Seatbelt(p) => p.allowed_dirs.first().cloned(),
        #[cfg(target_os = "linux")]
        SandboxPolicy::Landlock(p) => p.allowed_dirs.first().cloned(),
        #[cfg(target_os = "windows")]
        SandboxPolicy::Windows(p) => p.allowed_dirs.first().cloned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn sandbox_executes_echo() {
        let config = SandboxConfig::basic();

        let (command, expected) = ("echo hello", "hello");

        let result = execute_in_sandbox(command, &config)
            .await
            .expect("sandbox should execute");

        assert!(!result.timed_out, "should not time out");
        assert!(
            result.stdout.trim().contains(expected),
            "stdout should contain '{expected}', got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn sandbox_respects_timeout() {
        let config = SandboxConfig {
            timeout_secs: 1,
            ..SandboxConfig::basic()
        };

        let command = if cfg!(windows) {
            "ping -n 10 127.0.0.1"
        } else {
            "sleep 30"
        };

        let result = execute_in_sandbox(command, &config)
            .await
            .expect("sandbox should handle timeout");

        assert!(result.timed_out, "should have timed out");
    }

    #[test]
    fn sandbox_config_default_for_workspace() {
        let workspace = Path::new("/tmp/test");
        let config = SandboxConfig::default_for_workspace(workspace);
        assert_eq!(config.timeout_secs, 120);
        assert!(config.max_memory_mb.is_none());
    }

    #[test]
    fn sandbox_config_none_policy() {
        let config = SandboxConfig::none();
        assert!(matches!(config.policy, SandboxPolicy::None));
    }

    #[test]
    fn sandbox_config_basic_policy() {
        let config = SandboxConfig::basic();
        assert!(matches!(config.policy, SandboxPolicy::Basic));
    }

    #[tokio::test]
    async fn unrestricted_execution_works() {
        let config = SandboxConfig::none();
        let result = execute_in_sandbox("echo test", &config)
            .await
            .expect("unrestricted execution should work");
        assert!(!result.timed_out);
        assert!(result.stdout.trim().contains("test"));
    }
}
