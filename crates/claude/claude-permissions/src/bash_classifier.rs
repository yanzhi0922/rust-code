//! Bash command classifier for permission decisions.
//!
//! Corresponds to `.research/cc-haha/src/utils/permissions/bashClassifier.ts`.
//! Provides semantic classification of bash commands into safety categories,
//! supporting package installs, file operations, network operations, and git operations.

/// Result of classifying a bash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BashClassificationResult {
    /// The command is safe to execute.
    Allow,
    /// The command is denied with a reason.
    Deny(String),
}

/// Category of a bash command for classification purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    /// Package installation commands (npm, cargo, pip, etc.)
    PackageInstall,
    /// File read operations (cat, head, tail, ls, etc.)
    FileRead,
    /// File write operations (cp, mv, mkdir, touch, etc.)
    FileWrite,
    /// File delete operations (rm, rmdir, etc.)
    FileDelete,
    /// Network operations (curl, wget, ssh, scp, etc.)
    Network,
    /// Git operations (git status, git log, etc.)
    Git,
    /// System information (ps, df, uname, etc.)
    SystemInfo,
    /// Process management (kill, pkill, etc.)
    ProcessManagement,
    /// Shell builtins (echo, export, alias, etc.)
    ShellBuiltin,
    /// Container operations (docker, podman, etc.)
    Container,
    /// Build/test commands (make, cmake, cargo test, etc.)
    Build,
    /// Unknown or unclassified command
    Unknown,
}

/// Rule for classifying bash commands.
#[derive(Debug, Clone)]
pub struct BashRule {
    /// The pattern to match against commands.
    pub pattern: String,
    /// Whether this is an allow or deny rule.
    pub is_allow: bool,
    /// Optional human-readable reason.
    pub reason: Option<String>,
}

/// Classify a bash command based on allow and deny rules.
///
/// Performs semantic matching: first checks deny rules, then allow rules.
/// If no rule matches, defaults to deny.
pub fn classify_bash_command(
    command: &str,
    allow_rules: &[BashRule],
    deny_rules: &[BashRule],
) -> BashClassificationResult {
    let trimmed = command.trim();
    let command_lower = trimmed.to_lowercase();

    // Check deny rules first
    for rule in deny_rules {
        if matches_rule(&command_lower, &rule.pattern.to_lowercase()) {
            return BashClassificationResult::Deny(
                rule.reason
                    .clone()
                    .unwrap_or_else(|| format!("Command matches deny rule: '{}'", rule.pattern)),
            );
        }
    }

    // Check allow rules
    for rule in allow_rules {
        if matches_rule(&command_lower, &rule.pattern.to_lowercase()) {
            return BashClassificationResult::Allow;
        }
    }

    // Default: deny unknown commands
    BashClassificationResult::Deny(format!("Command not in allow list: '{trimmed}'"))
}

/// Check if a command matches a rule pattern.
///
/// Supports:
/// - Exact match: `git status`
/// - Prefix match: `git ` (matches any git subcommand)
/// - Glob match: `cargo *` (matches cargo with any arguments)
fn matches_rule(command: &str, pattern: &str) -> bool {
    // Exact match
    if command == pattern {
        return true;
    }

    // Prefix match (pattern ends with space)
    if pattern.ends_with(' ') && command.starts_with(pattern) {
        return true;
    }

    // Glob match (pattern contains *)
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            let prefix = parts[0];
            let suffix = parts[1];
            let starts = prefix.is_empty() || command.starts_with(prefix);
            let ends = suffix.is_empty() || command.ends_with(suffix);
            return starts && ends;
        }
    }

    // Substring match for deny patterns (catches dangerous patterns in pipelines)
    if pattern.len() > 3 && command.contains(pattern) {
        return true;
    }

    false
}

