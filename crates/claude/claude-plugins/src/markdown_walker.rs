//! Plugin markdown file walking and parsing.
//!
//! Rust equivalent of `walkPluginMarkdown.ts`. Recursively walks a plugin
//! directory, collecting `.md` files and parsing them into structured
//! sections for skill discovery and plugin documentation extraction.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors during markdown walking or parsing.
#[derive(Debug, Error)]
pub enum MarkdownError {
    /// I/O error reading a file or directory.
    #[error("markdown I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to read a markdown file.
    #[error("failed to read markdown file '{path}': {source}")]
    ReadFailed {
        /// Path to the file.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// MarkdownSection
// ---------------------------------------------------------------------------

/// A parsed section from a markdown document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownSection {
    /// Section heading text (without the `#` prefix).
    pub title: String,
    /// Heading level (1–6, corresponding to `#` through `######`).
    pub level: u8,
    /// Content of the section (everything between this heading and the next).
    pub content: String,
    /// Line number where the heading starts (1-based).
    pub line: usize,
}

// ---------------------------------------------------------------------------
// ParsedMarkdown
// ---------------------------------------------------------------------------

/// A fully parsed markdown document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedMarkdown {
    /// Sections extracted from the document.
    pub sections: Vec<MarkdownSection>,
    /// Everything before the first heading.
    pub preamble: String,
}

// ---------------------------------------------------------------------------
// Walk result
// ---------------------------------------------------------------------------

/// Information about a discovered markdown file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownFile {
    /// Full path to the markdown file.
    pub path: PathBuf,
    /// Subdirectory components relative to the root (namespace).
    pub namespace: Vec<String>,
    /// Parsed content of the file.
    pub parsed: ParsedMarkdown,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a markdown string into structured sections.
pub fn parse_markdown_sections(content: &str) -> ParsedMarkdown {
    let mut sections: Vec<MarkdownSection> = Vec::new();
    let mut preamble_lines: Vec<&str> = Vec::new();
    let mut current_section: Option<MarkdownSection> = None;
    let mut content_lines: Vec<&str> = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim_start();

        if let Some(rest) = heading_text(trimmed) {
            // Flush previous section
            if let Some(mut sec) = current_section.take() {
                sec.content = content_lines.join("\n").trim_end().to_string();
                sections.push(sec);
                content_lines.clear();
            }

            let level = heading_level(trimmed);
            current_section = Some(MarkdownSection {
                title: rest.to_string(),
                level,
                content: String::new(),
                line: line_num,
            });
        } else if current_section.is_some() {
            content_lines.push(line);
        } else {
            preamble_lines.push(line);
        }
    }

    // Flush last section
    if let Some(mut sec) = current_section.take() {
        sec.content = content_lines.join("\n").trim_end().to_string();
        sections.push(sec);
    }

    ParsedMarkdown {
        sections,
        preamble: preamble_lines.join("\n").trim_end().to_string(),
    }
}

/// Extract the first paragraph of text as a description from markdown.
///
/// Skips headings and blank lines, returns the first contiguous block of
/// non-empty, non-heading text.
pub fn extract_plugin_description(content: &str) -> Option<String> {
    let mut in_paragraph = false;
    let mut paragraph_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip headings
        if heading_text(trimmed).is_some() {
            if in_paragraph && !paragraph_lines.is_empty() {
                break;
            }
            continue;
        }

        if trimmed.is_empty() {
            if in_paragraph {
                break;
            }
            continue;
        }

        in_paragraph = true;
        paragraph_lines.push(trimmed);
    }

    if paragraph_lines.is_empty() {
        None
    } else {
        Some(paragraph_lines.join(" "))
    }
}

