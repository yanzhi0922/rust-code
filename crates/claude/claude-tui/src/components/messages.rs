//! Message subtype rendering components.
//!
//! Provides dedicated renderers for every message subtype that can appear in a
//! conversation, mirroring the TypeScript component architecture found in
//! `cc-haha/src/components/messages/`. Each renderer produces
//! `Vec<Line<'static>>` suitable for ratatui's [`Paragraph`] widget.
//!
//! # Message types
//!
//! | Renderer | Description |
//! |----------|-------------|
//! | [`render_thinking_message`] | Assistant thinking block (collapsible) |
//! | [`render_tool_use_message`] | Tool invocation with status indicator |
//! | [`render_tool_result_message`] | Tool result (success / failure) |
//! | [`render_compact_boundary`] | Conversation compaction marker |
//! | [`render_rate_limit_message`] | Rate-limit warning |
//! | [`render_plan_approval_message`] | Plan approval request / response |
//! | [`render_hook_progress_message`] | Hook execution progress |
//! | [`render_system_text_message`] | System-level text messages |
//! | [`render_snip_boundary`] | Snip (micro-compact) boundary |
//! | [`render_task_assignment_message`] | Sub-agent task assignment |
//! | [`render_user_bash_input`] | User bash command input |
//! | [`render_user_bash_output`] | Bash command output (stdout/stderr) |
//! | [`render_shutdown_message`] | Shutdown request / response |
//! | [`render_advisor_message`] | Advisor block rendering |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Create a dim text span.
fn dim_span(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().add_modifier(Modifier::DIM))
}

/// Create a bold text span with a given color.
fn bold_colored_span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Create a styled text span.
fn styled_span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(color))
}

/// Wrap a list of lines inside a bordered block, returning new lines with the
/// border drawn as ASCII art characters.
fn wrap_with_border(
    lines: Vec<Line<'static>>,
    border_color: Color,
    title: Option<&str>,
) -> Vec<Line<'static>> {
    let width = lines
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(0)
        .max(title.map(|t| t.len()).unwrap_or(0))
        + 4; // padding + border chars

    let mut result = Vec::new();

    // Top border
    let top = if let Some(t) = title {
        format!(
            "╭─ {} ─{}",
            t,
            "─".repeat(width.saturating_sub(t.len() + 4))
        )
    } else {
        format!("╭{}╮", "─".repeat(width.saturating_sub(2)))
    };
    result.push(Line::from(vec![styled_span(top, border_color)]));

    // Content lines
    for line in lines {
        let content_width = line.width();
        let padding = width.saturating_sub(content_width + 4);
        let mut spans = vec![styled_span("│ ", border_color)];
        for span in line.spans {
            spans.push(span);
        }
        spans.push(styled_span(
            format!("{}│", " ".repeat(padding)),
            border_color,
        ));
        result.push(Line::from(spans));
    }

    // Bottom border
    result.push(Line::from(vec![styled_span(
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        border_color,
    )]));

    result
}

// ---------------------------------------------------------------------------
// 1. Thinking Message
// ---------------------------------------------------------------------------

/// Render an assistant thinking message.
///
/// When `expanded` is false, only a single-line summary is shown
/// (`∴ Thinking …`). When `expanded` is true the full thinking text is
/// rendered in a dim style.
pub fn render_thinking_message(
    thinking: &str,
    expanded: bool,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    if thinking.is_empty() {
        return Vec::new();
    }

    if !expanded {
        // Collapsed: single-line indicator
        return vec![Line::from(vec![
            Span::styled(
                " ∴ ".to_owned(),
                Style::default()
                    .fg(style.assistant_color)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            ),
            Span::styled(
                "Thinking".to_owned(),
                Style::default()
                    .fg(style.assistant_color)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            ),
            dim_span(" (Ctrl+O to expand)"),
        ])];
    }

    // Expanded: header + full content
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(
            " ∴ ".to_owned(),
            Style::default()
                .fg(style.assistant_color)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
        Span::styled(
            "Thinking…".to_owned(),
            Style::default()
                .fg(style.assistant_color)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
    ]));

    for text_line in thinking.lines() {
        if text_line.is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                Span::styled(
                    text_line.to_owned(),
                    Style::default()
                        .fg(style.assistant_color)
                        .add_modifier(Modifier::DIM),
                ),
            ]));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// 2. Tool Use Message
