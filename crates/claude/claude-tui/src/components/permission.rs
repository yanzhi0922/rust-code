//! Permission request dialog component.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::message::PermissionRequest;

/// Render a centered permission request dialog.
pub fn render(f: &mut Frame, request: &PermissionRequest, area: Rect) {
    // Dialog dimensions.
    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = 7.min(area.height.saturating_sub(4));

    // Center the dialog.
    let x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
    let y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Clear the area behind the dialog.
    f.render_widget(Clear, dialog_area);

    let lines = vec![
        Line::from(Span::styled(
            " ⚠ Permission Request",
            Style::default()
                .fg(ratatui::style::Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw(String::new())),
        Line::from(vec![
            Span::styled(" Tool: ", Style::default().fg(ratatui::style::Color::Cyan)),
            Span::styled(
                request.tool_name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                " Action: ",
                Style::default().fg(ratatui::style::Color::Cyan),
            ),
            Span::raw(truncate_str(
                &request.description,
                (dialog_width as usize).saturating_sub(12),
            )),
        ]),
        Line::from(Span::raw(String::new())),
        if request.allow_all_available {
            Line::from(vec![
                Span::styled(
                    " [Y] Allow",
                    Style::default().fg(ratatui::style::Color::Green),
                ),
                Span::raw("  "),
                Span::styled("[N] Deny", Style::default().fg(ratatui::style::Color::Red)),
                Span::raw("  "),
                Span::styled(
                    "[A] Allow All",
                    Style::default().fg(ratatui::style::Color::Yellow),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    " [Y] Allow",
                    Style::default().fg(ratatui::style::Color::Green),
                ),
                Span::raw("  "),
                Span::styled("[N] Deny", Style::default().fg(ratatui::style::Color::Red)),
            ])
        },
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ratatui::style::Color::Yellow))
                .title(" Permission "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, dialog_area);
}

/// Truncate a string to a maximum character count.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> PermissionRequest {
        PermissionRequest {
            tool_name: "bash_command".to_owned(),
            description: "Run: ls -la".to_owned(),
            allow_all_available: true,
        }
    }

    #[test]
    fn permission_request_fields() {
        let req = make_request();
        assert_eq!(req.tool_name, "bash_command");
        assert!(req.allow_all_available);
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let result = truncate_str("abcdefghij", 5);
        assert_eq!(result, "abcde…");
    }
}
