//! Message type rendering components.
//!
//! Provides dedicated renderers for each message type in the conversation,
//! mirroring Claude Code's React/Ink component architecture with 20+ message
//! type components. Each renderer produces `Vec<Line<'static>>` for ratatui.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// AssistantTextRenderer
// ---------------------------------------------------------------------------

/// Renders assistant text messages with optional markdown-style formatting.
#[derive(Debug, Clone)]
pub struct AssistantTextRenderer {
    /// The text content to render.
    pub content: String,
    /// Whether the message is currently streaming.
    pub is_streaming: bool,
}

impl AssistantTextRenderer {
    /// Create a new assistant text renderer.
    pub fn new(content: String) -> Self {
        Self {
            content,
            is_streaming: false,
        }
    }

    /// Render the assistant text into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Header line with role indicator.
        let header_icon = if self.is_streaming { "●" } else { "◉" };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {header_icon} "),
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "assistant".to_owned(),
                Style::default()
                    .fg(style.assistant_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Content lines.
        for line in self.content.lines() {
            if line.is_empty() {
                lines.push(Line::from(""));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(line.to_owned(), Style::default().fg(style.assistant_color)),
                ]));
            }
        }

        // Streaming cursor.
        if self.is_streaming {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                Span::styled(
                    "▌".to_owned(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ]));
        }

        if lines.len() <= 1 && self.content.is_empty() {
            lines.push(Line::from(""));
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// UserTextRenderer
// ---------------------------------------------------------------------------

/// Renders user text messages.
#[derive(Debug, Clone)]
pub struct UserTextRenderer {
    /// The text content to render.
    pub content: String,
}

impl UserTextRenderer {
    /// Create a new user text renderer.
    pub fn new(content: String) -> Self {
        Self { content }
    }

    /// Render the user message into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Header line.
        lines.push(Line::from(vec![
            Span::styled(
                " ▸ ",
                Style::default()
                    .fg(style.user_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "user".to_owned(),
                Style::default()
                    .fg(style.user_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Content lines.
        for line in self.content.lines() {
            if line.is_empty() {
                lines.push(Line::from(""));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(line.to_owned(), Style::default().fg(style.user_color)),
                ]));
            }
        }

        if lines.len() <= 1 && self.content.is_empty() {
            lines.push(Line::from(""));
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// ToolUseRenderer
// ---------------------------------------------------------------------------

/// Status of a tool use block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolUseStatus {
    /// Tool is currently running.
    Running,
    /// Tool completed successfully.
    Success,
    /// Tool execution failed.
    Failed,
    /// Tool is pending user approval.
    Pending,
}

impl ToolUseStatus {
    /// Icon for the status.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Running => "⟳",
            Self::Success => "✓",
            Self::Failed => "✗",
            Self::Pending => "◎",
        }
    }

    /// Color for the status indicator.
    pub fn color(self, style: &StyleConfig) -> Color {
        match self {
            Self::Running => style.accent_color,
            Self::Success => style.tool_color,
            Self::Failed => style.error_color,
            Self::Pending => style.info_color,
        }
    }
}

/// Renders tool use blocks with status indicators.
#[derive(Debug, Clone)]
pub struct ToolUseRenderer {
    /// Tool name.
    pub tool_name: String,
    /// Input preview text.
    pub input_preview: String,
    /// Current status.
    pub status: ToolUseStatus,
    /// Duration in milliseconds (0 if still running).
    pub duration_ms: u64,
}

impl ToolUseRenderer {
    /// Create a new tool use renderer.
    pub fn new(tool_name: String, status: ToolUseStatus) -> Self {
        Self {
            tool_name,
            input_preview: String::new(),
            status,
            duration_ms: 0,
        }
    }

    /// Render the tool use block into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let icon = self.status.icon();
        let color = self.status.color(style);

        let duration_str = if self.duration_ms > 0 {
            let secs = self.duration_ms as f64 / 1000.0;
            format!(" ({secs:.1}s)")
        } else {
            String::new()
        };

        // Header line.
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("[tool] {}", self.tool_name),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(duration_str, Style::default().fg(style.info_color)),
        ]));

        // Input preview.
        if !self.input_preview.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("     ", Style::default()),
                Span::styled(
                    self.input_preview.clone(),
                    Style::default().fg(style.info_color),
                ),
            ]));
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// ToolResultRenderer
// ---------------------------------------------------------------------------