// ---------------------------------------------------------------------------

/// Render a tool use message showing the tool name, input preview, and status.
///
/// `status` should be one of `"running"`, `"success"`, `"failed"`, `"pending"`.
pub fn render_tool_use_message(
    tool_name: &str,
    input_preview: &str,
    status: &str,
    duration_ms: u64,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let (icon, status_color) = match status {
        "running" => ("⟳", style.accent_color),
        "success" => ("✓", style.tool_color),
        "failed" => ("✗", style.error_color),
        _ => ("◎", style.info_color),
    };

    let mut lines = Vec::new();

    // Header line: icon + tool name
    let mut header_spans = vec![
        styled_span(format!(" {icon} "), status_color),
        bold_colored_span(tool_name, style.tool_color),
    ];

    if duration_ms > 0 {
        header_spans.push(dim_span(format!(" ({}ms)", duration_ms)));
    }

    lines.push(Line::from(header_spans));

    // Input preview (truncated to 200 chars)
    if !input_preview.is_empty() {
        let truncated: String = input_preview.chars().take(200).collect();
        for preview_line in truncated.lines() {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                dim_span(preview_line),
            ]));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// 3. Tool Result Message
// ---------------------------------------------------------------------------

/// Render a tool result message with success or failure status.
pub fn render_tool_result_message(
    content: &str,
    is_error: bool,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let (icon, color) = if is_error {
        ("✗", style.error_color)
    } else {
        ("✓", style.tool_color)
    };

    let mut lines = Vec::new();

    // Status header
    lines.push(Line::from(vec![
        styled_span(format!(" {icon} "), color),
        bold_colored_span(if is_error { "Error" } else { "Result" }, color),
    ]));

    // Content lines
    if !content.is_empty() {
        for text_line in content.lines().take(50) {
            if text_line.is_empty() {
                lines.push(Line::from(""));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        text_line.to_owned(),
                        Style::default().fg(if is_error {
                            style.error_color
                        } else {
                            style.tool_color
                        }),
                    ),
                ]));
            }
        }

        let total_lines = content.lines().count();
        if total_lines > 50 {
            lines.push(Line::from(vec![dim_span(format!(
                "   … ({} more lines)",
                total_lines - 50
            ))]));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// 4. Compact Boundary
// ---------------------------------------------------------------------------

/// Render a conversation compaction boundary marker.
///
/// Shown when the conversation history has been compacted to save context
/// window space. Displays a dim separator with a hint about viewing history.
pub fn render_compact_boundary(style: &StyleConfig) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(vec![styled_span(
            " ✻ Conversation compacted (Ctrl+O for history)",
            style.info_color,
        )]),
        Line::from(""),
    ]
}

// ---------------------------------------------------------------------------
// 5. Rate Limit Message
// ---------------------------------------------------------------------------

/// Render a rate-limit warning message.
///
/// Displays the rate limit text along with an optional upsell suggestion.
pub fn render_rate_limit_message(
    text: &str,
    upsell_hint: Option<&str>,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        styled_span(" ⚠ ", style.error_color),
        bold_colored_span("Rate limit reached", style.error_color),
    ]));

    if !text.is_empty() {
        for text_line in text.lines() {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                styled_span(text_line, style.info_color),
            ]));
        }
    }

    if let Some(hint) = upsell_hint {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            dim_span("   💡 "),
            styled_span(hint, style.accent_color),
        ]));
    }

    lines
}

// ---------------------------------------------------------------------------
// 6. Plan Approval Message
// ---------------------------------------------------------------------------

/// The type of plan approval message to render.
#[derive(Debug, Clone)]
pub enum PlanApprovalKind<'a> {
    /// A request for plan approval.
    Request {
        /// Who is requesting approval.
        from: &'a str,
        /// The plan content.
        plan_content: &'a str,
        /// Optional file path of the plan.
        plan_file_path: Option<&'a str>,
    },
    /// A response to a plan approval request.
    Response {
        /// Who responded.
        from: &'a str,
        /// Whether the plan was approved.
        approved: bool,
        /// Optional reason for rejection.
        reason: Option<&'a str>,
    },
}

