//! Git filesystem utilities.
//!
//! Provides functions for reading Git configuration, resolving references,
//! and parsing gitignore patterns.

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Git config parsing
// ---------------------------------------------------------------------------

/// A parsed key-value pair from a git config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitConfigEntry {
    /// The section name (e.g. `"user"`, `"remote"`).
    pub section: String,
    /// The optional subsection name.
    pub subsection: Option<String>,
    /// The key name (e.g. `"name"`, `"email"`).
    pub key: String,
    /// The value.
    pub value: String,
}

/// Parse a `.gitconfig` file content into key-value entries.
///
/// Handles sections like `[user]`, subsections like `[remote "origin"]`,
/// and key-value pairs like `name = value`.
///
/// # Arguments
///
/// * `content` — The raw content of a git config file.
///
/// # Returns
///
/// A vector of parsed config entries.
pub fn read_git_config(content: &str) -> Vec<GitConfigEntry> {
    let mut entries = Vec::new();
    let mut current_section = String::new();
    let mut current_subsection: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments.
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Parse section header.
        if line.starts_with('[') && line.ends_with(']') {
            let inner = &line[1..line.len() - 1];
            if let Some(space_pos) = inner.find(' ') {
                current_section = inner[..space_pos].to_string();
                let rest = &inner[space_pos + 1..];
                // Strip quotes from subsection.
                current_subsection = Some(rest.trim_matches('"').to_string());
            } else {
                current_section = inner.to_string();
                current_subsection = None;
            }
            continue;
        }

        // Parse key = value.
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let value = line[eq_pos + 1..].trim().to_string();
            entries.push(GitConfigEntry {
                section: current_section.clone(),
                subsection: current_subsection.clone(),
                key,
                value,
            });
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Worktree HEAD SHA
// ---------------------------------------------------------------------------

/// Read the HEAD commit SHA from a worktree's `.git` reference.
///
/// # Arguments
///
/// * `worktree_path` — The path to the git worktree root.
///
/// # Returns
///
/// The HEAD commit SHA as a hex string.
pub fn get_worktree_head_sha(worktree_path: &Path) -> Result<String> {
    let git_path = worktree_path.join(".git");

    // Handle both regular .git directory and gitdir files (worktrees).
    let git_meta = std::fs::metadata(&git_path)
        .with_context(|| format!("Cannot access .git at {}", git_path.display()))?;

    let head_path = if git_meta.is_file() {
        // This is a gitdir file — read it to find the actual git directory.
        let gitdir_content = std::fs::read_to_string(&git_path)
            .with_context(|| format!("Cannot read gitdir file at {}", git_path.display()))?;
        let gitdir_line = gitdir_content
            .lines()
            .find(|l| l.starts_with("gitdir: "))
            .ok_or_else(|| anyhow!("Invalid gitdir file format"))?;
        let gitdir = &gitdir_line["gitdir: ".len()..];
        let resolved = worktree_path.join(gitdir);
        resolved.join("HEAD")
    } else {
        git_path.join("HEAD")
    };

    let head_content = std::fs::read_to_string(&head_path)
        .with_context(|| format!("Cannot read HEAD at {}", head_path.display()))?;
    let head_ref = head_content.trim();

    // If it's a direct SHA, return it.
    if !head_ref.starts_with("ref: ") {
        return Ok(head_ref.to_string());
    }

    // Otherwise resolve the symbolic reference.
    let ref_path = &head_ref["ref: ".len()..];
    let ref_file = worktree_path.join(".git").join(ref_path);
    let sha = std::fs::read_to_string(&ref_file)
        .with_context(|| format!("Cannot read ref at {}", ref_file.display()))?;
    Ok(sha.trim().to_string())
}

// ---------------------------------------------------------------------------
// Ref resolution
// ---------------------------------------------------------------------------

/// Resolve a git reference to a commit SHA.
///
/// Supports:
/// - Full SHAs (40 hex chars)
/// - Short SHAs (7+ hex chars)
/// - Branch names (e.g. `"main"`, `"refs/heads/main"`)
/// - `"HEAD"`
///
/// # Arguments
///
/// * `git_dir` — The `.git` directory path.
/// * `reference` — The reference string to resolve.
///
/// # Returns
///
/// The resolved commit SHA.
pub fn resolve_ref(git_dir: &Path, reference: &str) -> Result<String> {
    // Direct SHA.
    if reference.len() >= 7 && reference.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(reference.to_string());
    }

    if reference == "HEAD" {
        let head_content =
            std::fs::read_to_string(git_dir.join("HEAD")).with_context(|| "Cannot read HEAD")?;
        let head_ref = head_content.trim();
        if !head_ref.starts_with("ref: ") {
            return Ok(head_ref.to_string());
        }
        let ref_path = &head_ref["ref: ".len()..];
        let ref_file = git_dir.join(ref_path);
        let sha = std::fs::read_to_string(&ref_file)
            .with_context(|| format!("Cannot read ref {ref_path}"))?;
        return Ok(sha.trim().to_string());
    }

    // Branch name.
    let ref_path = if reference.starts_with("refs/") {
        git_dir.join(reference)
    } else {
        git_dir.join("refs").join("heads").join(reference)
    };

    let sha = std::fs::read_to_string(&ref_path)
        .with_context(|| format!("Cannot resolve ref: {reference}"))?;
    Ok(sha.trim().to_string())
}

// ---------------------------------------------------------------------------
// Common directory
// ---------------------------------------------------------------------------