/// Result status of a tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResultStatus {
    /// Tool succeeded.
    Success,
    /// Tool returned an error.
    Error,
    /// User denied the tool execution.
    Denied,
    /// Tool execution was cancelled.
    Cancelled,
}

impl ToolResultStatus {
    /// Icon for the result status.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Error => "✗",
            Self::Denied => "⊘",
            Self::Cancelled => "⊘",
        }
    }

    /// Color for the result status.
    pub fn color(self, style: &StyleConfig) -> Color {
        match self {
            Self::Success => style.tool_color,
            Self::Error => style.error_color,
            Self::Denied => style.info_color,
            Self::Cancelled => style.info_color,
        }
    }

    /// Label for the result status.
    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Renders tool results with success/error/denied/cancelled indicators.
#[derive(Debug, Clone)]
pub struct ToolResultRenderer {
    /// Tool name that produced this result.
    pub tool_name: String,
    /// Result status.
    pub status: ToolResultStatus,
    /// Output text (may be multi-line).
    pub output: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

impl ToolResultRenderer {
    /// Create a new tool result renderer.
    pub fn new(tool_name: String, status: ToolResultStatus) -> Self {
        Self {
            tool_name,
            status,
            output: String::new(),
            duration_ms: 0,
        }
    }

    /// Render the tool result into ratatui lines.
    pub fn render(&self, style: &StyleConfig, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let icon = self.status.icon();
        let color = self.status.color(style);
        let label = self.status.label();

        let duration_str = if self.duration_ms > 0 {
            let secs = self.duration_ms as f64 / 1000.0;
            format!(" ({secs:.1}s)")
        } else {
            String::new()
        };

        // Header line.
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("[{label}] {}", self.tool_name),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(duration_str, Style::default().fg(style.info_color)),
        ]));

        // Output lines (limited by width and count).
        if !self.output.is_empty() {
            let max_lines = 10;
            let content_width = width.saturating_sub(6);
            for line in self.output.lines().take(max_lines) {
                let truncated = if line.len() > content_width {
                    &line[..content_width]
                } else {
                    line
                };
                lines.push(Line::from(vec![
                    Span::styled("     ", Style::default()),
                    Span::styled(truncated.to_owned(), Style::default().fg(style.status_fg)),
                ]));
            }
            let total = self.output.lines().count();
            if total > max_lines {
                lines.push(Line::from(vec![Span::styled(
                    format!("     ... ({} more lines)", total - max_lines),
                    Style::default().fg(style.info_color),
                )]));
            }
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// ThinkingRenderer
// ---------------------------------------------------------------------------

/// Renders thinking blocks (collapsible).
#[derive(Debug, Clone)]
pub struct ThinkingRenderer {
    /// The thinking content.
    pub content: String,
    /// Whether the block is collapsed.
    pub is_collapsed: bool,
}

impl ThinkingRenderer {
    /// Create a new thinking renderer.
    pub fn new(content: String) -> Self {
        Self {
            content,
            is_collapsed: false,
        }
    }

    /// Render the thinking block into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let collapse_icon = if self.is_collapsed { "▸" } else { "▾" };

        // Header line.
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {collapse_icon} "),
                Style::default().fg(style.info_color),
            ),
            Span::styled(
                "thinking".to_owned(),
                Style::default()
                    .fg(style.info_color)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));

        if !self.is_collapsed {
            // Content lines (dimmed).
            for line in self.content.lines() {
                if line.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("   ", Style::default()),
                        Span::styled(line.to_owned(), Style::default().fg(style.info_color)),
                    ]));
                }
            }
        } else {
            // Show a one-line summary when collapsed.
            let summary = self
                .content
                .lines()
                .next()
                .map(|s| {
                    if s.len() > 60 {
                        format!("{}...", &s[..57])
                    } else {
                        s.to_owned()
                    }
                })
                .unwrap_or_default();
            if !summary.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(summary, Style::default().fg(style.info_color)),
                ]));
            }
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// CompactBoundaryRenderer
// ---------------------------------------------------------------------------