/// Render a plan approval request or response.
pub fn render_plan_approval_message(
    kind: &PlanApprovalKind<'_>,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    match kind {
        PlanApprovalKind::Request {
            from,
            plan_content,
            plan_file_path,
        } => {
            let mut inner = Vec::new();

            inner.push(Line::from(vec![bold_colored_span(
                format!("Plan Approval Request from {from}"),
                style.accent_color,
            )]));

            if !plan_content.is_empty() {
                inner.push(Line::from(""));
                for text_line in plan_content.lines() {
                    inner.push(Line::from(styled_span(text_line, style.assistant_color)));
                }
            }

            if let Some(path) = plan_file_path {
                inner.push(Line::from(""));
                inner.push(Line::from(vec![
                    dim_span("Plan file: "),
                    styled_span(*path, style.info_color),
                ]));
            }

            wrap_with_border(inner, style.accent_color, None)
        }
        PlanApprovalKind::Response {
            from,
            approved,
            reason,
        } => {
            let (icon, label, border_color) = if *approved {
                ("✓", "Approved", style.tool_color)
            } else {
                ("✗", "Rejected", style.error_color)
            };

            let mut inner = Vec::new();
            inner.push(Line::from(vec![
                styled_span(format!("{icon} "), border_color),
                bold_colored_span(format!("Plan {label} by {from}"), border_color),
            ]));

            if let Some(r) = reason {
                inner.push(Line::from(""));
                inner.push(Line::from(vec![
                    dim_span("Reason: "),
                    styled_span(*r, style.info_color),
                ]));
            }

            wrap_with_border(inner, border_color, None)
        }
    }
}

// ---------------------------------------------------------------------------
// 7. Hook Progress Message
// ---------------------------------------------------------------------------

/// Render a hook progress message.
///
/// Shows the hook event type and the number of hooks that have run.
pub fn render_hook_progress_message(
    hook_event: &str,
    in_progress_count: usize,
    resolved_count: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    if in_progress_count == 0 {
        return Vec::new();
    }

    let plural = if in_progress_count == 1 {
        "hook"
    } else {
        "hooks"
    };

    let status = if resolved_count == in_progress_count {
        "ran"
    } else {
        "running"
    };

    vec![Line::from(vec![
        dim_span(format!(" {in_progress_count} ")),
        bold_colored_span(hook_event, style.info_color),
        dim_span(format!(
            " {plural} {status} ({resolved_count}/{in_progress_count})"
        )),
    ])]
}

// ---------------------------------------------------------------------------
// 8. System Text Message
// ---------------------------------------------------------------------------

/// Render a system text message.
///
/// System messages are displayed with a distinctive style to differentiate
/// them from user and assistant messages.
pub fn render_system_text_message(content: &str, style: &StyleConfig) -> Vec<Line<'static>> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        styled_span(" ◈ ", style.system_color),
        bold_colored_span("system", style.system_color),
    ]));

    for text_line in content.lines() {
        if text_line.is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                styled_span(text_line, style.system_color),
            ]));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// 9. Snip Boundary
// ---------------------------------------------------------------------------

/// Render a snip boundary marker.
///
/// Snip boundaries mark where micro-compaction has removed content from the
/// conversation context.
pub fn render_snip_boundary(_style: &StyleConfig) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(vec![dim_span(" ✂ ── content snipped ──")]),
        Line::from(""),
    ]
}

// ---------------------------------------------------------------------------
// 10. Task Assignment Message
// ---------------------------------------------------------------------------

/// Render a task assignment message for sub-agent coordination.
pub fn render_task_assignment_message(
    task_id: u64,
    assigned_by: &str,
    subject: &str,
    description: Option<&str>,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut inner = Vec::new();

    inner.push(Line::from(vec![bold_colored_span(
        format!("Task #{task_id} assigned by {assigned_by}"),
        Color::Cyan,
    )]));

    inner.push(Line::from(vec![bold_colored_span(
        subject,
        style.status_fg,
    )]));

    if let Some(desc) = description
        && !desc.is_empty()
    {
        inner.push(Line::from(""));
        inner.push(Line::from(dim_span(desc)));
    }

    wrap_with_border(inner, Color::Cyan, None)
}