/// Check if a line is a markdown heading and return the heading text.
fn heading_text(line: &str) -> Option<&str> {
    if line.starts_with('#') {
        let hash_end = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hash_end) {
            let rest = &line[hash_end..];
            let text = rest.trim_start();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Get the heading level (number of `#` characters).
fn heading_level(line: &str) -> u8 {
    line.chars().take_while(|c| *c == '#').count() as u8
}

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

/// Regex pattern for `skill.md` files (case-insensitive).
const SKILL_MD_PATTERN: &str = "skill.md";

/// Walk a plugin directory, collecting all `.md` files with their parsed
/// content.
///
/// When `stop_at_skill_dir` is `true`, directories containing a `skill.md`
/// file are treated as leaf containers — `.md` files in them are collected
/// but subdirectories are not recursed into.
pub fn walk_plugin_markdown(
    root_dir: &Path,
    stop_at_skill_dir: bool,
) -> Result<Vec<MarkdownFile>, MarkdownError> {
    let mut results: Vec<MarkdownFile> = Vec::new();
    walk_dir_recursive(root_dir, root_dir, stop_at_skill_dir, &mut results)?;
    Ok(results)
}

fn walk_dir_recursive(
    root: &Path,
    current: &Path,
    stop_at_skill_dir: bool,
    results: &mut Vec<MarkdownFile>,
) -> Result<(), MarkdownError> {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return Ok(()), // Swallow errors — one bad dir shouldn't abort
    };

    let entry_list: Vec<std::fs::DirEntry> = entries.filter_map(|e| e.ok()).collect();

    // Check if this directory contains skill.md
    let has_skill_md = stop_at_skill_dir
        && entry_list.iter().any(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.file_name().to_ascii_lowercase() == OsStr::new(SKILL_MD_PATTERN)
        });

    // Compute namespace (subdirectory path relative to root)
    let namespace = current
        .strip_prefix(root)
        .unwrap_or(Path::new(""))
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(|s| s.to_string()),
            _ => None,
        })
        .collect::<Vec<String>>();

    for entry in &entry_list {
        let path = entry.path();

        if path.is_dir() {
            if has_skill_md {
                // Don't recurse into subdirs of skill directories
                continue;
            }
            walk_dir_recursive(root, &path, stop_at_skill_dir, results)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(source) => {
                    return Err(MarkdownError::ReadFailed {
                        path: path.clone(),
                        source,
                    });
                }
            };
            let parsed = parse_markdown_sections(&content);
            results.push(MarkdownFile {
                path,
                namespace: namespace.clone(),
                parsed,
            });
        }
    }

    Ok(())
}

