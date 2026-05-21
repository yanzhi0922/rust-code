//! PowerShell command semantic analysis for exit-code interpretation.
//!
//! PowerShell-native cmdlets do NOT need exit-code semantics — they signal
//! failure via terminating errors ($?), not exit codes. However, EXTERNAL
//! executables invoked from PowerShell DO set $LASTEXITCODE, and many use
//! non-zero codes to convey information rather than failure.
//!
//! This module provides semantic interpretation for common external commands
//! that may return non-zero exit codes for non-error conditions.

/// Semantic category of a PowerShell command for exit-code interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerShellCommandSemantic {
    /// Default: treat only exit code 0 as success.
    Default,
    /// Grep-like: exit 0 = match found, 1 = no match, 2+ = error.
    GrepLike,
    /// Robocopy: exit 0-7 = success, 8+ = error (bitfield).
    Robocopy,
    /// Package install command (npm, pip, etc.).
    PackageInstall,
    /// File change command (copy, move, etc.).
    FileChange,
    /// Git operation.
    GitOperation,
    /// Network request (curl, wget, etc.).
    NetworkRequest,
    /// Docker operation.
    DockerOperation,
    /// Process management.
    ProcessManagement,
    /// Registry changes.
    RegistryChange,
    /// Service management.
    ServiceManagement,
}

/// Interprets a command result based on semantic rules.
///
/// Returns `(is_error, optional_message)` based on the command's exit code
/// and the detected semantic category.
#[must_use]
pub fn interpret_powershell_result(command: &str, exit_code: i32) -> (bool, Option<String>) {
    let semantic = analyze_powershell_command(command);
    match semantic {
        PowerShellCommandSemantic::GrepLike => {
            if exit_code >= 2 {
                (
                    true,
                    Some(format!("Command failed with exit code {exit_code}")),
                )
            } else if exit_code == 1 {
                (false, Some("No matches found".to_owned()))
            } else {
                (false, None)
            }
        }
        PowerShellCommandSemantic::Robocopy => {
            if exit_code >= 8 {
                (
                    true,
                    Some(format!("Robocopy failed with exit code {exit_code}")),
                )
            } else if exit_code == 0 {
                (false, Some("No files copied (already in sync)".to_owned()))
            } else if (1..8).contains(&exit_code) {
                let msg = if exit_code & 1 != 0 {
                    "Files copied successfully"
                } else {
                    "Robocopy completed (no errors)"
                };
                (false, Some(msg.to_owned()))
            } else {
                (false, None)
            }
        }
        _ => {
            if exit_code != 0 {
                (
                    true,
                    Some(format!("Command failed with exit code {exit_code}")),
                )
            } else {
                (false, None)
            }
        }
    }
}

/// Analyzes a PowerShell command to determine its semantic category.
#[must_use]
pub fn analyze_powershell_command(command: &str) -> PowerShellCommandSemantic {
    let base = extract_base_command(command);

    // Grep-like semantics
    if GREP_LIKE.contains(&base.as_str()) {
        return PowerShellCommandSemantic::GrepLike;
    }

    // Robocopy
    if base == "robocopy" {
        return PowerShellCommandSemantic::Robocopy;
    }

    // Package installs
    if PACKAGE_MANAGERS.contains(&base.as_str())
        && (command.to_ascii_lowercase().contains("install")
            || command.to_ascii_lowercase().contains("add ")
            || command.to_ascii_lowercase().contains("update"))
    {
        return PowerShellCommandSemantic::PackageInstall;
    }

    // Git operations
    if base == "git" {
        return PowerShellCommandSemantic::GitOperation;
    }

    // Network requests
    if NETWORK_COMMANDS.contains(&base.as_str()) {
        return PowerShellCommandSemantic::NetworkRequest;
    }

    // Docker operations
    if DOCKER_COMMANDS.contains(&base.as_str()) {
        return PowerShellCommandSemantic::DockerOperation;
    }

    // Process management
    if PROCESS_COMMANDS.contains(&base.as_str()) {
        return PowerShellCommandSemantic::ProcessManagement;
    }

    // Registry changes
    if command.to_ascii_lowercase().contains("hklm:")
        || command.to_ascii_lowercase().contains("hkcu:")
        || command.to_ascii_lowercase().contains("registry::")
    {
        return PowerShellCommandSemantic::RegistryChange;
    }

    // Service management
    if SERVICE_COMMANDS.contains(&base.as_str()) {
        return PowerShellCommandSemantic::ServiceManagement;
    }

    // File changes
    if FILE_CHANGE_COMMANDS.contains(&base.as_str()) {
        return PowerShellCommandSemantic::FileChange;
    }

    PowerShellCommandSemantic::Default
}