/// Categorize a bash command into a semantic category.
pub fn categorize_command(command: &str) -> CommandCategory {
    let trimmed = command.trim().to_lowercase();
    let first_word = trimmed.split_whitespace().next().unwrap_or("");

    match first_word {
        "npm" | "yarn" | "pnpm" | "bun" | "pip" | "pip3" | "gem" | "cargo" | "apt" | "apt-get"
        | "yum" | "dnf" | "brew" | "pacman" => CommandCategory::PackageInstall,
        "cat" | "head" | "tail" | "less" | "more" | "file" | "stat" | "wc" | "md5sum"
        | "sha256sum" | "xxd" => CommandCategory::FileRead,
        "ls" | "dir" | "find" | "tree" | "locate" | "which" | "whereis" => {
            CommandCategory::FileRead
        }
        "cp" | "mv" | "mkdir" | "touch" | "chmod" | "chown" | "ln" | "tar" | "zip" | "unzip"
        | "gzip" | "gunzip" => CommandCategory::FileWrite,
        "rm" | "rmdir" | "shred" | "truncate" => CommandCategory::FileDelete,
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "ftp" | "nc" | "ncat" | "telnet" | "dig"
        | "nslookup" | "ping" | "traceroute" => CommandCategory::Network,
        "git" => CommandCategory::Git,
        "ps" | "top" | "htop" | "df" | "du" | "free" | "uname" | "uptime" | "hostname"
        | "whoami" | "id" | "date" | "env" | "printenv" => CommandCategory::SystemInfo,
        "kill" | "pkill" | "killall" | "nice" | "renice" | "nohup" | "bg" | "fg" | "jobs"
        | "disown" => CommandCategory::ProcessManagement,
        "echo" | "export" | "alias" | "unalias" | "set" | "unset" | "source" | "type"
        | "command" | "hash" | "readonly" | "shift" | "eval" => CommandCategory::ShellBuiltin,
        "docker" | "podman" | "kubectl" | "docker-compose" => CommandCategory::Container,
        "make" | "cmake" | "ninja" | "bazel" | "gradle" | "mvn" | "ant" | "xcodebuild"
        | "msbuild" | "dotnet" => CommandCategory::Build,
        _ => CommandCategory::Unknown,
    }
}

/// Get default allow rules for bash commands.
pub fn default_allow_rules() -> Vec<BashRule> {
    vec![
        BashRule {
            pattern: "git status".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git log ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git diff".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git branch".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git show".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git remote".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git stash list".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "git tag".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "cargo ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "npm ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "node ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "npx ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "python ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "python3 ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "pip install ".into(),
            is_allow: true,
            reason: Some("Package install to virtualenv".into()),
        },
        BashRule {
            pattern: "ls".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "cat ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "head ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "tail ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "find ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "tree".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "echo ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "pwd".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "which ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "whoami".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "env".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "make ".into(),
            is_allow: true,
            reason: None,
        },
        BashRule {
            pattern: "cmake ".into(),
            is_allow: true,
            reason: None,
        },
    ]
}

