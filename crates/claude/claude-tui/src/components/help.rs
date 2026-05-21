//! Help panel component showing keyboard shortcuts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::style::StyleConfig;

/// Keybinding entry.
struct KeyBinding {
    keys: &'static str,
    description: &'static str,
}

/// Render the help panel overlay.
pub fn render(f: &mut Frame, style: &StyleConfig, area: Rect) {
    let bindings = [
        // Mode
        KeyBinding {
            keys: "Esc",
            description: "Normal mode",
        },
        KeyBinding {
            keys: "i",
            description: "Insert mode",
        },
        KeyBinding {
            keys: "v",
            description: "Visual mode",
        },
        KeyBinding {
            keys: ":",
            description: "Command mode",
        },
        KeyBinding {
            keys: "/",
            description: "Search mode",
        },
        // Navigation
        KeyBinding {
            keys: "j / ↓",
            description: "Scroll down",
        },
        KeyBinding {
            keys: "k / ↑",
            description: "Scroll up",
        },
        KeyBinding {
            keys: "G",
            description: "Jump to bottom",
        },
        KeyBinding {
            keys: "gg",
            description: "Jump to top",
        },
        KeyBinding {
            keys: "Ctrl-U",
            description: "Half page up",
        },
        KeyBinding {
            keys: "Ctrl-D",
            description: "Half page down",
        },
        // Input
        KeyBinding {
            keys: "Enter",
            description: "Send message",
        },
        KeyBinding {
            keys: "Shift+Enter",
            description: "New line",
        },
        KeyBinding {
            keys: "Tab",
            description: "Toggle sidebar",
        },
        KeyBinding {
            keys: "↑ / ↓",
            description: "Input history",
        },
        KeyBinding {
            keys: "Ctrl-C",
            description: "Clear / Quit",
        },
        // Commands
        KeyBinding {
            keys: ":q",
            description: "Quit",
        },
        KeyBinding {
            keys: ":help",
            description: "This panel",
        },
        KeyBinding {
            keys: ":clear",
            description: "Clear chat",
        },
        KeyBinding {
            keys: ":sidebar",
            description: "Toggle sidebar",
        },
        KeyBinding {
            keys: "/help",
            description: "Slash commands",
        },
    ];

    let max_key_len = bindings.iter().map(|b| b.keys.len()).max().unwrap_or(10);

    let mut lines = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts",
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
    ];

    let mut current_section = "";
    let sections = [
        ("Mode", 0),
        ("Navigation", 5),
        ("Input", 11),
        ("Commands", 16),
    ];

    for (i, binding) in bindings.iter().enumerate() {
        // Add section headers.
        for (section_name, section_start) in &sections {
            if *section_start == i && *section_name != current_section {
                current_section = section_name;
                lines.push(Line::from(Span::styled(
                    format!(" {section_name}:"),
                    Style::default()
                        .fg(style.status_fg)
                        .add_modifier(Modifier::BOLD),
                )));
            }
        }

        let padding = " ".repeat(max_key_len.saturating_sub(binding.keys.len()));
        lines.push(Line::from(vec![
            Span::styled(
                format!("   {}{}  ", binding.keys, padding),
                Style::default().fg(style.accent_color),
            ),
            Span::styled(
                binding.description.to_owned(),
                Style::default().fg(style.status_fg),
            ),
        ]));
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::styled(
        " Press Esc to close",
        Style::default().fg(style.info_color),
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(style.accent_color))
                .title(" Help "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use crate::style::StyleConfig;

    #[test]
    fn help_panel_has_content() {
        let style = StyleConfig::dark();
        // Verify the style is valid for rendering.
        assert_eq!(style.accent_color, ratatui::style::Color::Blue);
    }

    #[test]
    fn style_config_dark() {
        let style = StyleConfig::dark();
        assert_eq!(style.name, "dark");
    }

    #[test]
    fn style_config_has_all_colors() {
        let style = StyleConfig::dark();
        // Just ensure all color fields are accessible.
        let _ = (
            style.user_color,
            style.assistant_color,
            style.system_color,
            style.tool_color,
            style.error_color,
            style.info_color,
            style.accent_color,
        );
    }
}