const GREP_LIKE: &[&str] = &["grep", "rg", "findstr", "select-string"];
const PACKAGE_MANAGERS: &[&str] = &[
    "npm", "yarn", "pnpm", "pip", "pip3", "python", "python3", "dotnet", "cargo", "nuget", "choco",
    "scoop", "winget",
];
const NETWORK_COMMANDS: &[&str] = &[
    "curl",
    "wget",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
];
const DOCKER_COMMANDS: &[&str] = &["docker", "docker-compose", "podman"];
const PROCESS_COMMANDS: &[&str] = &[
    "get-process",
    "stop-process",
    "spps",
    "kill",
    "start-process",
    "saps",
    "wait-process",
    "debug-process",
];
const SERVICE_COMMANDS: &[&str] = &[
    "get-service",
    "stop-service",
    "start-service",
    "restart-service",
    "remove-service",
    "set-service",
    "sasv",
    "spsv",
];
const FILE_CHANGE_COMMANDS: &[&str] = &[
    "copy-item",
    "cp",
    "move-item",
    "mv",
    "remove-item",
    "rm",
    "del",
    "ri",
    "set-content",
    "sc",
    "add-content",
    "ac",
    "out-file",
    "new-item",
    "ni",
    "rename-item",
    "rni",
];

/// Extracts the base command name from a PowerShell command string.
/// Strips leading `&` / `.` call operators, quotes, path components, and `.exe` suffix.
fn extract_base_command(command: &str) -> String {
    // Take the last pipeline segment (it determines the exit code)
    let segments: Vec<&str> = command
        .split(['|', ';'])
        .filter(|s| !s.trim().is_empty())
        .collect();
    let last = segments.last().copied().unwrap_or(command);

    // Strip PowerShell call operators: & "cmd", . "cmd"
    let stripped = last
        .trim()
        .trim_start_matches('&')
        .trim_start_matches('.')
        .trim_start();

    // Get first token
    let first_token = stripped.split_whitespace().next().unwrap_or("");

    // Strip surrounding quotes
    let unquoted = first_token.trim_matches(|c| c == '"' || c == '\'');

    // Strip path: C:\bin\grep.exe → grep.exe, .\rg.exe → rg.exe
    let basename = unquoted.rsplit(['\\', '/']).next().unwrap_or(unquoted);

    // Strip .exe suffix (case-insensitive)
    let lower = basename.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        basename[..basename.len() - 4].to_ascii_lowercase()
    } else {
        lower
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PowerShellCommandSemantic, analyze_powershell_command, interpret_powershell_result,
    };

    #[test]
    fn test_grep_semantic() {
        assert_eq!(
            analyze_powershell_command("grep -r 'pattern' ."),
            PowerShellCommandSemantic::GrepLike
        );
        assert_eq!(
            analyze_powershell_command("rg 'pattern'"),
            PowerShellCommandSemantic::GrepLike
        );
        assert_eq!(
            analyze_powershell_command("findstr /s 'pattern' *.txt"),
            PowerShellCommandSemantic::GrepLike
        );
    }

    #[test]
    fn test_robocopy_semantic() {
        assert_eq!(
            analyze_powershell_command("robocopy src dest /mir"),
            PowerShellCommandSemantic::Robocopy
        );
    }

    #[test]
    fn test_git_semantic() {
        assert_eq!(
            analyze_powershell_command("git status"),
            PowerShellCommandSemantic::GitOperation
        );
        assert_eq!(
            analyze_powershell_command("git commit -m 'test'"),
            PowerShellCommandSemantic::GitOperation
        );
    }

    #[test]
    fn test_docker_semantic() {
        assert_eq!(
            analyze_powershell_command("docker build -t myimage ."),
            PowerShellCommandSemantic::DockerOperation
        );
        assert_eq!(
            analyze_powershell_command("docker-compose up"),
            PowerShellCommandSemantic::DockerOperation
        );
    }

    #[test]
    fn test_network_semantic() {
        assert_eq!(
            analyze_powershell_command("curl https://example.com"),
            PowerShellCommandSemantic::NetworkRequest
        );
        assert_eq!(
            analyze_powershell_command("Invoke-WebRequest https://example.com"),
            PowerShellCommandSemantic::NetworkRequest
        );
    }

    #[test]
    fn test_process_management_semantic() {
        assert_eq!(
            analyze_powershell_command("Get-Process"),
            PowerShellCommandSemantic::ProcessManagement
        );
        assert_eq!(
            analyze_powershell_command("Stop-Process -Name 'notepad'"),
            PowerShellCommandSemantic::ProcessManagement
        );
    }

    #[test]
    fn test_registry_semantic() {
        assert_eq!(
            analyze_powershell_command("Get-Item HKLM:\\SOFTWARE\\Microsoft"),
            PowerShellCommandSemantic::RegistryChange
        );
    }

    #[test]
    fn test_service_semantic() {
        assert_eq!(
            analyze_powershell_command("Get-Service -Name 'wuauserv'"),
            PowerShellCommandSemantic::ServiceManagement
        );
    }

    #[test]
    fn test_file_change_semantic() {
        assert_eq!(
            analyze_powershell_command("Copy-Item src.txt dst.txt"),
            PowerShellCommandSemantic::FileChange
        );
        assert_eq!(
            analyze_powershell_command("Remove-Item foo.txt"),
            PowerShellCommandSemantic::FileChange
        );
    }

    #[test]
    fn test_interpret_grep_no_match() {
        let (is_error, msg) = interpret_powershell_result("grep 'pattern' file.txt", 1);
        assert!(!is_error);
        assert_eq!(msg.as_deref(), Some("No matches found"));
    }

    #[test]
    fn test_interpret_grep_error() {
        let (is_error, _msg) = interpret_powershell_result("grep 'pattern' nonexistent", 2);
        assert!(is_error);
    }

    #[test]
    fn test_interpret_robocopy_success() {
        let (is_error, msg) = interpret_powershell_result("robocopy src dest", 1);
        assert!(!is_error);
        assert_eq!(msg.as_deref(), Some("Files copied successfully"));
    }

    #[test]
    fn test_interpret_robocopy_error() {
        let (is_error, _msg) = interpret_powershell_result("robocopy src dest", 8);
        assert!(is_error);
    }

    #[test]
    fn test_extract_base_command_with_path() {
        assert_eq!(
            super::extract_base_command("C:\\bin\\grep.exe 'test'"),
            "grep"
        );
        assert_eq!(super::extract_base_command(".\\rg.exe 'test'"), "rg");
    }

    #[test]
    fn test_extract_base_command_with_call_operator() {
        assert_eq!(super::extract_base_command("& 'grep' 'test'"), "grep");
        assert_eq!(super::extract_base_command(". ./script.ps1"), "script.ps1");
    }

    #[test]
    fn test_default_semantic() {
        assert_eq!(
            analyze_powershell_command("Write-Output 'hello'"),
            PowerShellCommandSemantic::Default
        );
    }
}
