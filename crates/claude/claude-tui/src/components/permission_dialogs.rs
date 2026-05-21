//! Permission dialog components for the TUI.
//!
//! Provides rendering for various permission request dialogs,
//! including bash, file, MCP, and other tool permission prompts.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`PermissionKind`] | Type of permission being requested |
//! | [`PermissionDecision`] | User's decision on a permission request |
//! | [`PermissionRequest`] | A permission request with context |
//! | [`render_permission_request`] | Render a permission request dialog |
//! | [`render_permission_explanation`] | Render why a permission is needed |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// PermissionKind
// ---------------------------------------------------------------------------

/// The type of permission being requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionKind {
    /// Bash command execution.
    Bash { command: String },
    /// File read operation.
    FileRead { path: String },
    /// File write operation.
    FileWrite { path: String },
    /// File edit operation.
    FileEdit { path: String },
    /// MCP tool call.
    McpTool { server: String, tool: String },
    /// Web fetch.
    WebFetch { url: String },
    /// Enter plan mode.
    EnterPlanMode,
    /// Exit plan mode.
    ExitPlanMode,
    /// Generic tool use.
    ToolUse { tool_name: String },
}

impl PermissionKind {
    /// Returns the display label.
    pub fn label(&self) -> &str {
        match self {
            PermissionKind::Bash { .. } => "Bash Command",
            PermissionKind::FileRead { .. } => "File Read",
            PermissionKind::FileWrite { .. } => "File Write",
            PermissionKind::FileEdit { .. } => "File Edit",
            PermissionKind::McpTool { .. } => "MCP Tool",
            PermissionKind::WebFetch { .. } => "Web Fetch",
            PermissionKind::EnterPlanMode => "Enter Plan Mode",
            PermissionKind::ExitPlanMode => "Exit Plan Mode",
            PermissionKind::ToolUse { .. } => "Tool Use",
        }
    }

    /// Returns the icon for this permission type.
    pub fn icon(&self) -> &'static str {
        match self {
            PermissionKind::Bash { .. } => "⚡",
            PermissionKind::FileRead { .. } => "📖",
            PermissionKind::FileWrite { .. } => "✏️",
            PermissionKind::FileEdit { .. } => "📝",
            PermissionKind::McpTool { .. } => "🔌",
            PermissionKind::WebFetch { .. } => "🌐",
            PermissionKind::EnterPlanMode => "📋",
            PermissionKind::ExitPlanMode => "✅",
            PermissionKind::ToolUse { .. } => "🔧",
        }
    }
}

// ---------------------------------------------------------------------------
// PermissionDecision
// ---------------------------------------------------------------------------

/// User's decision on a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this time only.
    Allow,
    /// Allow always (add rule).
    AllowAlways,
    /// Deny this time.
    Deny,
    /// Deny always (add rule).
    DenyAlways,
}

// ---------------------------------------------------------------------------
// PermissionRequest
// ---------------------------------------------------------------------------

/// A permission request with full context.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// The kind of permission being requested.
    pub kind: PermissionKind,
    /// Human-readable explanation of why the permission is needed.
    pub explanation: String,
    /// The rule source that triggered this request (if any).
    pub rule_source: Option<String>,
    /// Whether this is a YOLO/auto-mode request.
    pub is_auto_mode: bool,
    /// Number of consecutive denials (for rate limiting).
    pub denial_count: usize,
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn dim_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::DIM),
    )
}

fn warning_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render a permission request dialog.
pub fn render_permission_request(
    request: &PermissionRequest,
    _style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Header with icon and label
    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", request.kind.icon()), Style::default()),
        Span::styled(
            request.kind.label().to_owned(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " Permission Request".to_owned(),
            Style::default().fg(Color::Yellow),
        ),
    ]));

    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    // Target details based on kind
    match &request.kind {
        PermissionKind::Bash { command } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "  Command: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(command.clone(), Style::default().fg(Color::Cyan)),
            ]));
        }
        PermissionKind::FileRead { path }
        | PermissionKind::FileWrite { path }
        | PermissionKind::FileEdit { path } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "  Path: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(path.clone(), Style::default().fg(Color::Cyan)),
            ]));
        }
        PermissionKind::McpTool { server, tool } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "  Server: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(server.clone(), Style::default().fg(Color::Cyan)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    "  Tool: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(tool.clone(), Style::default().fg(Color::Cyan)),
            ]));
        }
        PermissionKind::WebFetch { url } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "  URL: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(url.clone(), Style::default().fg(Color::Cyan)),
            ]));
        }
        PermissionKind::ToolUse { tool_name } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "  Tool: ".to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(tool_name.clone(), Style::default().fg(Color::Cyan)),
            ]));
        }
        PermissionKind::EnterPlanMode | PermissionKind::ExitPlanMode => {}
    }

    // Explanation
    if !request.explanation.is_empty() {
        lines.push(Line::from(""));
        for line in request.explanation.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
    }

    // Rule source
    if let Some(source) = &request.rule_source {
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(&format!("  Rule source: {source}"))));
    }

    // Auto mode indicator
    if request.is_auto_mode {
        lines.push(Line::from(""));
        lines.push(Line::from(warning_span("  ⚡ Auto-mode active")));
    }

    // Denial warning
    if request.denial_count > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(warning_span(&format!(
            "  ⚠ {} consecutive denials",
            request.denial_count
        ))));
    }

    // Options
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  y".to_owned(), Style::default().fg(Color::Green)),
        dim_span(" allow │ "),
        Span::styled("a".to_owned(), Style::default().fg(Color::Green)),
        dim_span(" always │ "),
        Span::styled("n".to_owned(), Style::default().fg(Color::Red)),
        dim_span(" deny │ "),
        Span::styled("d".to_owned(), Style::default().fg(Color::Red)),
        dim_span(" always deny"),
    ]));

    lines
}

