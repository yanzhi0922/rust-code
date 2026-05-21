//! Message types and rendering helpers for the TUI.
//!
//! Defines the data model for chat messages, tool calls, permission requests,
//! and provides helpers for converting messages into ratatui renderable form.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

/// Structured display payload for a memory-saved system message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySavedInfo {
    /// Topic memory files written by the background memory agent.
    pub written_paths: Vec<String>,
    /// Number of written files that belong to team memory.
    pub team_count: Option<usize>,
}

/// UI-specific message subtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMessageKind {
    /// Regular role/content message.
    Plain,
    /// Claude Code-compatible memory-saved system message.
    MemorySaved(MemorySavedInfo),
}

/// Role of a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

impl MessageRole {
    /// Short label for display.
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }

    /// Color from the style config.
    pub fn color(self, style: &StyleConfig) -> Color {
        match self {
            Self::User => style.user_color,
            Self::Assistant => style.assistant_color,
            Self::System => style.system_color,
            Self::Tool => style.tool_color,
        }
    }
}

/// Information about a tool call within a message.
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    /// Tool name (e.g. `"bash_command"`).
    pub tool_name: String,
    /// Input to the tool (JSON or text).
    pub input: String,
    /// Output from the tool.
    pub output: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the output is collapsed.
    pub is_collapsed: bool,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// A single chat message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Message role.
    pub role: MessageRole,
    /// Text content.
    pub content: String,
    /// Unix timestamp (seconds).
    pub timestamp: i64,
    /// Tool calls associated with this message.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Whether tool outputs are collapsed.
    pub is_collapsed: bool,
    /// UI-specific subtype information.
    pub kind: ChatMessageKind,
}

impl ChatMessage {
    /// Create a new user message.
    pub fn user(content: String) -> Self {
        ChatMessage {
            role: MessageRole::User,
            content,
            timestamp: 0,
            tool_calls: Vec::new(),
            is_collapsed: false,
            kind: ChatMessageKind::Plain,
        }
    }

    /// Create a new assistant message.
    pub fn assistant(content: String) -> Self {
        ChatMessage {
            role: MessageRole::Assistant,
            content,
            timestamp: 0,
            tool_calls: Vec::new(),
            is_collapsed: false,
            kind: ChatMessageKind::Plain,
        }
    }

    /// Create a new system message.
    pub fn system(content: String) -> Self {
        ChatMessage {
            role: MessageRole::System,
            content,
            timestamp: 0,
            tool_calls: Vec::new(),
            is_collapsed: false,
            kind: ChatMessageKind::Plain,
        }
    }

    /// Create a structured memory-saved system message.
    pub fn memory_saved(written_paths: Vec<String>, team_count: Option<usize>) -> Self {
        let count = written_paths.len();
        let content = if count == 1 {
            "Saved 1 memory".to_owned()
        } else {
            format!("Saved {count} memories")
        };
        ChatMessage {
            role: MessageRole::System,
            content,
            timestamp: 0,
            tool_calls: Vec::new(),
            is_collapsed: false,
            kind: ChatMessageKind::MemorySaved(MemorySavedInfo {
                written_paths,
                team_count,
            }),
        }
    }

    /// Create a new tool result message.
    pub fn tool(content: String) -> Self {
        ChatMessage {
            role: MessageRole::Tool,
            content,
            timestamp: 0,
            tool_calls: Vec::new(),
            is_collapsed: false,
            kind: ChatMessageKind::Plain,
        }
    }

    /// Estimate the number of terminal rows this message will occupy.
    pub fn estimated_height(&self, width: usize) -> usize {
        if width == 0 {
            return 1;
        }
        let content_lines = self.content.lines().count().max(1);
        let wrapped = self
            .content
            .lines()
            .map(|line| {
                let line_len = unicode_width::UnicodeWidthStr::width(line);
                if line_len == 0 {
                    1
                } else {
                    line_len.div_ceil(width)
                }
            })
            .sum::<usize>()
            .max(1);

        let header = 1; // role label line
        let tool_lines: usize = self
            .tool_calls
            .iter()
            .map(|tc| {
                if tc.is_collapsed {
                    1
                } else {
                    2 + tc.output.lines().count().min(10)
                }
            })
            .sum();

        header + content_lines.max(wrapped) + tool_lines
    }
}

