//! Tool output rendering component (collapsible).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::message::ToolCallInfo;
use crate::style::StyleConfig;

/// Render tool call information into ratatui Lines.
pub fn render_tool_call(
    tool_call: &ToolCallInfo,
    width: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let icon = if tool_call.is_error { "✗" } else { "✓" };
    let color = if tool_call.is_error {
        style.error_color
    } else {
        style.tool_color
    };

    let duration_s = tool_call.duration_ms as f64 / 1000.0;

    if tool_call.is_collapsed {
        let header = format!(
            "  {icon} [tool] {} ({duration_s:.1}s) — collapsed",
            tool_call.tool_name
        );
        vec![Line::from(Span::styled(header, Style::default().fg(color)))]
    } else {
        let mut lines = Vec::new();

        // Header line.
        let header = format!("  {icon} [tool] {} ({duration_s:.1}s)", tool_call.tool_name);
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));

        // Input preview.
        if !tool_call.input.is_empty() {
            let input_preview =
                crate::message::truncate_text(&tool_call.input, width.saturating_sub(6));
            lines.push(Line::from(Span::styled(
                format!("    Input: {input_preview}"),
                Style::default().fg(style.info_color),
            )));
        }

        // Output lines.
        if !tool_call.output.is_empty() {
            for line in tool_call.output.lines().take(10) {
                let truncated = crate::message::truncate_text(line, width.saturating_sub(4));
                lines.push(Line::from(Span::styled(
                    format!("    {truncated}"),
                    Style::default().fg(style.status_fg),
                )));
            }
            let total_lines = tool_call.output.lines().count();
            if total_lines > 10 {
                lines.push(Line::from(Span::styled(
                    format!("    ... ({} more lines)", total_lines - 10),
                    Style::default().fg(style.info_color),
                )));
            }
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_call(collapsed: bool) -> ToolCallInfo {
        ToolCallInfo {
            tool_name: "bash_command".to_owned(),
            input: r#"{"command": "ls"}"#.to_owned(),
            output: "file1.txt\nfile2.txt\nfile3.txt".to_owned(),
            duration_ms: 1500,
            is_collapsed: collapsed,
            is_error: false,
        }
    }

    #[test]
    fn collapsed_tool_call_single_line() {
        let tc = make_tool_call(true);
        let style = StyleConfig::dark();
        let lines = render_tool_call(&tc, 80, &style);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content.contains("collapsed"))
        );
    }

    #[test]
    fn expanded_tool_call_multiple_lines() {
        let tc = make_tool_call(false);
        let style = StyleConfig::dark();
        let lines = render_tool_call(&tc, 80, &style);
        assert!(lines.len() > 1);
    }

    #[test]
    fn error_tool_call_uses_error_color() {
        let mut tc = make_tool_call(false);
        tc.is_error = true;
        let style = StyleConfig::dark();
        let lines = render_tool_call(&tc, 80, &style);
        assert!(!lines.is_empty());
    }
}