/// Get default deny rules for bash commands.
pub fn default_deny_rules() -> Vec<BashRule> {
    vec![
        BashRule {
            pattern: "rm -rf /".into(),
            is_allow: false,
            reason: Some("Refusing to recursively delete root filesystem".into()),
        },
        BashRule {
            pattern: "rm -rf /*".into(),
            is_allow: false,
            reason: Some("Refusing to recursively delete root filesystem".into()),
        },
        BashRule {
            pattern: "mkfs".into(),
            is_allow: false,
            reason: Some("Refusing to format filesystem".into()),
        },
        BashRule {
            pattern: "dd if=".into(),
            is_allow: false,
            reason: Some("Refusing raw disk write".into()),
        },
        BashRule {
            pattern: "shutdown".into(),
            is_allow: false,
            reason: Some("Refusing system shutdown".into()),
        },
        BashRule {
            pattern: "reboot".into(),
            is_allow: false,
            reason: Some("Refusing system reboot".into()),
        },
        BashRule {
            pattern: "format disk".into(),
            is_allow: false,
            reason: Some("Refusing disk format".into()),
        },
        BashRule {
            pattern: "drop database".into(),
            is_allow: false,
            reason: Some("Refusing database drop".into()),
        },
        BashRule {
            pattern: "git push --force".into(),
            is_allow: false,
            reason: Some("Refusing force push".into()),
        },
        BashRule {
            pattern: "git push -f".into(),
            is_allow: false,
            reason: Some("Refusing force push".into()),
        },
        BashRule {
            pattern: "sudo ".into(),
            is_allow: false,
            reason: Some("Refusing sudo".into()),
        },
        BashRule {
            pattern: "su ".into(),
            is_allow: false,
            reason: Some("Refusing user switch".into()),
        },
        BashRule {
            pattern: "chmod 777".into(),
            is_allow: false,
            reason: Some("Refusing overly permissive chmod".into()),
        },
        BashRule {
            pattern: "> /dev/sd".into(),
            is_allow: false,
            reason: Some("Refusing direct device write".into()),
        },
        BashRule {
            pattern: "curl ".into(),
            is_allow: false,
            reason: Some("Network operations require review".into()),
        },
        BashRule {
            pattern: "wget ".into(),
            is_allow: false,
            reason: Some("Network operations require review".into()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_exact_match() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("git status", &allow, &deny);
        assert_eq!(result, BashClassificationResult::Allow);
    }

    #[test]
    fn allow_prefix_match() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("git log --oneline -10", &allow, &deny);
        assert_eq!(result, BashClassificationResult::Allow);
    }

    #[test]
    fn deny_dangerous_command() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("rm -rf /", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn deny_sudo() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("sudo apt install something", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn deny_force_push() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("git push --force origin main", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn deny_unknown_command() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("some-random-command arg1", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn deny_takes_precedence_over_allow() {
        // curl is in deny list, even if we add it to allow
        let allow = vec![BashRule {
            pattern: "curl ".into(),
            is_allow: true,
            reason: None,
        }];
        let deny = default_deny_rules();
        let result = classify_bash_command("curl https://example.com", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn categorize_git() {
        assert_eq!(categorize_command("git status"), CommandCategory::Git);
        assert_eq!(categorize_command("git log"), CommandCategory::Git);
    }

    #[test]
    fn categorize_package_install() {
        assert_eq!(
            categorize_command("npm install express"),
            CommandCategory::PackageInstall
        );
        assert_eq!(
            categorize_command("cargo build"),
            CommandCategory::PackageInstall
        );
    }

    #[test]
    fn categorize_file_read() {
        assert_eq!(
            categorize_command("cat file.txt"),
            CommandCategory::FileRead
        );
        assert_eq!(categorize_command("ls -la"), CommandCategory::FileRead);
    }

    #[test]
    fn categorize_network() {
        assert_eq!(
            categorize_command("ssh user@host"),
            CommandCategory::Network
        );
        assert_eq!(
            categorize_command("ping google.com"),
            CommandCategory::Network
        );
    }

    #[test]
    fn categorize_unknown() {
        assert_eq!(
            categorize_command("my-custom-tool arg1"),
            CommandCategory::Unknown
        );
    }

    #[test]
    fn matches_rule_exact() {
        assert!(matches_rule("git status", "git status"));
        assert!(!matches_rule("git status", "git log"));
    }

    #[test]
    fn matches_rule_prefix() {
        assert!(matches_rule("git log --oneline", "git log "));
        assert!(!matches_rule("git status", "git log "));
    }

    #[test]
    fn matches_rule_glob() {
        assert!(matches_rule("cargo build --release", "cargo *"));
        assert!(matches_rule("npm test", "npm *"));
    }

    #[test]
    fn matches_rule_substring() {
        assert!(matches_rule("echo hello && rm -rf /", "rm -rf"));
    }

    #[test]
    fn empty_command_denied() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("", &allow, &deny);
        assert!(matches!(result, BashClassificationResult::Deny(_)));
    }

    #[test]
    fn whitespace_trimmed() {
        let allow = default_allow_rules();
        let deny = default_deny_rules();
        let result = classify_bash_command("  git status  ", &allow, &deny);
        assert_eq!(result, BashClassificationResult::Allow);
    }
}
