//! Detection of destructive shell commands (bash and PowerShell).
//!
//! Identifies potentially destructive commands and returns a warning string
//! for display in the permission dialog. This is purely informational —
//! it doesn't affect permission logic or auto-approval.

use once_cell::sync::Lazy;
use regex::Regex;

struct DestructivePattern {
    pattern: Regex,
    warning: &'static str,
}

static DESTRUCTIVE_PATTERNS: Lazy<Vec<DestructivePattern>> = Lazy::new(|| {
    let mut patterns = Vec::new();

    // ── Git — data loss / hard to reverse ──────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+reset\s+--hard\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may discard uncommitted changes",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+push\b[^;&|\n]*[ \t](--force|--force-with-lease|-f)\b")
    {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may overwrite remote history",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+clean\b.*-[a-zA-Z]*f") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may permanently delete untracked files",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+checkout\s+(--\s+)?\.[ \t]*($|[;&|\n])") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may discard all working tree changes",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+restore\s+(--\s+)?\.[ \t]*($|[;&|\n])") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may discard all working tree changes",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+stash[ \t]+(drop|clear)\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may permanently remove stashed changes",
        });
    }
    if let Ok(re) =
        Regex::new(r"(?i)\bgit\s+branch\s+(-D[ \t]|--delete\s+--force|--force\s+--delete)\b")
    {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may force-delete a branch",
        });
    }

    // ── Git — safety bypass ────────────────────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+(commit|push|merge)\b[^;&|\n]*--no-verify\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may skip safety hooks",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bgit\s+commit\b[^;&|\n]*--amend\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may rewrite the last commit",
        });
    }

    // ── Unix rm ────────────────────────────────────────────────────────
    if let Ok(re) = Regex::new(
        r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR][a-zA-Z]*f|(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f[a-zA-Z]*[rR]",
    ) {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may recursively force-remove files",
        });
    }
    if let Ok(re) = Regex::new(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR]") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may recursively remove files",
        });
    }
    if let Ok(re) = Regex::new(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may force-remove files",
        });
    }

    // ── PowerShell Remove-Item ─────────────────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\b(Remove-Item|del|rd|rmdir|ri)\b.*-Recurse.*-Force\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may recursively force-remove files",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\b(Remove-Item|del|rd|rmdir|ri)\b.*-Force.*-Recurse\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may recursively force-remove files",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\b(Remove-Item|del|rd|rmdir|ri)\b.*-Recurse\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may recursively remove files",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\b(Remove-Item|del|rd|rmdir|ri)\b.*-Force\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may force-remove files",
        });
    }

    // ── PowerShell system commands ─────────────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\bStop-Process\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may terminate running processes",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bRemove-Service\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may remove a Windows service",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bClear-Content\b.*\*") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may clear content of multiple files",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bFormat-Volume\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may format a disk volume",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bClear-Disk\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may clear a disk",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bStop-Computer\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: will shut down the computer",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bRestart-Computer\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: will restart the computer",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bClear-RecycleBin\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: permanently deletes recycled files",
        });
    }

    // ── Database ───────────────────────────────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\b(DROP|TRUNCATE)\s+(TABLE|DATABASE|SCHEMA)\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may drop or truncate database objects",
        });
    }
    if let Ok(re) = Regex::new(r#"(?i)\bDELETE\s+FROM\s+\w+[ \t]*(;|"|'|\n|$)"#) {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may delete all rows from a database table",
        });
    }

    // ── Infrastructure ─────────────────────────────────────────────────
    if let Ok(re) = Regex::new(r"(?i)\bkubectl\s+delete\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may delete Kubernetes resources",
        });
    }
    if let Ok(re) = Regex::new(r"(?i)\bterraform\s+destroy\b") {
        patterns.push(DestructivePattern {
            pattern: re,
            warning: "Note: may destroy Terraform infrastructure",
        });
    }

    patterns
});

