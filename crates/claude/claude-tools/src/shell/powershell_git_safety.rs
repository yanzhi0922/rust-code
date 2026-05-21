//! PowerShell-specific git safety checks.
//!
//! Detects destructive git operations and returns warning messages.
//! This is a PowerShell-specific complement to the generic `git_safety` module,
//! providing richer warnings for the PowerShell tool prompt.

use once_cell::sync::Lazy;
use regex::Regex;

/// Checks if a PowerShell command contains destructive git operations.
///
/// Returns a warning message if a destructive git operation is detected,
/// or `None` if the command appears safe.
#[must_use]
pub fn check_powershell_git_safety(command: &str) -> Option<String> {
    let checks: &[fn(&str) -> Option<String>] = &[
        check_git_reset_hard,
        check_git_push_force,
        check_git_clean,
        check_git_checkout_discard,
        check_git_rebase,
        check_git_amend,
        check_git_stash_drop,
        check_git_no_verify,
        check_git_bare_repo_attack,
    ];

    for check in checks {
        if let Some(warning) = check(command) {
            return Some(warning);
        }
    }
    None
}

/// Detects `git reset --hard` which discards all uncommitted changes.
fn check_git_reset_hard(command: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+reset\s+--hard\b").expect("valid regex"));
    if RE.is_match(command) {
        return Some(
            "Warning: git reset --hard will discard ALL uncommitted changes permanently. \
             Consider `git stash` to save changes first, or `git reset --mixed` for a softer reset."
                .to_owned(),
        );
    }
    None
}

/// Detects `git push --force` or `git push -f` which overwrites remote history.
fn check_git_push_force(command: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\bgit\s+push\b.*(--force\b|--force-with-lease\b|-f\b)")
            .expect("valid regex")
    });
    if RE.is_match(command) {
        return Some(
            "Warning: force push overwrites remote history. This can cause data loss for collaborators. \
             Consider `git push --force-with-lease` for a safer alternative."
                .to_owned(),
        );
    }
    None
}

/// Detects `git clean -fdx` which permanently deletes untracked files.
fn check_git_clean(command: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bgit\s+clean\b").expect("valid regex"));
    if RE.is_match(command) {
        let lower = command.to_ascii_lowercase();
        let has_force = lower.contains("-f") || lower.contains("--force");
        let has_dry_run = lower.contains("--dry-run") || git_clean_has_dry_run_flag(&lower);
        if has_force && !has_dry_run {
            return Some(
                "Warning: git clean with -f permanently deletes untracked files and directories. \
                 Run with -n first to preview what will be deleted."
                    .to_owned(),
            );
        }
    }
    None
}

/// Checks if a `git clean` command has a dry-run flag (-n) in its combined flags.
/// Handles combined flags like `-fdxn`, `-dfn`, `-fxn`, etc.
fn git_clean_has_dry_run_flag(lower_command: &str) -> bool {
    // Extract the part after "git clean"
    let after_clean = match lower_command.find("clean") {
        Some(idx) => &lower_command[idx + 5..],
        None => return false,
    };

    // Look for standalone -n or --dry-run
    if after_clean.contains("--dry-run") {
        return true;
    }

    // Look for combined short flags like -fdxn
    // Find all -X patterns and check if any contain 'n'
    for part in after_clean.split_whitespace() {
        if part.starts_with('-') && !part.starts_with("--") && part.len() > 1 {
            // This is a combined short flag like -fdxn
            if part.contains('n') {
                return true;
            }
        }
    }

    false
}

/// Detects `git checkout -- <file>` which discards working directory changes.
fn check_git_checkout_discard(command: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+checkout\s+--\s").expect("valid regex"));
    if RE.is_match(command) {
        return Some(
            "Warning: git checkout -- <file> discards uncommitted changes to the specified files. \
             Consider `git stash` to save changes first."
                .to_owned(),
        );
    }
    // Also check git restore --source (newer syntax)
    static RE_RESTORE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+restore\b.*--source\b").expect("valid regex"));
    if RE_RESTORE.is_match(command) {
        return Some(
            "Warning: git restore --source discards changes by restoring from another source. \
             Consider `git stash` to save changes first."
                .to_owned(),
        );
    }
    None
}

/// Detects `git rebase` operations which rewrite history.
fn check_git_rebase(command: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+rebase\b").expect("valid regex"));
    if RE.is_match(command) {
        let lower = command.to_ascii_lowercase();
        // Interactive rebase is blocked elsewhere; warn about any rebase
        if lower.contains("--interactive") || lower.contains("-i") {
            return Some(
                "Warning: interactive rebase opens an editor which is not supported in this tool. \
                 Use non-interactive rebase instead."
                    .to_owned(),
            );
        }
        return Some(
            "Warning: rebase rewrites commit history. If the branch has been pushed, \
             this will require a force push to update the remote."
                .to_owned(),
        );
    }
    None
}

