#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    PowerShell,
}

#[must_use]
pub fn is_read_only_command(kind: ShellKind, command: &str) -> bool {
    let normalized = normalize(command);
    let safe_prefixes = match kind {
        ShellKind::Bash => BASH_READ_ONLY_PREFIXES,
        ShellKind::PowerShell => POWERSHELL_READ_ONLY_PREFIXES,
    };
    safe_prefixes
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

fn normalize(command: &str) -> String {
    command
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const BASH_READ_ONLY_PREFIXES: &[&str] = &[
    "ls",
    "pwd",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "rg",
    "grep",
    "find",
    "stat",
    "du",
    "df",
    "ps",
    "env",
    "printenv",
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "git rev-parse",
    "git ls-files",
    "cargo check",
    "cargo test",
    "cargo build",
    "cargo fmt",
    "cargo clippy",
    "rustc --version",
    "python --version",
    "python3 --version",
    "node --version",
    "npm list",
    "date",
    "whoami",
];

const POWERSHELL_READ_ONLY_PREFIXES: &[&str] = &[
    // Filesystem inspection
    "get-childitem",
    "dir",
    "ls",
    "get-item",
    "get-content",
    "type",
    "gc",
    "cat",
    "select-string",
    "test-path",
    "resolve-path",
    "get-location",
    "pwd",
    "get-filehash",
    "get-acl",
    "format-hex",
    // Process & service inspection
    "get-process",
    "gps",
    "get-service",
    "gsv",
    // System information
    "get-date",
    "get-host",
    "get-command",
    "gcm",
    "get-member",
    "gm",
    "get-history",
    "get-variable",
    "gv",
    "get-module",
    "get-formatdata",
    // Object manipulation (read-only when not modifying external state)
    "measure-object",
    "select-object",
    "sort-object",
    "group-object",
    "where-object",
    "?",
    "foreach-object",
    "%",
    // Display-only output
    "write-output",
    "write-host",
    "echo",
    "format-table",
    "format-list",
    "format-wide",
    "format-custom",
    "out-string",
    "out-default",
    // Native Windows diagnostic commands (read-only)
    "systeminfo",
    "hostname",
    "ipconfig",
    "tasklist",
    "netstat",
    "ping",
    "tracert",
    "nslookup",
    "whoami",
    "where",
    "where.exe",
    // .NET inspection
    "dotnet --version",
    "dotnet --info",
    "dotnet --list-runtimes",
    "dotnet --list-sdks",
    // Git read-only commands
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "git branch -a",
    "git branch -l",
    "git rev-parse",
    "git ls-files",
    "git ls-tree",
    "git remote",
    "git remote -v",
    "git tag",
    "git tag -l",
    "git stash list",
    "git describe",
    "git shortlog",
    "git count-objects",
    "git fsck",
    "git version",
    "git whatchanged",
    "git blame",
    // Build tool queries (read-only)
    "cargo check",
    "cargo test",
    "cargo build",
    "cargo fmt",
    "cargo clippy",
    "cargo --version",
    "cargo --list",
    "rustc --version",
    "rustc --print",
    "python --version",
    "py --version",
    "node --version",
    "npm --version",
    "npm list",
    "npm view",
    "npm info",
    "npm ls",
    "yarn --version",
    "yarn list",
    "pnpm --version",
    "pnpm list",
];

#[cfg(test)]
mod tests {
    use super::{ShellKind, is_read_only_command};

    #[test]
    fn bash_read_only_prefixes_match() {
        assert!(is_read_only_command(ShellKind::Bash, "git status"));
        assert!(is_read_only_command(ShellKind::Bash, "cargo test --lib"));
        assert!(!is_read_only_command(
            ShellKind::Bash,
            "git reset --hard HEAD"
        ));
    }

    #[test]
    fn powershell_read_only_prefixes_match() {
        assert!(is_read_only_command(
            ShellKind::PowerShell,
            "Get-ChildItem -Force"
        ));
        assert!(!is_read_only_command(
            ShellKind::PowerShell,
            "Remove-Item foo -Recurse"
        ));
    }
}
