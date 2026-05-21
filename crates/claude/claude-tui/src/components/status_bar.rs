//! Status bar component showing model, tokens, cost, and mode.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

/// Render the bottom status bar.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let style = &app.style;
    let status = &app.status;

    let mode_color = style.mode_color(&status.mode_label);

    // Left section: mode | model | tokens
    let mode_span = Span::styled(
        format!(" {} ", status.mode_label),
        Style::default()
            .fg(ratatui::style::Color::Black)
            .bg(mode_color)
            .add_modifier(Modifier::BOLD),
    );

    let separator = Span::styled(" │ ", Style::default().fg(style.info_color));

    let model_span = Span::styled(
        format!(" {} ", status.model_name),
        Style::default().fg(style.status_fg),
    );

    let token_span = if status.max_tokens > 0 {
        Span::styled(
            format!(" {}/{} tokens ", status.token_count, status.max_tokens),
            Style::default().fg(style.info_color),
        )
    } else {
        Span::styled(
            format!(" {} tokens ", status.token_count),
            Style::default().fg(style.info_color),
        )
    };

    // Right section: cost | MCP | sidebar hint
    let cost_span = Span::styled(
        format!(" ${:.4} ", status.cost),
        Style::default().fg(style.accent_color),
    );

    let mcp_count = app.mcp_servers.len();
    let connected = app
        .mcp_servers
        .iter()
        .filter(|s| s.status == "connected")
        .count();
    let mcp_span = Span::styled(
        format!(" {}/{} servers ", connected, mcp_count),
        Style::default().fg(style.info_color),
    );

    let hint_span = Span::styled(" Tab:sidebar ", Style::default().fg(style.info_color));

    // Calculate padding to push right section to the right edge.
    let left_total_width = mode_span.content.len()
        + separator.content.len()
        + model_span.content.len()
        + separator.content.len()
        + token_span.content.len();
    let right_total_width = cost_span.content.len()
        + separator.content.len()
        + mcp_span.content.len()
        + hint_span.content.len();

    let total_content = left_total_width + right_total_width;
    let padding = area.width as usize;
    let spacer_len = padding.saturating_sub(total_content);
    let spacer = Span::raw(" ".repeat(spacer_len));

    let line = Line::from(vec![
        mode_span,
        separator.clone(),
        model_span,
        separator,
        token_span,
        spacer,
        cost_span,
        Span::styled(" │ ", Style::default().fg(style.info_color)),
        mcp_span,
        hint_span,
    ]);

    let paragraph =
        Paragraph::new(line).style(Style::default().bg(style.status_bg).fg(style.status_fg));

    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::message::McpServerStatus;

    #[test]
    fn status_bar_default_info() {
        let app = App::new();
        assert_eq!(app.status.mode_label, "INSERT");
        assert_eq!(app.status.model_name, "unknown");
    }

    #[test]
    fn status_bar_with_mcp_servers() {
        let mut app = App::new();
        app.mcp_servers.push(McpServerStatus {
            name: "test".to_owned(),
            status: "connected".to_owned(),
            tool_count: 3,
        });
        assert_eq!(app.mcp_servers.len(), 1);
    }

    #[test]
    fn status_bar_cost_formatting() {
        let mut app = App::new();
        app.status.cost = 0.1234;
        let formatted = format!("{:.4}", app.status.cost);
        assert_eq!(formatted, "0.1234");
    }
}