/// Render a permission explanation (why a permission is needed).
pub fn render_permission_explanation(
    kind: &PermissionKind,
    reason: &str,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", kind.icon()), Style::default()),
        Span::styled(
            kind.label().to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    for line in reason.lines() {
        lines.push(Line::from(format!("  {line}")));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    fn sample_request(kind: PermissionKind) -> PermissionRequest {
        PermissionRequest {
            kind,
            explanation: "This command needs to run to install dependencies.".to_owned(),
            rule_source: Some("project".to_owned()),
            is_auto_mode: false,
            denial_count: 0,
        }
    }

    // -- PermissionKind --

    #[test]
    fn kind_label_bash() {
        assert_eq!(
            PermissionKind::Bash {
                command: "ls".to_owned()
            }
            .label(),
            "Bash Command"
        );
    }

    #[test]
    fn kind_label_file_read() {
        assert_eq!(
            PermissionKind::FileRead {
                path: "/foo".to_owned()
            }
            .label(),
            "File Read"
        );
    }

    #[test]
    fn kind_label_mcp() {
        assert_eq!(
            PermissionKind::McpTool {
                server: "srv".to_owned(),
                tool: "t".to_owned()
            }
            .label(),
            "MCP Tool"
        );
    }

    #[test]
    fn kind_icon_bash() {
        assert_eq!(
            PermissionKind::Bash {
                command: "ls".to_owned()
            }
            .icon(),
            "⚡"
        );
    }

    #[test]
    fn kind_icon_file_write() {
        assert_eq!(
            PermissionKind::FileWrite {
                path: "/f".to_owned()
            }
            .icon(),
            "✏️"
        );
    }

    // -- Rendering --

    #[test]
    fn render_bash_request_shows_command() {
        let req = sample_request(PermissionKind::Bash {
            command: "npm install".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("npm install"));
        assert!(combined.contains("Bash Command"));
    }

    #[test]
    fn render_file_request_shows_path() {
        let req = sample_request(PermissionKind::FileWrite {
            path: "/etc/hosts".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("/etc/hosts"));
    }

    #[test]
    fn render_mcp_request_shows_server_and_tool() {
        let req = sample_request(PermissionKind::McpTool {
            server: "github".to_owned(),
            tool: "create_issue".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("github"));
        assert!(combined.contains("create_issue"));
    }

    #[test]
    fn render_web_request_shows_url() {
        let req = sample_request(PermissionKind::WebFetch {
            url: "https://example.com".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("https://example.com"));
    }

    #[test]
    fn render_request_shows_explanation() {
        let req = sample_request(PermissionKind::Bash {
            command: "ls".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("install dependencies"));
    }

    #[test]
    fn render_request_shows_rule_source() {
        let req = sample_request(PermissionKind::Bash {
            command: "ls".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Rule source: project"));
    }

    #[test]
    fn render_request_auto_mode() {
        let mut req = sample_request(PermissionKind::Bash {
            command: "ls".to_owned(),
        });
        req.is_auto_mode = true;
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Auto-mode"));
    }

    #[test]
    fn render_request_denial_warning() {
        let mut req = sample_request(PermissionKind::Bash {
            command: "ls".to_owned(),
        });
        req.denial_count = 3;
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("3 consecutive denials"));
    }

    #[test]
    fn render_request_shows_options() {
        let req = sample_request(PermissionKind::Bash {
            command: "ls".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("allow"));
        assert!(combined.contains("always"));
        assert!(combined.contains("deny"));
    }

    #[test]
    fn render_explanation_shows_reason() {
        let kind = PermissionKind::Bash {
            command: "rm -rf /".to_owned(),
        };
        let lines = render_permission_explanation(
            &kind,
            "This is a dangerous command that could delete files.",
            &test_style(),
        );
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("dangerous command"));
    }

    #[test]
    fn render_plan_mode_request() {
        let req = PermissionRequest {
            kind: PermissionKind::EnterPlanMode,
            explanation: "Agent wants to enter plan mode.".to_owned(),
            rule_source: None,
            is_auto_mode: false,
            denial_count: 0,
        };
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Enter Plan Mode"));
    }

    #[test]
    fn render_tool_use_request() {
        let req = sample_request(PermissionKind::ToolUse {
            tool_name: "custom_tool".to_owned(),
        });
        let lines = render_permission_request(&req, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("custom_tool"));
    }
}