/// Detects `git commit --amend` which modifies the last commit.
fn check_git_amend(command: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+commit\b.*--amend\b").expect("valid regex"));
    if RE.is_match(command) {
        return Some(
            "Note: amending a commit modifies existing history. If this commit has already been \
             pushed, you will need to force push to update the remote. \
             Consider creating a new commit instead."
                .to_owned(),
        );
    }
    None
}

/// Detects `git stash drop` or `git stash clear` which permanently removes stashed changes.
fn check_git_stash_drop(command: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bgit\s+stash\s+(drop|clear)\b").expect("valid regex"));
    if RE.is_match(command) {
        return Some(
            "Warning: git stash drop/clear permanently removes stashed changes. \
             Consider applying the stash first with `git stash pop` or `git stash apply`."
                .to_owned(),
        );
    }
    None
}

/// Detects `--no-verify` or GPG bypass flags.
fn check_git_no_verify(command: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)\bgit\s+\w+\b.*(--no-verify\b|--no-gpg-sign\b|commit\.gpgsign\s*=\s*false)",
        )
        .expect("valid regex")
    });
    if RE.is_match(command) {
        return Some(
            "Warning: --no-verify skips git hooks. Hooks often enforce important checks \
             (linting, tests, commit message format). Only use this if explicitly instructed."
                .to_owned(),
        );
    }
    None
}

/// Detects potential bare-repo attacks via git-internal path manipulation.
fn check_git_bare_repo_attack(command: &str) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    // Check for writes to .git/ internal paths
    if lower.contains(".git\\hooks") || lower.contains(".git/hooks") {
        return Some(
            "Warning: command modifies git hook files. This can alter git behavior. \
             Ensure you trust the source of these changes."
                .to_owned(),
        );
    }
    // Check for creation of git-internal files at repo root (bare repo attack)
    let has_head =
        lower.contains("head") && !lower.contains("headers") && !lower.contains("get-head");
    let has_objects = lower.contains("objects") && !lower.contains("get-");
    let has_refs = lower.contains("refs");
    if has_head && (has_objects || has_refs) {
        return Some(
            "Warning: command creates git-internal files (HEAD, objects, refs) which could be \
             used for a bare-repository attack."
                .to_owned(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::check_powershell_git_safety;

    #[test]
    fn test_git_reset_hard() {
        let result = check_powershell_git_safety("git reset --hard HEAD~1");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("reset --hard"));
    }

    #[test]
    fn test_git_push_force() {
        let result = check_powershell_git_safety("git push --force origin main");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("force push"));

        let result = check_powershell_git_safety("git push -f origin main");
        assert!(result.is_some());
    }

    #[test]
    fn test_git_clean_force() {
        let result = check_powershell_git_safety("git clean -fdx");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("clean"));
    }

    #[test]
    fn test_git_clean_dry_run_ok() {
        let result = check_powershell_git_safety("git clean -fdxn");
        assert!(result.is_none());
    }

    #[test]
    fn test_git_checkout_discard() {
        let result = check_powershell_git_safety("git checkout -- file.txt");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("checkout"));
    }

    #[test]
    fn test_git_rebase() {
        let result = check_powershell_git_safety("git rebase main");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("rebase"));
    }

    #[test]
    fn test_git_amend() {
        let result = check_powershell_git_safety("git commit --amend -m 'fixed'");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("amend"));
    }

    #[test]
    fn test_git_stash_drop() {
        let result = check_powershell_git_safety("git stash drop stash@{0}");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("stash"));
    }

    #[test]
    fn test_git_no_verify() {
        let result = check_powershell_git_safety("git commit --no-verify -m 'test'");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("no-verify"));
    }

    #[test]
    fn test_git_safe_commands() {
        assert!(check_powershell_git_safety("git status").is_none());
        assert!(check_powershell_git_safety("git log --oneline").is_none());
        assert!(check_powershell_git_safety("git diff HEAD").is_none());
        assert!(check_powershell_git_safety("git branch -a").is_none());
    }

    #[test]
    fn test_git_push_normal() {
        assert!(check_powershell_git_safety("git push origin main").is_none());
    }

    #[test]
    fn test_git_bare_repo_hooks() {
        let result = check_powershell_git_safety("Set-Content .git/hooks/pre-commit '#!/bin/sh'");
        assert!(result.is_some());
        assert!(result.expect("valid regex").contains("hook"));
    }
}