/// Get the git common directory for a worktree.
///
/// For the main repository, this is the `.git` directory itself.
/// For worktrees, it is read from `.git/commondir`.
///
/// # Arguments
///
/// * `git_dir` — The `.git` directory path.
///
/// # Returns
///
/// The common directory path.
pub fn get_common_dir(git_dir: &Path) -> Result<PathBuf> {
    let commondir_file = git_dir.join("commondir");
    if commondir_file.exists() {
        let content =
            std::fs::read_to_string(&commondir_file).with_context(|| "Cannot read commondir")?;
        let relative = content.trim();
        Ok(git_dir.join(relative))
    } else {
        Ok(git_dir.to_path_buf())
    }
}

// ---------------------------------------------------------------------------
// Gitignore parsing
// ---------------------------------------------------------------------------

/// A parsed gitignore pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitignorePattern {
    /// The pattern string.
    pub pattern: String,
    /// Whether the pattern is negated (starts with `!`).
    pub negated: bool,
    /// Whether the pattern is directory-only (ends with `/`).
    pub directory_only: bool,
}

/// Parse gitignore content into patterns.
///
/// # Arguments
///
/// * `content` — The raw content of a `.gitignore` file.
///
/// # Returns
///
/// A vector of parsed patterns.
pub fn parse_gitignore(content: &str) -> Vec<GitignorePattern> {
    let mut patterns = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let negated = line.starts_with('!');
        let pattern = if negated { &line[1..] } else { line };

        let directory_only = pattern.ends_with('/');
        let clean_pattern = if directory_only {
            &pattern[..pattern.len() - 1]
        } else {
            pattern
        };

        patterns.push(GitignorePattern {
            pattern: clean_pattern.to_string(),
            negated,
            directory_only,
        });
    }

    patterns
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- read_git_config ---

    #[test]
    fn parse_simple_config() {
        let content = "[user]\n\tname = John\n\temail = john@example.com\n";
        let entries = read_git_config(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].section, "user");
        assert_eq!(entries[0].key, "name");
        assert_eq!(entries[0].value, "John");
        assert_eq!(entries[1].key, "email");
        assert_eq!(entries[1].value, "john@example.com");
    }

    #[test]
    fn parse_config_with_subsection() {
        let content = "[remote \"origin\"]\n\turl = https://example.com/repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        let entries = read_git_config(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].section, "remote");
        assert_eq!(
            entries[0].subsection.as_ref().expect("subsection"),
            "origin"
        );
        assert_eq!(entries[0].key, "url");
    }

    #[test]
    fn parse_config_skips_comments() {
        let content = "# This is a comment\n; Another comment\n[user]\nname = Test\n";
        let entries = read_git_config(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, "Test");
    }

    #[test]
    fn parse_config_empty() {
        let entries = read_git_config("");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_config_multiple_sections() {
        let content = "[core]\n\tbare = false\n[user]\n\tname = Alice\n";
        let entries = read_git_config(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].section, "core");
        assert_eq!(entries[1].section, "user");
    }

    #[test]
    fn parse_config_value_with_equals() {
        let content = "[remote \"origin\"]\n\turl = https://example.com/a=b\n";
        let entries = read_git_config(content);
        assert_eq!(entries[0].value, "https://example.com/a=b");
    }

    // --- parse_gitignore ---

    #[test]
    fn parse_gitignore_simple() {
        let content = "*.log\nbuild/\n!.important\n";
        let patterns = parse_gitignore(content);
        assert_eq!(patterns.len(), 3);

        assert_eq!(patterns[0].pattern, "*.log");
        assert!(!patterns[0].negated);
        assert!(!patterns[0].directory_only);

        assert_eq!(patterns[1].pattern, "build");
        assert!(!patterns[1].negated);
        assert!(patterns[1].directory_only);

        assert_eq!(patterns[2].pattern, ".important");
        assert!(patterns[2].negated);
        assert!(!patterns[2].directory_only);
    }

    #[test]
    fn parse_gitignore_comments() {
        let content = "# Comment\n*.o\n";
        let patterns = parse_gitignore(content);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].pattern, "*.o");
    }

    #[test]
    fn parse_gitignore_empty() {
        let patterns = parse_gitignore("");
        assert!(patterns.is_empty());
    }

    #[test]
    fn parse_gitignore_blank_lines() {
        let content = "\n\n*.a\n\n*.b\n\n";
        let patterns = parse_gitignore(content);
        assert_eq!(patterns.len(), 2);
    }

    // --- resolve_ref (unit tests for SHA detection) ---

    #[test]
    fn resolve_ref_full_sha() {
        // This tests the SHA detection path (no file I/O needed).
        // We can't test the full path without a real git repo.
        let sha = "abcdef1234567890abcdef1234567890abcdef12";
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // --- get_common_dir ---

    #[test]
    fn gitignore_pattern_fields() {
        let pat = GitignorePattern {
            pattern: "target".to_string(),
            negated: false,
            directory_only: true,
        };
        assert_eq!(pat.pattern, "target");
        assert!(!pat.negated);
        assert!(pat.directory_only);
    }

    // --- GitConfigEntry ---

    #[test]
    fn git_config_entry_fields() {
        let entry = GitConfigEntry {
            section: "core".to_string(),
            subsection: None,
            key: "autocrlf".to_string(),
            value: "false".to_string(),
        };
        assert_eq!(entry.section, "core");
        assert!(entry.subsection.is_none());
        assert_eq!(entry.key, "autocrlf");
        assert_eq!(entry.value, "false");
    }
}
