use serde::{Deserialize, Serialize};

use super::backgrounding::requested_background;
use super::bash_security::{self, SecurityCheckResult};
use super::git_safety::is_destructive_git_command;
use super::path_validation::command_changes_directory;
use super::readonly::{ShellKind, is_read_only_command};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellCommandSemantic {
    ReadOnly,
    WorkspaceWrite,
    Background,
    Dangerous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellCommandAnalysis {
    pub semantic: ShellCommandSemantic,
    pub read_only: bool,
    pub background: bool,
    pub destructive_git: bool,
    pub dangerous: bool,
    pub changes_directory: bool,
    /// Security flags from the enhanced bash security checks.
    #[serde(default)]
    pub security_flags: Vec<String>,
}

#[must_use]
pub fn analyze_command(
    kind: ShellKind,
    command: &str,
    explicit_background: bool,
) -> ShellCommandAnalysis {
    let normalized = command.trim().to_ascii_lowercase();
    let background = requested_background(explicit_background, kind, command);
    let read_only = is_read_only_command(kind, command);
    let destructive_git = is_destructive_git_command(&normalized);
    let changes_directory = command_changes_directory(kind, command);

    // Run the full bash security check suite matching TS bashSecurity.ts.
    let SecurityCheckResult {
        safe: security_safe,
        reasons: security_flags,
    } = if kind == ShellKind::Bash {
        bash_security::check_bash_security(command)
    } else {
        SecurityCheckResult::safe()
    };

    let dangerous = destructive_git || contains_dangerous_pattern(&normalized) || !security_safe;
    let semantic = if dangerous {
        ShellCommandSemantic::Dangerous
    } else if background {
        ShellCommandSemantic::Background
    } else if read_only {
        ShellCommandSemantic::ReadOnly
    } else {
        ShellCommandSemantic::WorkspaceWrite
    };
    ShellCommandAnalysis {
        semantic,
        read_only,
        background,
        destructive_git,
        dangerous,
        changes_directory,
        security_flags,
    }
}

fn contains_dangerous_pattern(command: &str) -> bool {
    DANGEROUS_PATTERNS
        .iter()
        .any(|pattern| command.contains(pattern))
}

const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "shutdown",
    "reboot",
    "format ",
    "del /s /q",
    "| sh",
    "| bash",
    "| sudo",
    "chmod 777",
    "mkfs.",
    "dd if=",
];

#[cfg(test)]
mod tests {
    use super::{ShellCommandSemantic, analyze_command};
    use crate::shell::readonly::ShellKind;

    #[test]
    fn analysis_detects_background() {
        let analysis = analyze_command(ShellKind::Bash, "npm start &", false);
        assert_eq!(analysis.semantic, ShellCommandSemantic::Background);
    }

    #[test]
    fn analysis_detects_dangerous_git() {
        let analysis = analyze_command(ShellKind::Bash, "git reset --hard HEAD", false);
        assert_eq!(analysis.semantic, ShellCommandSemantic::Dangerous);
        assert!(analysis.destructive_git);
    }

    #[test]
    fn analysis_detects_read_only_commands() {
        let analysis = analyze_command(ShellKind::PowerShell, "Get-ChildItem", false);
        assert_eq!(analysis.semantic, ShellCommandSemantic::ReadOnly);
    }

    #[test]
    fn analysis_detects_command_substitution() {
        let analysis = analyze_command(ShellKind::Bash, "echo $(whoami)", false);
        assert_eq!(analysis.semantic, ShellCommandSemantic::Dangerous);
        assert!(!analysis.security_flags.is_empty());
    }

    #[test]
    fn safe_command_has_no_security_flags() {
        let analysis = analyze_command(ShellKind::Bash, "ls -la /tmp", false);
        assert!(analysis.security_flags.is_empty());
        assert_eq!(analysis.semantic, ShellCommandSemantic::ReadOnly);
    }
}
