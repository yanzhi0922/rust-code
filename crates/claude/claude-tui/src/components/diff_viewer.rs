//! File diff viewer component.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

/// A single diff line.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub content: String,
    pub kind: DiffLineKind,
}

/// Kind of diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Context line (unchanged).
    Context,
    /// Added line.
    Add,
    /// Removed line.
    Remove,
    /// Hunk header (`@@ ... @@`).
    HunkHeader,
    /// File header (`--- a/file` or `+++ b/file`).
    FileHeader,
}

/// Parse a unified diff into DiffLine entries.
pub fn parse_diff(diff_text: &str) -> Vec<DiffLine> {
    diff_text
        .lines()
        .map(|line| {
            let kind = if line.starts_with("@@") {
                DiffLineKind::HunkHeader
            } else if line.starts_with("+++") || line.starts_with("---") {
                DiffLineKind::FileHeader
            } else if line.starts_with('+') {
                DiffLineKind::Add
            } else if line.starts_with('-') {
                DiffLineKind::Remove
            } else {
                DiffLineKind::Context
            };
            DiffLine {
                content: line.to_owned(),
                kind,
            }
        })
        .collect()
}

/// Render diff lines into ratatui Lines.
pub fn render_diff(
    diff_lines: &[DiffLine],
    style: &StyleConfig,
    max_lines: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for dl in diff_lines.iter().take(max_lines) {
        let (prefix, color) = match dl.kind {
            DiffLineKind::Context => (" ", style.status_fg),
            DiffLineKind::Add => ("+", style.mode_insert),
            DiffLineKind::Remove => ("-", style.error_color),
            DiffLineKind::HunkHeader => ("@@", style.tool_color),
            DiffLineKind::FileHeader => ("", style.accent_color),
        };

        let content = if dl.content.len() > 200 {
            crate::message::truncate_text(&dl.content, 200)
        } else {
            dl.content.clone()
        };

        lines.push(Line::from(Span::styled(
            format!("{prefix}{content}"),
            Style::default().fg(color),
        )));
    }

    if diff_lines.len() > max_lines {
        lines.push(Line::from(Span::styled(
            format!("  ... ({} more lines)", diff_lines.len() - max_lines),
            Style::default().fg(style.info_color),
        )));
    }

    lines
}

/// Diff statistics.
#[derive(Debug, Clone, Default)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
}

impl DiffStats {
    /// Format a summary string.
    pub fn summary(&self) -> String {
        format!("+{} -{}", self.added, self.removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::StyleConfig;

    #[test]
    fn parse_empty_diff() {
        let lines = parse_diff("");
        assert!(lines.is_empty());
    }

    #[test]
    fn parse_simple_diff() {
        let diff = "@@ -1,3 +1,3 @@\n-old line\n+new line\n context\n";
        let lines = parse_diff(diff);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].kind, DiffLineKind::HunkHeader);
        assert_eq!(lines[1].kind, DiffLineKind::Remove);
        assert_eq!(lines[2].kind, DiffLineKind::Add);
        assert_eq!(lines[3].kind, DiffLineKind::Context);
    }

    #[test]
    fn render_diff_lines() {
        let style = StyleConfig::dark();
        let diff_lines = vec![
            DiffLine {
                content: "@@ -1 +1 @@".to_owned(),
                kind: DiffLineKind::HunkHeader,
            },
            DiffLine {
                content: "-old".to_owned(),
                kind: DiffLineKind::Remove,
            },
            DiffLine {
                content: "+new".to_owned(),
                kind: DiffLineKind::Add,
            },
        ];
        let rendered = render_diff(&diff_lines, &style, 100);
        assert_eq!(rendered.len(), 3);
    }

    #[test]
    fn diff_stats_summary() {
        let stats = DiffStats {
            added: 5,
            removed: 3,
        };
        assert_eq!(stats.summary(), "+5 -3");
    }

    #[test]
    fn render_diff_truncates_long() {
        let style = StyleConfig::dark();
        let diff_lines: Vec<DiffLine> = (0..100)
            .map(|i| DiffLine {
                content: format!("line {i}"),
                kind: DiffLineKind::Context,
            })
            .collect();
        let rendered = render_diff(&diff_lines, &style, 10);
        assert!(rendered.len() > 10); // 10 + truncation notice
    }
}