/// Checks if a command matches known destructive patterns.
///
/// Returns a human-readable warning string, or `None` if no destructive
/// pattern is detected.
#[must_use]
pub fn get_destructive_warning(command: &str) -> Option<&'static str> {
    for dp in DESTRUCTIVE_PATTERNS.iter() {
        if dp.pattern.is_match(command) {
            if is_git_clean_dry_run(command) {
                continue;
            }
            return Some(dp.warning);
        }
    }
    None
}

fn is_git_clean_dry_run(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if !lower.contains("git") || !lower.contains("clean") {
        return false;
    }
    lower.contains("-n") || lower.contains("--dry-run")
}

#[cfg(test)]
mod tests {
    use super::get_destructive_warning;

    // ── Git tests ──────────────────────────────────────────────────────

    #[test]
    fn test_git_reset_hard() {
        assert_eq!(
            get_destructive_warning("git reset --hard HEAD"),
            Some("Note: may discard uncommitted changes")
        );
    }

    #[test]
    fn test_git_push_force() {
        assert_eq!(
            get_destructive_warning("git push --force origin main"),
            Some("Note: may overwrite remote history")
        );
    }

    #[test]
    fn test_git_push_force_with_lease() {
        assert_eq!(
            get_destructive_warning("git push --force-with-lease"),
            Some("Note: may overwrite remote history")
        );
    }

    #[test]
    fn test_git_push_short_f() {
        assert_eq!(
            get_destructive_warning("git push -f origin main"),
            Some("Note: may overwrite remote history")
        );
    }

    #[test]
    fn test_git_clean_force() {
        assert_eq!(
            get_destructive_warning("git clean -fdx"),
            Some("Note: may permanently delete untracked files")
        );
    }

    #[test]
    fn test_git_clean_dry_run_no_warning() {
        assert!(get_destructive_warning("git clean -nfdx").is_none());
    }

    #[test]
    fn test_git_checkout_dot() {
        assert_eq!(
            get_destructive_warning("git checkout ."),
            Some("Note: may discard all working tree changes")
        );
    }

    #[test]
    fn test_git_restore_dot() {
        assert_eq!(
            get_destructive_warning("git restore ."),
            Some("Note: may discard all working tree changes")
        );
    }

    #[test]
    fn test_git_stash_drop() {
        assert_eq!(
            get_destructive_warning("git stash drop stash@{0}"),
            Some("Note: may permanently remove stashed changes")
        );
    }

    #[test]
    fn test_git_stash_clear() {
        assert_eq!(
            get_destructive_warning("git stash clear"),
            Some("Note: may permanently remove stashed changes")
        );
    }

    #[test]
    fn test_git_branch_force_delete() {
        assert_eq!(
            get_destructive_warning("git branch -D feature-branch"),
            Some("Note: may force-delete a branch")
        );
    }

    #[test]
    fn test_git_commit_no_verify() {
        assert_eq!(
            get_destructive_warning("git commit --no-verify -m \"wip\""),
            Some("Note: may skip safety hooks")
        );
    }

    #[test]
    fn test_git_push_no_verify() {
        assert_eq!(
            get_destructive_warning("git push --no-verify"),
            Some("Note: may skip safety hooks")
        );
    }

    #[test]
    fn test_git_merge_no_verify() {
        assert_eq!(
            get_destructive_warning("git merge --no-verify feature"),
            Some("Note: may skip safety hooks")
        );
    }

    #[test]
    fn test_git_commit_amend() {
        assert_eq!(
            get_destructive_warning("git commit --amend -m \"fix\""),
            Some("Note: may rewrite the last commit")
        );
    }

    // ── Unix rm tests ──────────────────────────────────────────────────

    #[test]
    fn test_unix_rm_rf() {
        assert_eq!(
            get_destructive_warning("rm -rf /tmp/build"),
            Some("Note: may recursively force-remove files")
        );
    }

