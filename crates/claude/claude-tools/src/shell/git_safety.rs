#[must_use]
pub fn is_destructive_git_command(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    DESTRUCTIVE_GIT_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

const DESTRUCTIVE_GIT_PATTERNS: &[&str] = &[
    "git reset --hard",
    "git clean -fd",
    "git clean -fx",
    "git clean -fdx",
    "git checkout --",
    "git restore --source",
    "git push --force",
    "git push -f",
];

#[cfg(test)]
mod tests {
    use super::is_destructive_git_command;

    #[test]
    fn destructive_git_patterns_are_detected() {
        assert!(is_destructive_git_command("git reset --hard HEAD"));
        assert!(is_destructive_git_command("git clean -fdx"));
        assert!(!is_destructive_git_command("git status"));
    }
}