/// A permission request to be displayed to the user.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Tool name requesting permission.
    pub tool_name: String,
    /// Description of the operation.
    pub description: String,
    /// Whether "allow all" is available.
    pub allow_all_available: bool,
}

/// Model information for the status bar.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model display name.
    pub name: String,
    /// Provider name.
    pub provider: String,
}

impl Default for ModelInfo {
    fn default() -> Self {
        ModelInfo {
            name: "unknown".to_owned(),
            provider: "unknown".to_owned(),
        }
    }
}

/// Status bar information.
#[derive(Debug, Clone)]
pub struct StatusBarInfo {
    /// Model display name.
    pub model_name: String,
    /// Current token count.
    pub token_count: usize,
    /// Maximum token count.
    pub max_tokens: usize,
    /// Accumulated cost in USD.
    pub cost: f64,
    /// Vim mode label.
    pub mode_label: String,
}

impl Default for StatusBarInfo {
    fn default() -> Self {
        StatusBarInfo {
            model_name: "unknown".to_owned(),
            token_count: 0,
            max_tokens: 0,
            cost: 0.0,
            mode_label: "INSERT".to_owned(),
        }
    }
}

/// MCP server status for the sidebar.
#[derive(Debug, Clone)]
pub struct McpServerStatus {
    /// Server name.
    pub name: String,
    /// Connection status: "connected", "failed", "pending", "needs-auth", "disabled".
    pub status: String,
    /// Number of tools provided by this server.
    pub tool_count: usize,
}

/// Render a role label as a styled Span.
pub fn role_span(role: MessageRole, style: &StyleConfig) -> Span<'static> {
    let color = role.color(style);
    let label = format!("[{}] ", role.label());
    Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Render a single-line preview of a message (for sidebar / compact view).
pub fn message_preview(msg: &ChatMessage, max_chars: usize) -> String {
    let first_line = msg.content.lines().next().unwrap_or("");
    let truncated: String = first_line.chars().take(max_chars).collect();
    if first_line.chars().count() > max_chars {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

/// Truncate text for display.
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

/// Convert a ChatMessage into ratatui Lines for rendering.
pub fn message_to_lines(
    msg: &ChatMessage,
    width: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    if let ChatMessageKind::MemorySaved(info) = &msg.kind {
        return memory_saved_to_lines(info, width, style);
    }

    let mut lines = Vec::new();

    // Header line with role.
    let header = vec![
        role_span(msg.role, style),
        Span::raw(truncate_text(&msg.content, width.saturating_sub(10))),
    ];
    lines.push(Line::from(header));

    // Content lines.
    for content_line in msg.content.lines() {
        let color = msg.role.color(style);
        let wrapped = wrap_text(content_line, width);
        for chunk in wrapped {
            lines.push(Line::from(Span::styled(chunk, Style::default().fg(color))));
        }
    }

    // Tool call summaries.
    for tc in &msg.tool_calls {
        let icon = if tc.is_error { "✗" } else { "✓" };
        let summary = if tc.is_collapsed {
            format!(
                "  {icon} [tool] {} ({:.1}s)",
                tc.tool_name,
                tc.duration_ms as f64 / 1000.0
            )
        } else {
            let output_preview = truncate_text(&tc.output, width.saturating_sub(20));
            format!(
                "  {icon} [tool] {} ({:.1}s)\n    {}",
                tc.tool_name,
                tc.duration_ms as f64 / 1000.0,
                output_preview
            )
        };
        for line in summary.lines() {
            let color = if tc.is_error {
                style.error_color
            } else {
                style.tool_color
            };
            lines.push(Line::from(Span::styled(
                line.to_owned(),
                Style::default().fg(color),
            )));
        }
    }

    // Blank line separator.
    lines.push(Line::from(Span::raw(String::new())));

    lines
}

fn memory_saved_to_lines(
    info: &MemorySavedInfo,
    width: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let count = info.written_paths.len();
    let title = match info.team_count {
        Some(team_count) if team_count > 0 && team_count < count => {
            let private_count = count - team_count;
            format!("Saved {count} memories ({private_count} private, {team_count} team)")
        }
        Some(team_count) if team_count == count && count > 0 => {
            format!(
                "Saved {count} team {}",
                if count == 1 { "memory" } else { "memories" }
            )
        }
        _ if count == 1 => "Saved 1 memory".to_owned(),
        _ => format!("Saved {count} memories"),
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "[system] ",
            Style::default()
                .fg(style.system_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(title, Style::default().fg(style.system_color)),
    ])];

    for path in &info.written_paths {
        let basename = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path);
        let line = format!("  {basename} — {path}");
        lines.push(Line::from(Span::styled(
            truncate_text(&line, width),
            Style::default().fg(style.system_color),
        )));
    }

    lines.push(Line::from(Span::raw(String::new())));
    lines
}