/// Renders compact boundary markers between message sections.
#[derive(Debug, Clone)]
pub struct CompactBoundaryRenderer {
    /// Label for the boundary (e.g., "compact boundary").
    pub label: String,
    /// Number of messages that were compacted.
    pub compacted_count: usize,
}

impl CompactBoundaryRenderer {
    /// Create a new compact boundary renderer.
    pub fn new(label: String, compacted_count: usize) -> Self {
        Self {
            label,
            compacted_count,
        }
    }

    /// Render the compact boundary into ratatui lines.
    pub fn render(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let info = format!(
            "── {} ({} messages compacted) ──",
            self.label, self.compacted_count
        );
        let padding = width.saturating_sub(info.len());
        let left_pad = padding / 2;

        // Separator line.
        lines.push(Line::from(vec![
            Span::styled("─".repeat(left_pad), Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {} ", self.label),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("({} messages compacted) ", self.compacted_count),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "─".repeat(width.saturating_sub(
                    left_pad + self.label.len() + self.compacted_count.to_string().len() + 26,
                )),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        lines
    }
}

// ---------------------------------------------------------------------------
// SystemMessageRenderer
// ---------------------------------------------------------------------------

/// Renders system messages (info, warning, error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemMessageLevel {
    Info,
    Warning,
    Error,
}

impl SystemMessageLevel {
    /// Icon for the level.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Info => "ℹ",
            Self::Warning => "⚠",
            Self::Error => "✕",
        }
    }

    /// Color for the level.
    pub fn color(self, style: &StyleConfig) -> Color {
        match self {
            Self::Info => style.info_color,
            Self::Warning => style.tool_color,
            Self::Error => style.error_color,
        }
    }
}

/// Renders system messages with appropriate styling.
#[derive(Debug, Clone)]
pub struct SystemMessageRenderer {
    /// Message content.
    pub content: String,
    /// Message severity level.
    pub level: SystemMessageLevel,
}

impl SystemMessageRenderer {
    /// Create a new system message renderer.
    pub fn new(content: String, level: SystemMessageLevel) -> Self {
        Self { content, level }
    }

    /// Render the system message into ratatui lines.
    pub fn render(&self, style: &StyleConfig) -> Vec<Line<'static>> {
        let icon = self.level.icon();
        let color = self.level.color(style);

        let mut lines = Vec::new();

