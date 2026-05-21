//! Input area component with Vim mode indicator.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::vim::VimMode;

/// Render the input area.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let mode = app.mode();
    let style = &app.style;

    // Mode indicator.
    let (mode_label, mode_color) = match mode {
        VimMode::Normal => ("── NORMAL ──", style.mode_normal),
        VimMode::Insert => ("── INSERT ──", style.mode_insert),
        VimMode::Command => ("── COMMAND ──", style.mode_command),
        VimMode::Visual => ("── VISUAL ──", style.mode_visual),
        VimMode::Search => ("── SEARCH ──", style.mode_search),
    };

    // Command/search buffer display.
    let display_text = match mode {
        VimMode::Command => format!(":{}", app.vim.command_buffer()),
        VimMode::Search => format!("/{}", app.vim.search_buffer()),
        _ => app.input.clone(),
    };

    let border_style = Style::default().fg(mode_color);

    let input_line = vec![
        Span::styled(
            format!(" {mode_label} "),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("> ", Style::default().fg(style.accent_color)),
        Span::raw(display_text),
    ];

    let paragraph = Paragraph::new(vec![Line::from(input_line)]).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(border_style),
    );

    f.render_widget(paragraph, area);

    // Show cursor position in insert/command/search mode.
    if matches!(mode, VimMode::Insert | VimMode::Command | VimMode::Search) {
        let prefix_len = mode_label.len() + 3; // " ── MODE ── " + "> "
        let buffer_len = match mode {
            VimMode::Command => app.vim.command_buffer().len() + 1, // +1 for ':'
            VimMode::Search => app.vim.search_buffer().len() + 1,   // +1 for '/'
            _ => app.cursor,
        };
        let cursor_x = (prefix_len + buffer_len) as u16;
        if cursor_x < area.width {
            f.set_cursor_position((area.x + cursor_x, area.y + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    #[test]
    fn input_area_renders_insert_mode() {
        let app = App::new();
        assert_eq!(app.mode(), VimMode::Insert);
    }

    #[test]
    fn input_area_with_text() {
        let mut app = App::new();
        app.input = "hello world".to_owned();
        app.cursor = 11;
        assert_eq!(app.input(), "hello world");
    }

    #[test]
    fn input_area_normal_mode() {
        let mut app = App::new();
        app.vim.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.mode(), VimMode::Normal);
    }
}