/// Wrap text to fit within a given width, respecting Unicode character widths.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let text_width = unicode_width::UnicodeWidthStr::width(text);
    if text_width <= width {
        return vec![text.to_owned()];
    }

    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > width {
            result.push(current.clone());
            current.clear();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }

    if !current.is_empty() {
        result.push(current);
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_role_label() {
        assert_eq!(MessageRole::User.label(), "user");
        assert_eq!(MessageRole::Assistant.label(), "assistant");
        assert_eq!(MessageRole::System.label(), "system");
        assert_eq!(MessageRole::Tool.label(), "tool");
    }

    #[test]
    fn chat_message_user_constructor() {
        let msg = ChatMessage::user("hello".to_owned());
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn chat_message_estimated_height() {
        let msg = ChatMessage::assistant("line1\nline2\nline3".to_owned());
        let h = msg.estimated_height(80);
        assert!(h >= 4); // 3 content lines + 1 header
    }

    #[test]
    fn message_preview_truncates() {
        let msg = ChatMessage::assistant("a".repeat(200));
        let preview = message_preview(&msg, 50);
        // Preview should be truncated and end with ellipsis.
        assert!(preview.ends_with('…'));
        assert!(preview.len() <= 60); // Allow for Unicode ellipsis width
    }

    #[test]
    fn message_preview_short_text() {
        let msg = ChatMessage::assistant("short".to_owned());
        let preview = message_preview(&msg, 50);
        assert_eq!(preview, "short");
    }

    #[test]
    fn truncate_text_short() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_long() {
        let result = truncate_text("abcdefghij", 5);
        assert_eq!(result, "abcde…");
    }

    #[test]
    fn wrap_text_short() {
        let result = wrap_text("hello", 10);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn wrap_text_long() {
        let result = wrap_text("abcdefghij", 5);
        assert_eq!(result, vec!["abcde", "fghij"]);
    }

    #[test]
    fn message_to_lines_contains_role() {
        let style = StyleConfig::dark();
        let msg = ChatMessage::user("test".to_owned());
        let lines = message_to_lines(&msg, 80, &style);
        assert!(!lines.is_empty());
    }

    #[test]
    fn memory_saved_message_renders_structured_paths() {
        let style = StyleConfig::dark();
        let msg = ChatMessage::memory_saved(
            vec![
                "C:/mem/user_role.md".to_owned(),
                "C:/team/project.md".to_owned(),
            ],
            Some(1),
        );
        let rendered = message_to_lines(&msg, 120, &style)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Saved 2 memories (1 private, 1 team)"));
        assert!(rendered.contains("user_role.md"));
        assert!(rendered.contains("C:/team/project.md"));
    }

    #[test]
    fn model_info_default() {
        let info = ModelInfo::default();
        assert_eq!(info.name, "unknown");
        assert_eq!(info.provider, "unknown");
    }

    #[test]
    fn status_bar_info_default() {
        let info = StatusBarInfo::default();
        assert_eq!(info.mode_label, "INSERT");
        assert_eq!(info.token_count, 0);
    }
}
