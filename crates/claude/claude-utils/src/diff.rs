//! Structured diff utilities.
//!
//! Provides parsing and analysis of unified diff format, including
//! hunk extraction, statistics calculation, and marker escaping.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DiffHunk
// ---------------------------------------------------------------------------

/// A single hunk from a unified diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffHunk {
    /// The starting line number in the old file.
    pub old_start: u32,
    /// The number of lines in the old file.
    pub old_count: u32,
    /// The starting line number in the new file.
    pub new_start: u32,
    /// The number of lines in the new file.
    pub new_count: u32,
    /// The lines in this hunk (including `+`, `-`, ` ` prefixes).
    pub lines: Vec<String>,
}

impl DiffHunk {
    /// Count added lines in this hunk.
    #[must_use]
    pub fn added_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count()
    }

    /// Count removed lines in this hunk.
    #[must_use]
    pub fn removed_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count()
    }

    /// Count context lines in this hunk.
    #[must_use]
    pub fn context_count(&self) -> usize {
        self.lines.iter().filter(|l| l.starts_with(' ')).count()
    }
}

// ---------------------------------------------------------------------------
// Diff stats
// ---------------------------------------------------------------------------

/// Statistics from a diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffStats {
    /// Number of hunks.
    pub hunks: usize,
    /// Total lines added.
    pub added: usize,
    /// Total lines removed.
    pub removed: usize,
    /// Total lines changed (added + removed).
    pub changed: usize,
    /// Number of files changed (from diff headers).
    pub files_changed: usize,
}

impl DiffStats {
    /// Calculate the net change in lines.
    #[must_use]
    pub fn net_change(&self) -> isize {
        self.added as isize - self.removed as isize
    }
}

// ---------------------------------------------------------------------------
// Parse unified diff
// ---------------------------------------------------------------------------

/// Parse a unified diff string into hunks.
///
/// Supports the standard unified diff format with `@@` hunk headers.
///
/// # Arguments
///
/// * `diff` — The unified diff string.
///
/// # Returns
///
/// A vector of parsed hunks.
pub fn parse_unified_diff(diff: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut current_old_start = 0u32;
    let mut current_old_count = 0u32;
    let mut current_new_start = 0u32;
    let mut current_new_count = 0u32;
    let mut in_hunk = false;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            // Save previous hunk if any.
            if in_hunk && !current_lines.is_empty() {
                hunks.push(DiffHunk {
                    old_start: current_old_start,
                    old_count: current_old_count,
                    new_start: current_new_start,
                    new_count: current_new_count,
                    lines: std::mem::take(&mut current_lines),
                });
            }

            in_hunk = true;

            // Parse the hunk header: @@ -old_start,old_count +new_start,new_count @@
            if let Some(header_end) = rest.find("@@") {
                let header = &rest[..header_end];
                if let Some((old_part, new_part)) = parse_hunk_header(header) {
                    current_old_start = old_part.0;
                    current_old_count = old_part.1;
                    current_new_start = new_part.0;
                    current_new_count = new_part.1;
                }
            }
        } else if in_hunk {
            // Check for end of hunk (next diff header or non-content line).
            if line.starts_with("diff --git ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
            {
                // End of hunk — save and reset.
                if !current_lines.is_empty() {
                    hunks.push(DiffHunk {
                        old_start: current_old_start,
                        old_count: current_old_count,
                        new_start: current_new_start,
                        new_count: current_new_count,
                        lines: std::mem::take(&mut current_lines),
                    });
                }
                in_hunk = false;
            } else if line.starts_with('+')
                || line.starts_with('-')
                || line.starts_with(' ')
                || line.starts_with('\\')
            {
                current_lines.push(line.to_string());
            }
        }
    }

    // Save the last hunk.
    if in_hunk && !current_lines.is_empty() {
        hunks.push(DiffHunk {
            old_start: current_old_start,
            old_count: current_old_count,
            new_start: current_new_start,
            new_count: current_new_count,
            lines: current_lines,
        });
    }

    hunks
}

/// Parse a hunk header like ` -1,3 +1,4 ` into old and new ranges.
fn parse_hunk_header(header: &str) -> Option<((u32, u32), (u32, u32))> {
    let header = header.trim();

    // Find the old range (starts with -).
    let old_start = header.find('-')?;
    let rest = &header[old_start + 1..];

    // Find the separator between old and new ranges.
    let space_pos = rest.find(' ')?;
    let old_part = &rest[..space_pos];
    let new_part = &rest[space_pos + 1..];

    let old_range = parse_range(old_part)?;
    let new_range = parse_range(new_part.strip_prefix('+')?)?;

    Some((old_range, new_range))
}

/// Parse a range like `1,3` into `(start, count)`.
fn parse_range(range: &str) -> Option<(u32, u32)> {
    if let Some(comma_pos) = range.find(',') {
        let start = range[..comma_pos].parse().ok()?;
        let count = range[comma_pos + 1..].parse().ok()?;
        Some((start, count))
    } else {
        let start = range.parse().ok()?;
        Some((start, 1))
    }
}

