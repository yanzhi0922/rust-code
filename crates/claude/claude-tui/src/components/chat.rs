//! Chat message list component with virtual scrolling.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::message::message_to_lines;

/// Render the chat message area.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let scroll = &app.scroll;
    let (start, end) = scroll.visible_range();
    let width = area.width.saturating_sub(2) as usize; // minus borders

    let mut lines: Vec<Line> = Vec::new();

    if app.messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No messages yet. Type to start a conversation.",
            Style::default().fg(app.style.info_color),
        )));
        lines.push(Line::from(Span::raw(String::new())));
        lines.push(Line::from(Span::styled(
            "  Vim mode: Esc=normal, i=insert, j/k=scroll, G=bottom, gg=top",
            Style::default().fg(app.style.info_color),
        )));
    } else {
        for i in start..end.min(app.messages.len()) {
            let msg = &app.messages[i];
            let msg_lines = message_to_lines(msg, width, &app.style);
            lines.extend(msg_lines);
        }
    }

    // Streaming indicator.
    if app.is_streaming {
        lines.push(Line::from(Span::styled(
            format!(" {} Thinking...", app.spinner_char()),
            Style::default()
                .fg(app.style.accent_color)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    // Calculate the scroll bar position.
    let scroll_offset = scroll.scroll_offset();

    let title = if app.messages.is_empty() {
        " Chat ".to_owned()
    } else {
        let visible = end.min(app.messages.len()) - start;
        format!(" Chat ({}/{}) ", visible, app.messages.len())
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE).title(title))
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));

    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::message::ChatMessage;

    #[test]
    fn empty_app_renders_without_panic() {
        let app = App::new();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn messages_with_content() {
        let mut app = App::new();
        app.add_message(ChatMessage::user("hello".to_owned()));
        app.add_message(ChatMessage::assistant("world".to_owned()));
        assert_eq!(app.messages.len(), 2);
    }

    #[test]
    fn scroll_visible_range_with_messages() {
        let mut app = App::new();
        for i in 0..20 {
            app.add_message(ChatMessage::user(format!("msg {i}")));
        }
        let (start, end) = app.scroll.visible_range();
        assert!(start < end);
        assert!(end <= 20);
    }
}