        for line in self.content.lines() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {icon} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(line.to_owned(), Style::default().fg(color)),
            ]));
        }

        if lines.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                format!(" {icon} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )]));
        }

        lines
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    // ---- AssistantTextRenderer tests ----

    #[test]
    fn assistant_text_basic() {
        let r = AssistantTextRenderer::new("Hello world".to_owned());
        let lines = r.render(&test_style());
        // Header + content line
        assert!(lines.len() >= 2);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn assistant_text_multiline() {
        let r = AssistantTextRenderer::new("line1\nline2\nline3".to_owned());
        let lines = r.render(&test_style());
        // Header + 3 content lines
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn assistant_text_streaming() {
        let mut r = AssistantTextRenderer::new("thinking...".to_owned());
        r.is_streaming = true;
        let lines = r.render(&test_style());
        // Header + content + cursor
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn assistant_text_empty() {
        let r = AssistantTextRenderer::new(String::new());
        let lines = r.render(&test_style());
        // Header + empty line
        assert!(lines.len() >= 2);
    }

    // ---- UserTextRenderer tests ----

    #[test]
    fn user_text_basic() {
        let r = UserTextRenderer::new("Hello".to_owned());
        let lines = r.render(&test_style());
        assert!(lines.len() >= 2);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn user_text_multiline() {
        let r = UserTextRenderer::new("a\nb\nc".to_owned());
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 4); // header + 3 lines
    }

    #[test]
    fn user_text_empty() {
        let r = UserTextRenderer::new(String::new());
        let lines = r.render(&test_style());
        assert!(lines.len() >= 2);
    }

    // ---- ToolUseRenderer tests ----

    #[test]
    fn tool_use_running() {
        let r = ToolUseRenderer::new("bash".to_owned(), ToolUseStatus::Running);
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 1); // header only, no input
    }

    #[test]
    fn tool_use_with_input() {
        let mut r = ToolUseRenderer::new("read_file".to_owned(), ToolUseStatus::Success);
        r.input_preview = "src/main.rs".to_owned();
        r.duration_ms = 500;
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 2); // header + input
    }

    #[test]
    fn tool_use_status_icons() {
        assert_eq!(ToolUseStatus::Running.icon(), "⟳");
        assert_eq!(ToolUseStatus::Success.icon(), "✓");
        assert_eq!(ToolUseStatus::Failed.icon(), "✗");
        assert_eq!(ToolUseStatus::Pending.icon(), "◎");
    }

    // ---- ToolResultRenderer tests ----

    #[test]
    fn tool_result_success() {
        let r = ToolResultRenderer::new("bash".to_owned(), ToolResultStatus::Success);
        let lines = r.render(&test_style(), 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn tool_result_error_with_output() {
        let mut r = ToolResultRenderer::new("bash".to_owned(), ToolResultStatus::Error);
        r.output = "error line 1\nerror line 2".to_owned();
        r.duration_ms = 1000;
        let lines = r.render(&test_style(), 80);
        // header + 2 output lines
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn tool_result_denied() {
        let r = ToolResultRenderer::new("rm".to_owned(), ToolResultStatus::Denied);
        let lines = r.render(&test_style(), 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn tool_result_status_labels() {
        assert_eq!(ToolResultStatus::Success.label(), "success");
        assert_eq!(ToolResultStatus::Error.label(), "error");
        assert_eq!(ToolResultStatus::Denied.label(), "denied");
        assert_eq!(ToolResultStatus::Cancelled.label(), "cancelled");
    }

    // ---- ThinkingRenderer tests ----

    #[test]
    fn thinking_expanded() {
        let r = ThinkingRenderer::new("deep thought\nmore thought".to_owned());
        let lines = r.render(&test_style());
        // header + 2 content lines
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn thinking_collapsed() {
        let mut r = ThinkingRenderer::new("deep thought\nmore thought".to_owned());
        r.is_collapsed = true;
        let lines = r.render(&test_style());
        // header + summary (1 line)
        assert_eq!(lines.len(), 2);
    }

    // ---- CompactBoundaryRenderer tests ----

    #[test]
    fn compact_boundary() {
        let r = CompactBoundaryRenderer::new("context compacted".to_owned(), 15);
        let lines = r.render(80);
        assert_eq!(lines.len(), 1);
    }

    // ---- SystemMessageRenderer tests ----

    #[test]
    fn system_info_message() {
        let r = SystemMessageRenderer::new("Session started".to_owned(), SystemMessageLevel::Info);
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn system_error_message() {
        let r =
            SystemMessageRenderer::new("Connection failed".to_owned(), SystemMessageLevel::Error);
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn system_multiline_warning() {
        let r = SystemMessageRenderer::new(
            "Warning line 1\nWarning line 2".to_owned(),
            SystemMessageLevel::Warning,
        );
        let lines = r.render(&test_style());
        assert_eq!(lines.len(), 2);
    }
}