// ---------------------------------------------------------------------------
// 11. User Bash Input
// ---------------------------------------------------------------------------

/// Render user bash input with a distinctive prompt indicator.
pub fn render_user_bash_input(input: &str, style: &StyleConfig) -> Vec<Line<'static>> {
    if input.is_empty() {
        return Vec::new();
    }

    vec![Line::from(vec![
        styled_span(" ! ", style.tool_color),
        styled_span(input, style.status_fg),
    ])]
}

// ---------------------------------------------------------------------------
// 12. User Bash Output
// ---------------------------------------------------------------------------

/// Render bash command output (stdout and stderr).
pub fn render_user_bash_output(
    stdout: &str,
    stderr: &str,
    verbose: bool,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if !stdout.is_empty() {
        if verbose {
            for text_line in stdout.lines() {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    styled_span(text_line, style.tool_color),
                ]));
            }
        } else {
            let stdout_lines: Vec<&str> = stdout.lines().collect();
            let display_count = stdout_lines.len().min(5);
            for text_line in &stdout_lines[..display_count] {
                lines.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    styled_span(*text_line, style.tool_color),
                ]));
            }
            if stdout_lines.len() > 5 {
                lines.push(Line::from(dim_span(format!(
                    "   … ({} more lines)",
                    stdout_lines.len() - 5
                ))));
            }
        }
    }

    if !stderr.is_empty() {
        lines.push(Line::from(dim_span("   stderr:")));
        for text_line in stderr.lines().take(10) {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default()),
                styled_span(text_line, style.error_color),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(dim_span("   (no output)")));
    }

    lines
}

// ---------------------------------------------------------------------------
// 13. Shutdown Message
// ---------------------------------------------------------------------------

/// The type of shutdown message.
#[derive(Debug, Clone)]
pub enum ShutdownKind<'a> {
    /// A shutdown request.
    Request {
        /// Who is requesting the shutdown.
        from: &'a str,
        /// The reason for the shutdown.
        reason: Option<&'a str>,
    },
    /// A shutdown rejection.
    Rejected {
        /// Who rejected the shutdown.
        from: &'a str,
        /// The reason for rejection.
        reason: Option<&'a str>,
    },
    /// A shutdown approval.
    Approved {
        /// Who approved the shutdown.
        from: &'a str,
    },
}

/// Render a shutdown message.
pub fn render_shutdown_message(kind: &ShutdownKind<'_>, style: &StyleConfig) -> Vec<Line<'static>> {
    match kind {
        ShutdownKind::Request { from, reason } => {
            let mut inner = Vec::new();
            inner.push(Line::from(vec![bold_colored_span(
                format!("Shutdown request from {from}"),
                style.tool_color,
            )]));

            if let Some(r) = reason {
                inner.push(Line::from(vec![
                    dim_span("Reason: "),
                    styled_span(*r, style.info_color),
                ]));
            }

            wrap_with_border(inner, style.tool_color, None)
        }
        ShutdownKind::Rejected { from, reason } => {
            let mut inner = Vec::new();
            inner.push(Line::from(vec![bold_colored_span(
                format!("Shutdown rejected by {from}"),
                style.info_color,
            )]));

            if let Some(r) = reason {
                inner.push(Line::from(""));
                inner.push(Line::from(vec![
                    dim_span("Reason: "),
                    styled_span(*r, style.info_color),
                ]));
            }

            inner.push(Line::from(""));
            inner.push(Line::from(dim_span(
                "Teammate is continuing to work. You may request shutdown again later.",
            )));

            wrap_with_border(inner, style.info_color, None)
        }
        ShutdownKind::Approved { from } => {
            let mut inner = Vec::new();
            inner.push(Line::from(vec![
                styled_span("✓ ", style.tool_color),
                bold_colored_span(format!("Shutdown approved by {from}"), style.tool_color),
            ]));

            wrap_with_border(inner, style.tool_color, None)
        }
    }
}

// ---------------------------------------------------------------------------
// 14. Advisor Message
// ---------------------------------------------------------------------------

/// The type of advisor block.
#[derive(Debug, Clone)]
pub enum AdvisorBlockKind<'a> {
    /// Advisor is performing a tool use.
    ToolUse {
        /// Block identifier.
        id: &'a str,
        /// Tool name.
        tool_name: &'a str,
        /// Input parameters (JSON).
        input: Option<&'a str>,
        /// Whether the tool call is resolved.
        is_resolved: bool,
        /// Whether the tool call errored.
        is_error: bool,
    },
    /// Advisor is producing text output.
    Text {
        /// The text content.
        content: &'a str,
    },
}

