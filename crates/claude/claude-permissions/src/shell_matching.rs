//! Shell command rule matching.
//!
//! Corresponds to `src/utils/permissions/shellRuleMatching.ts`.
//! Matches shell commands against permission rules with support for
//! glob patterns and command decomposition.

/// Check if a bash command matches a permission rule pattern.
///
/// Supports:
/// - Exact match: `git status`
/// - Prefix match: `git *`
/// - Glob match: `git push * origin main`
/// - Tool-prefixed match: `Bash(git status)`
#[must_use]
pub fn shell_command_matches_pattern(command: &str, pattern: &str) -> bool {
    let cmd = command.trim();
    let pat = pattern.trim();

    // Exact match
    if cmd == pat {
        return true;
    }

    // Tool-prefixed pattern: "Bash(git status)"
    if let Some(inner) = extract_tool_content(pat) {
        return shell_command_matches_pattern(cmd, inner);
    }

    // Wildcard patterns
    if pat.contains('*') {
        return glob_match_command(cmd, pat);
    }

    false
}

/// Extract content from tool-prefixed patterns like "Bash(git status)".
fn extract_tool_content(pattern: &str) -> Option<&str> {
    if let Some(start) = pattern.find('(')
        && let Some(end) = pattern.rfind(')')
        && start < end
    {
        return Some(&pattern[start + 1..end]);
    }
    None
}

/// Glob-style matching for shell commands.
fn glob_match_command(command: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    match parts.len() {
        1 => command == parts[0],
        2 => {
            let (prefix, suffix) = (parts[0], parts[1]);
            command.starts_with(prefix)
                && command.ends_with(suffix)
                && command.len() >= prefix.len() + suffix.len()
        }
        _ => {
            // Multi-wildcard: use recursive matching
            recursive_glob_match(command, pattern)
        }
    }
}

/// Recursive glob matching for complex patterns.
fn recursive_glob_match(text: &str, pattern: &str) -> bool {
    let mut ti = 0;
    let mut pi = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    let text_chars: Vec<char> = text.chars().collect();
    let pat_chars: Vec<char> = pattern.chars().collect();

    while ti < text_chars.len() {
        if pi < pat_chars.len() && (pat_chars[pi] == text_chars[ti] || pat_chars[pi] == '?') {
            ti += 1;
            pi += 1;
        } else if pi < pat_chars.len() && pat_chars[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }

    pi == pat_chars.len()
}

/// Decompose a compound shell command into individual commands.
/// Handles `&&`, `||`, `;`, `|`, and subshells.
pub fn decompose_command(command: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut current = String::new();
    let mut in_quote = None;
    let mut depth = 0;

    for ch in command.chars() {
        match ch {
            '\'' | '"' if in_quote.is_none() => {
                in_quote = Some(ch);
                current.push(ch);
            }
            '\'' | '"' if in_quote == Some(ch) => {
                in_quote = None;
                current.push(ch);
            }
            '(' if in_quote.is_none() => {
                depth += 1;
                current.push(ch);
            }
            ')' if in_quote.is_none() && depth > 0 => {
                depth -= 1;
                current.push(ch);
            }
            '&' if in_quote.is_none() && depth == 0 => {
                if command.chars().nth(current.len()) == Some('&') {
                    // &&
                    if !current.trim().is_empty() {
                        commands.push(current.trim().to_string());
                    }
                    current.clear();
                }
            }
            '|' if in_quote.is_none() && depth == 0 => {
                if !current.trim().is_empty() {
                    commands.push(current.trim().to_string());
                }
                current.clear();
            }
            ';' if in_quote.is_none() && depth == 0 => {
                if !current.trim().is_empty() {
                    commands.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.trim().is_empty() {
        commands.push(current.trim().to_string());
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(shell_command_matches_pattern("git status", "git status"));
    }

    #[test]
    fn wildcard_prefix() {
        assert!(shell_command_matches_pattern("git status", "git *"));
        assert!(shell_command_matches_pattern(
            "git push origin main",
            "git *"
        ));
    }

    #[test]
    fn wildcard_prefix_suffix() {
        assert!(shell_command_matches_pattern(
            "git push origin main",
            "git push * main"
        ));
    }

    #[test]
    fn tool_prefixed_pattern() {
        assert!(shell_command_matches_pattern(
            "git status",
            "Bash(git status)"
        ));
        assert!(shell_command_matches_pattern("git push", "Bash(git *)"));
    }

    #[test]
    fn no_match() {
        assert!(!shell_command_matches_pattern("npm install", "git *"));
        assert!(!shell_command_matches_pattern("git status", "git push"));
    }

    #[test]
    fn decompose_simple() {
        let cmds = decompose_command("git status");
        assert_eq!(cmds, vec!["git status"]);
    }

    #[test]
    fn decompose_chain() {
        let cmds = decompose_command("git status; git add .");
        assert_eq!(cmds.len(), 2);
        assert!(cmds.contains(&"git status".to_string()));
        assert!(cmds.contains(&"git add .".to_string()));
    }

    #[test]
    fn decompose_pipe() {
        let cmds = decompose_command("cat file.txt | grep pattern");
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn decompose_preserves_quoted() {
        let cmds = decompose_command("echo 'hello; world'");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0], "echo 'hello; world'");
    }

    #[test]
    fn recursive_glob() {
        assert!(recursive_glob_match(
            "git push origin main --force",
            "git * --force"
        ));
        assert!(recursive_glob_match("abc", "a*c"));
        assert!(!recursive_glob_match("ac", "a*b*c"));
    }
}