/// Walk a plugin directory and return just the file paths (no parsing).
///
/// Useful when callers want to do their own processing.
pub fn walk_markdown_paths(root_dir: &Path) -> Vec<(PathBuf, Vec<String>)> {
    let mut results: Vec<(PathBuf, Vec<String>)> = Vec::new();

    for entry in WalkDir::new(root_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let namespace = path
            .parent()
            .and_then(|p| p.strip_prefix(root_dir).ok())
            .map(|p| {
                p.components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => s.to_str().map(|s| s.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        results.push((path.to_path_buf(), namespace));
    }

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -- parse_markdown_sections --

    #[test]
    fn parse_simple_sections() {
        let md = "# Title\n\nSome intro\n\n## Details\n\nDetail content\n";
        let parsed = parse_markdown_sections(md);
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].title, "Title");
        assert_eq!(parsed.sections[0].level, 1);
        assert_eq!(parsed.sections[1].title, "Details");
        assert_eq!(parsed.sections[1].level, 2);
        assert!(parsed.sections[1].content.contains("Detail content"));
    }

    #[test]
    fn parse_preamble() {
        let md = "Intro text\n\n# Heading\nContent\n";
        let parsed = parse_markdown_sections(md);
        assert_eq!(parsed.preamble, "Intro text");
        assert_eq!(parsed.sections.len(), 1);
    }

    #[test]
    fn parse_empty() {
        let parsed = parse_markdown_sections("");
        assert!(parsed.sections.is_empty());
        assert!(parsed.preamble.is_empty());
    }

    #[test]
    fn parse_no_headings() {
        let md = "Just some text\non multiple lines\n";
        let parsed = parse_markdown_sections(md);
        assert!(parsed.sections.is_empty());
        assert_eq!(parsed.preamble, "Just some text\non multiple lines");
    }

    #[test]
    fn parse_nested_headings() {
        let md = "# H1\n\nh1 content\n\n## H2\n\nh2 content\n\n### H3\n\nh3 content\n";
        let parsed = parse_markdown_sections(md);
        assert_eq!(parsed.sections.len(), 3);
        assert_eq!(parsed.sections[0].level, 1);
        assert_eq!(parsed.sections[1].level, 2);
        assert_eq!(parsed.sections[2].level, 3);
    }

    #[test]
    fn parse_line_numbers() {
        let md = "# First\n\ncontent\n\n# Second\n";
        let parsed = parse_markdown_sections(md);
        assert_eq!(parsed.sections[0].line, 1);
        assert_eq!(parsed.sections[1].line, 5);
    }

    // -- extract_plugin_description --

    #[test]
    fn extract_description_from_preamble() {
        let md = "This is a great plugin.\n\n# Installation\n\nInstall it.\n";
        let desc = extract_plugin_description(md).expect("should find");
        assert_eq!(desc, "This is a great plugin.");
    }

    #[test]
    fn extract_description_skips_headings() {
        let md = "# Plugin Name\n\nThe actual description.\n";
        let desc = extract_plugin_description(md).expect("should find");
        assert_eq!(desc, "The actual description.");
    }

    #[test]
    fn extract_description_none_for_empty() {
        assert!(extract_plugin_description("").is_none());
        assert!(extract_plugin_description("# Only Heading\n").is_none());
    }

    #[test]
    fn extract_description_multiline_paragraph() {
        let md = "Line one\nline two\nline three\n\n# Next\n";
        let desc = extract_plugin_description(md).expect("should find");
        assert_eq!(desc, "Line one line two line three");
    }

    // -- walk_plugin_markdown --

    #[test]
    fn walk_collects_md_files() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("readme.md"), "# Readme\nContent\n").expect("write");
        fs::create_dir(dir.path().join("skills")).expect("dir");
        fs::write(dir.path().join("skills").join("skill.md"), "# Skill\n").expect("write");

        let files = walk_plugin_markdown(dir.path(), false).expect("walk");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn walk_stop_at_skill_dir() {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir_all(dir.path().join("skills").join("sub")).expect("dir");
        fs::write(dir.path().join("skills").join("skill.md"), "# Skill\n").expect("write");
        fs::write(
            dir.path().join("skills").join("sub").join("extra.md"),
            "# Extra\n",
        )
        .expect("write");

        // With stop_at_skill_dir=true, sub/ should not be recursed
        let files = walk_plugin_markdown(dir.path(), true).expect("walk");
        assert_eq!(files.len(), 1); // Only skill.md, not sub/extra.md
    }

    #[test]
    fn walk_namespace() {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir_all(dir.path().join("foo").join("bar")).expect("dir");
        fs::write(dir.path().join("foo").join("bar").join("doc.md"), "# Doc\n").expect("write");

        let files = walk_plugin_markdown(dir.path(), false).expect("walk");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].namespace, vec!["foo", "bar"]);
    }

    #[test]
    fn walk_empty_dir() {
        let dir = TempDir::new().expect("tempdir");
        let files = walk_plugin_markdown(dir.path(), false).expect("walk");
        assert!(files.is_empty());
    }

    #[test]
    fn walk_markdown_paths_returns_tuples() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("a.md"), "# A\n").expect("write");
        fs::create_dir(dir.path().join("sub")).expect("dir");
        fs::write(dir.path().join("sub").join("b.md"), "# B\n").expect("write");

        let paths = walk_markdown_paths(dir.path());
        assert_eq!(paths.len(), 2);
    }
}