/// Render an advisor message block.
pub fn render_advisor_message(
    block: &AdvisorBlockKind<'_>,
    advisor_model: Option<&str>,
    verbose: bool,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    match block {
        AdvisorBlockKind::ToolUse {
            tool_name,
            input,
            is_resolved,
            is_error,
            ..
        } => {
            let (icon, status_color) = if *is_error {
                ("✗", style.error_color)
            } else if *is_resolved {
                ("✓", style.tool_color)
            } else {
                ("⟳", style.accent_color)
            };

            let mut lines = Vec::new();

            let mut header = vec![
                styled_span(format!(" {icon} "), status_color),
                bold_colored_span("Advising", style.assistant_color),
            ];

            if let Some(model) = advisor_model {
                header.push(dim_span(format!(" using {model}")));
            }

            lines.push(Line::from(header));

            if verbose {
                lines.push(Line::from(vec![
                    dim_span("   Tool: "),
                    styled_span(*tool_name, style.tool_color),
                ]));

                if let Some(inp) = input
                    && !inp.is_empty()
                {
                    let truncated: String = inp.chars().take(200).collect();
                    for text_line in truncated.lines() {
                        lines.push(Line::from(vec![dim_span("   "), dim_span(text_line)]));
                    }
                }
            }

            lines
        }
        AdvisorBlockKind::Text { content } => {
            if content.is_empty() {
                return Vec::new();
            }

            let mut lines = Vec::new();

            let mut header = vec![bold_colored_span("Advisor", style.assistant_color)];

            if let Some(model) = advisor_model {
                header.push(dim_span(format!(" ({model})")));
            }

            lines.push(Line::from(header));

            for text_line in content.lines() {
                if text_line.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("   ", Style::default()),
                        styled_span(text_line, style.assistant_color),
                    ]));
                }
            }

            lines
        }
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

    // --- Thinking message tests ---

    #[test]
    fn thinking_collapsed_returns_single_line() {
        let lines = render_thinking_message("I need to think about this", false, &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("Thinking"));
    }

    #[test]
    fn thinking_expanded_shows_content() {
        let lines = render_thinking_message("step 1\nstep 2", true, &test_style());
        assert!(lines.len() >= 3); // header + 2 content lines
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("step 1"));
        assert!(combined.contains("step 2"));
    }

    #[test]
    fn thinking_empty_returns_nothing() {
        let lines = render_thinking_message("", false, &test_style());
        assert!(lines.is_empty());
    }

    // --- Tool use message tests ---

    #[test]
    fn tool_use_running_shows_spinner() {
        let lines = render_tool_use_message("Bash", "ls -la", "running", 0, &test_style());
        assert!(!lines.is_empty());
        let text = lines[0].to_string();
        assert!(text.contains("⟳"));
        assert!(text.contains("Bash"));
    }

    #[test]
    fn tool_use_success_shows_check() {
        let lines = render_tool_use_message("Read", "/foo.rs", "success", 150, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("✓"));
        assert!(text.contains("150ms"));
    }

    #[test]
    fn tool_use_failed_shows_cross() {
        let lines = render_tool_use_message("Write", "/err", "failed", 0, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("✗"));
    }

    #[test]
    fn tool_use_with_empty_preview() {
        let lines = render_tool_use_message("Tool", "", "running", 0, &test_style());
        assert_eq!(lines.len(), 1); // only header
    }

    // --- Tool result message tests ---

    #[test]
    fn tool_result_success() {
        let lines = render_tool_result_message("file contents", false, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("✓"));
        assert!(text.contains("Result"));
    }

    #[test]
    fn tool_result_error() {
        let lines = render_tool_result_message("permission denied", true, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("✗"));
        assert!(text.contains("Error"));
    }

    #[test]
    fn tool_result_truncates_long_content() {
        let long = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_tool_result_message(&long, false, &test_style());
        // Should have header + 50 lines + truncation notice
        let last = lines
            .last()
            .expect("truncated output should have lines")
            .to_string();
        assert!(last.contains("more lines"));
    }

    #[test]
    fn tool_result_empty_content() {
        let lines = render_tool_result_message("", false, &test_style());
        assert_eq!(lines.len(), 1); // only header
    }

    // --- Compact boundary tests ---

    #[test]
    fn compact_boundary_has_marker() {
        let lines = render_compact_boundary(&test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("compacted"));
    }

    #[test]
    fn compact_boundary_has_three_lines() {
        let lines = render_compact_boundary(&test_style());
        assert_eq!(lines.len(), 3); // empty + content + empty
    }

    // --- Rate limit message tests ---

    #[test]
    fn rate_limit_basic() {
        let lines = render_rate_limit_message("Slow down", None, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("Rate limit"));
    }

    #[test]
    fn rate_limit_with_upsell() {
        let lines = render_rate_limit_message("Too many requests", Some("/upgrade"), &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("/upgrade"));
    }

    // --- Plan approval tests ---

    #[test]
    fn plan_approval_request() {
        let kind = PlanApprovalKind::Request {
            from: "agent-1",
            plan_content: "Do the thing",
            plan_file_path: Some("/tmp/plan.md"),
        };
        let lines = render_plan_approval_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("agent-1"));
        assert!(combined.contains("Do the thing"));
        assert!(combined.contains("/tmp/plan.md"));
    }

    #[test]
    fn plan_approval_approved() {
        let kind = PlanApprovalKind::Response {
            from: "user",
            approved: true,
            reason: None,
        };
        let lines = render_plan_approval_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("✓"));
        assert!(combined.contains("Approved"));
    }

    #[test]
    fn plan_approval_rejected_with_reason() {
        let kind = PlanApprovalKind::Response {
            from: "user",
            approved: false,
            reason: Some("Not safe"),
        };
        let lines = render_plan_approval_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("✗"));
        assert!(combined.contains("Rejected"));
        assert!(combined.contains("Not safe"));
    }

    // --- Hook progress tests ---

    #[test]
    fn hook_progress_zero_count_returns_empty() {
        let lines = render_hook_progress_message("PreToolUse", 0, 0, &test_style());
        assert!(lines.is_empty());
    }

    #[test]
    fn hook_progress_shows_count() {
        let lines = render_hook_progress_message("PreToolUse", 3, 2, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("3"));
        assert!(text.contains("PreToolUse"));
    }

    #[test]
    fn hook_progress_singular() {
        let lines = render_hook_progress_message("PostToolUse", 1, 0, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("hook")); // singular
        assert!(!text.contains("hooks"));
    }

    // --- System text message tests ---

    #[test]
    fn system_text_basic() {
        let lines = render_system_text_message("Hello world", &test_style());
        assert!(lines.len() >= 2);
        let text = lines[0].to_string();
        assert!(text.contains("system"));
    }

    #[test]
    fn system_text_empty_returns_empty() {
        let lines = render_system_text_message("", &test_style());
        assert!(lines.is_empty());
    }

    #[test]
    fn system_text_multiline() {
        let lines = render_system_text_message("line1\nline2\nline3", &test_style());
        assert!(lines.len() >= 4); // header + 3 content lines
    }

    // --- Snip boundary tests ---

    #[test]
    fn snip_boundary_has_marker() {
        let lines = render_snip_boundary(&test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("snipped"));
    }

    // --- Task assignment tests ---

    #[test]
    fn task_assignment_basic() {
        let lines =
            render_task_assignment_message(42, "coordinator", "Fix the bug", None, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("#42"));
        assert!(combined.contains("coordinator"));
        assert!(combined.contains("Fix the bug"));
    }

    #[test]
    fn task_assignment_with_description() {
        let lines = render_task_assignment_message(
            1,
            "lead",
            "Refactor module",
            Some("See the attached spec for details"),
            &test_style(),
        );
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("See the attached spec"));
    }

    // --- User bash input tests ---

    #[test]
    fn bash_input_basic() {
        let lines = render_user_bash_input("ls -la", &test_style());
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("ls -la"));
    }

    #[test]
    fn bash_input_empty_returns_empty() {
        let lines = render_user_bash_input("", &test_style());
        assert!(lines.is_empty());
    }

    // --- User bash output tests ---

    #[test]
    fn bash_output_stdout_only() {
        let lines = render_user_bash_output("hello", "", false, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("hello"));
    }

    #[test]
    fn bash_output_stderr() {
        let lines = render_user_bash_output("", "error occurred", false, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("error occurred"));
    }

    #[test]
    fn bash_output_empty_shows_no_output() {
        let lines = render_user_bash_output("", "", false, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("no output"));
    }

    #[test]
    fn bash_output_verbose_shows_all() {
        let long_output: String = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_user_bash_output(&long_output, "", true, &test_style());
        // Verbose mode should show all lines
        assert!(lines.len() >= 20);
    }

    #[test]
    fn bash_output_non_verbose_truncates() {
        let long_output: String = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_user_bash_output(&long_output, "", false, &test_style());
        // Non-verbose should show 5 + truncation notice
        let last = lines
            .last()
            .expect("bash output should have lines")
            .to_string();
        assert!(last.contains("more lines"));
    }

    // --- Shutdown message tests ---

    #[test]
    fn shutdown_request() {
        let kind = ShutdownKind::Request {
            from: "agent-2",
            reason: Some("Task complete"),
        };
        let lines = render_shutdown_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("agent-2"));
        assert!(combined.contains("Task complete"));
    }

    #[test]
    fn shutdown_rejected() {
        let kind = ShutdownKind::Rejected {
            from: "user",
            reason: Some("Still working"),
        };
        let lines = render_shutdown_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("rejected"));
        assert!(combined.contains("continuing to work"));
    }

    #[test]
    fn shutdown_approved() {
        let kind = ShutdownKind::Approved { from: "user" };
        let lines = render_shutdown_message(&kind, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("approved"));
    }

    // --- Advisor message tests ---

    #[test]
    fn advisor_tool_use_running() {
        let block = AdvisorBlockKind::ToolUse {
            id: "123",
            tool_name: "Read",
            input: Some("{\"path\":\"/foo\"}"),
            is_resolved: false,
            is_error: false,
        };
        let lines = render_advisor_message(&block, None, true, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("Advising"));
        assert!(text.contains("⟳"));
    }

    #[test]
    fn advisor_tool_use_with_model() {
        let block = AdvisorBlockKind::ToolUse {
            id: "456",
            tool_name: "Bash",
            input: None,
            is_resolved: true,
            is_error: false,
        };
        let lines = render_advisor_message(&block, Some("gpt-4"), false, &test_style());
        let text = lines[0].to_string();
        assert!(text.contains("gpt-4"));
    }

    #[test]
    fn advisor_text_block() {
        let block = AdvisorBlockKind::Text {
            content: "Here is my advice",
        };
        let lines = render_advisor_message(&block, None, false, &test_style());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("Advisor"));
        assert!(combined.contains("Here is my advice"));
    }

    #[test]
    fn advisor_text_empty_returns_empty() {
        let block = AdvisorBlockKind::Text { content: "" };
        let lines = render_advisor_message(&block, None, false, &test_style());
        assert!(lines.is_empty());
    }

    // --- Border wrapping tests ---

    #[test]
    fn border_wrap_basic() {
        let inner = vec![Line::from("Hello"), Line::from("World")];
        let result = wrap_with_border(inner, Color::Blue, None);
        assert!(result.len() >= 4); // top + 2 content + bottom
        let first = result[0].to_string();
        assert!(first.starts_with('╭'));
    }

    #[test]
    fn border_wrap_with_title() {
        let inner = vec![Line::from("content")];
        let result = wrap_with_border(inner, Color::Green, Some("Title"));
        let first = result[0].to_string();
        assert!(first.contains("Title"));
    }
}