/// Calculate diff statistics from a unified diff string.
///
/// # Arguments
///
/// * `diff` — The unified diff string.
///
/// # Returns
///
/// The diff statistics.
#[must_use]
pub fn diff_stats(diff: &str) -> DiffStats {
    let hunks = parse_unified_diff(diff);
    let files_changed = diff
        .lines()
        .filter(|l| l.starts_with("diff --git "))
        .count()
        .max(1);

    let added: usize = hunks.iter().map(|h| h.added_count()).sum();
    let removed: usize = hunks.iter().map(|h| h.removed_count()).sum();

    DiffStats {
        hunks: hunks.len(),
        added,
        removed,
        changed: added + removed,
        files_changed,
    }
}

// ---------------------------------------------------------------------------
// Marker escaping
// ---------------------------------------------------------------------------

/// Escape special diff markers (`&`, `$`) in text to prevent shell expansion.
///
/// Replaces `&` with `&` and `$` with `&#36;`.
///
/// # Arguments
///
/// * `text` — The text to escape.
///
/// # Returns
///
/// The escaped text.
#[must_use]
pub fn escape_diff_markers(text: &str) -> String {
    text.replace('$', "&#36;")
}

/// Unescape diff markers back to original characters.
///
/// # Arguments
///
/// * `text` — The text to unescape.
///
/// # Returns
///
/// The unescaped text.
#[must_use]
pub fn unescape_diff_markers(text: &str) -> String {
    text.replace("&#36;", "$")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- DiffHunk ---

    #[test]
    fn diff_hunk_added_count() {
        let hunk = DiffHunk {
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![
                " context".to_string(),
                "+added".to_string(),
                "-removed".to_string(),
                "+added2".to_string(),
            ],
        };
        assert_eq!(hunk.added_count(), 2);
    }

    #[test]
    fn diff_hunk_removed_count() {
        let hunk = DiffHunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 1,
            lines: vec![" context".to_string(), "-removed".to_string()],
        };
        assert_eq!(hunk.removed_count(), 1);
    }

    #[test]
    fn diff_hunk_context_count() {
        let hunk = DiffHunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 2,
            lines: vec![
                " ctx1".to_string(),
                " ctx2".to_string(),
                "+added".to_string(),
            ],
        };
        assert_eq!(hunk.context_count(), 2);
    }

    // --- parse_unified_diff ---

    #[test]
    fn parse_simple_diff() {
        let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 line1
+added line
 line2
 line3";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
        assert_eq!(hunks[0].added_count(), 1);
    }

    #[test]
    fn parse_multi_hunk_diff() {
        let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 line1
-old
+new
 line3
@@ -10,2 +10,3 @@
 line10
+inserted
 line11";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[1].old_start, 10);
        assert_eq!(hunks[1].added_count(), 1);
    }

    #[test]
    fn parse_empty_diff() {
        let hunks = parse_unified_diff("");
        assert!(hunks.is_empty());
    }

    #[test]
    fn parse_diff_no_hunks() {
        let diff = "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n";
        let hunks = parse_unified_diff(diff);
        assert!(hunks.is_empty());
    }

    // --- diff_stats ---

    #[test]
    fn diff_stats_simple() {
        let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 line1
+added line
 line2
 line3";
        let stats = diff_stats(diff);
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.hunks, 1);
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.changed, 1);
    }

    #[test]
    fn diff_stats_net_change() {
        let stats = DiffStats {
            hunks: 1,
            added: 5,
            removed: 3,
            changed: 8,
            files_changed: 1,
        };
        assert_eq!(stats.net_change(), 2);
    }

    #[test]
    fn diff_stats_negative_net() {
        let stats = DiffStats {
            hunks: 1,
            added: 1,
            removed: 5,
            changed: 6,
            files_changed: 1,
        };
        assert_eq!(stats.net_change(), -4);
    }

    // --- escape_diff_markers ---

    #[test]
    fn escape_markers_ampersand() {
        assert_eq!(escape_diff_markers("a & b"), "a & b");
    }

    #[test]
    fn escape_markers_dollar() {
        assert_eq!(escape_diff_markers("$HOME"), "&#36;HOME");
    }

    #[test]
    fn escape_markers_both() {
        assert_eq!(escape_diff_markers("a & $b"), "a & &#36;b");
    }

    #[test]
    fn escape_markers_none() {
        assert_eq!(escape_diff_markers("hello world"), "hello world");
    }

    #[test]
    fn escape_markers_empty() {
        assert_eq!(escape_diff_markers(""), "");
    }

    // --- unescape_diff_markers ---

    #[test]
    fn unescape_markers() {
        assert_eq!(unescape_diff_markers("a & &#36;b"), "a & $b");
    }

    #[test]
    fn escape_unescape_roundtrip() {
        let original = "foo & bar $baz";
        assert_eq!(
            unescape_diff_markers(&escape_diff_markers(original)),
            original
        );
    }

    // --- DiffStats serialization ---

    #[test]
    fn diff_stats_serialization_roundtrip() {
        let stats = DiffStats {
            hunks: 2,
            added: 10,
            removed: 5,
            changed: 15,
            files_changed: 3,
        };
        let json = serde_json::to_string(&stats).expect("serialize");
        let deserialized: DiffStats = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(stats, deserialized);
    }

    // --- DiffHunk serialization ---

    #[test]
    fn diff_hunk_serialization_roundtrip() {
        let hunk = DiffHunk {
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![" ctx".to_string(), "+add".to_string()],
        };
        let json = serde_json::to_string(&hunk).expect("serialize");
        let deserialized: DiffHunk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hunk, deserialized);
    }
}