    #[test]
    fn test_unix_rm_fr() {
        assert_eq!(
            get_destructive_warning("rm -fr /tmp/build"),
            Some("Note: may recursively force-remove files")
        );
    }

    #[test]
    fn test_unix_rm_r() {
        assert_eq!(
            get_destructive_warning("rm -r /tmp/build"),
            Some("Note: may recursively remove files")
        );
    }

    #[test]
    fn test_unix_rm_f() {
        assert_eq!(
            get_destructive_warning("rm -f /tmp/file.txt"),
            Some("Note: may force-remove files")
        );
    }

    #[test]
    fn test_unix_rm_chained() {
        assert_eq!(
            get_destructive_warning("echo done; rm -rf ./dist"),
            Some("Note: may recursively force-remove files")
        );
    }

    // ── PowerShell tests ───────────────────────────────────────────────

    #[test]
    fn test_remove_item_recurse_force() {
        assert_eq!(
            get_destructive_warning("Remove-Item ./node_modules -Recurse -Force"),
            Some("Note: may recursively force-remove files")
        );
    }

    #[test]
    fn test_remove_item_recurse_only() {
        assert_eq!(
            get_destructive_warning("Remove-Item ./dist -Recurse"),
            Some("Note: may recursively remove files")
        );
    }

    #[test]
    fn test_remove_item_force_only() {
        assert_eq!(
            get_destructive_warning("Remove-Item ./temp -Force"),
            Some("Note: may force-remove files")
        );
    }

    #[test]
    fn test_stop_process() {
        assert_eq!(
            get_destructive_warning("Stop-Process -Name 'notepad'"),
            Some("Note: may terminate running processes")
        );
    }

    #[test]
    fn test_format_volume() {
        assert_eq!(
            get_destructive_warning("Format-Volume -DriveLetter D -FileSystem NTFS"),
            Some("Note: may format a disk volume")
        );
    }

    #[test]
    fn test_stop_computer() {
        assert_eq!(
            get_destructive_warning("Stop-Computer"),
            Some("Note: will shut down the computer")
        );
    }

    #[test]
    fn test_restart_computer() {
        assert_eq!(
            get_destructive_warning("Restart-Computer"),
            Some("Note: will restart the computer")
        );
    }

    #[test]
    fn test_clear_recycle_bin() {
        assert_eq!(
            get_destructive_warning("Clear-RecycleBin"),
            Some("Note: permanently deletes recycled files")
        );
    }

    // ── Database tests ─────────────────────────────────────────────────

    #[test]
    fn test_database_drop() {
        assert_eq!(
            get_destructive_warning("DROP TABLE users"),
            Some("Note: may drop or truncate database objects")
        );
    }

    #[test]
    fn test_database_delete_from() {
        assert_eq!(
            get_destructive_warning("DELETE FROM users;"),
            Some("Note: may delete all rows from a database table")
        );
    }

    // ── Infrastructure tests ───────────────────────────────────────────

    #[test]
    fn test_kubectl_delete() {
        assert_eq!(
            get_destructive_warning("kubectl delete pod my-pod"),
            Some("Note: may delete Kubernetes resources")
        );
    }

    #[test]
    fn test_terraform_destroy() {
        assert_eq!(
            get_destructive_warning("terraform destroy"),
            Some("Note: may destroy Terraform infrastructure")
        );
    }

    // ── Safe commands ──────────────────────────────────────────────────

    #[test]
    fn test_safe_commands_no_warning() {
        assert!(get_destructive_warning("Get-Process").is_none());
        assert!(get_destructive_warning("Get-ChildItem -Force").is_none());
        assert!(get_destructive_warning("git status").is_none());
        assert!(get_destructive_warning("Write-Output 'hello'").is_none());
        assert!(get_destructive_warning("ls -la").is_none());
        assert!(get_destructive_warning("echo hello").is_none());
        assert!(get_destructive_warning("cargo build").is_none());
    }
}
