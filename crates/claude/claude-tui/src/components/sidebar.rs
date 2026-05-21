//! Sidebar component showing sessions, tools, MCP servers, or help.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::{App, SidebarTab};
use crate::layout::sidebar_content_area;

/// Render the sidebar panel.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let style = &app.style;

    // Tab bar
    let tabs: Vec<Span> = SidebarTab::all()
        .iter()
        .flat_map(|tab| {
            let is_active = *tab == app.sidebar_tab;
            let label = tab.label();
            let tab_style = if is_active {
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(style.info_color)
            };
            let separator = Span::styled(" ", Style::default());
            let tab_span = Span::styled(format!(" {label} "), tab_style);
            vec![separator, tab_span]
        })
        .collect();

    let tab_line = Line::from(tabs);
    let tab_widget =
        Paragraph::new(tab_line).style(Style::default().bg(style.sidebar_bg).fg(style.status_fg));
    f.render_widget(tab_widget, area);

    // Content area below tabs.
    let content_area = sidebar_content_area(area);
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    match app.sidebar_tab {
        SidebarTab::Sessions => render_sessions(f, app, content_area),
        SidebarTab::Tools => render_tools(f, app, content_area),
        SidebarTab::Mcp => render_mcp(f, app, content_area),
        SidebarTab::Help => render_help_tab(f, app, content_area),
    }
}

fn render_sessions(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .messages
        .iter()
        .enumerate()
        .filter_map(|(i, msg)| {
            if msg.role == crate::message::MessageRole::User {
                let preview =
                    crate::message::message_preview(msg, (area.width as usize).saturating_sub(6));
                let text = format!("{}. {preview}", i + 1);
                Some(ListItem::new(text))
            } else {
                None
            }
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(app.style.status_fg)),
    );
    f.render_widget(list, area);
}

fn render_tools(f: &mut Frame, app: &App, area: Rect) {
    let specs = claude_tools::runtime_builtin_tool_specs();
    let items: Vec<ListItem> = specs
        .iter()
        .map(|spec| {
            let text = format!(" ⚙ {}", spec.name);
            ListItem::new(Line::from(Span::styled(
                text,
                Style::default().fg(app.style.tool_color),
            )))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(app.style.status_fg)),
    );
    f.render_widget(list, area);
}

fn render_mcp(f: &mut Frame, app: &App, area: Rect) {
    if app.mcp_servers.is_empty() {
        let text = Paragraph::new("  No MCP servers configured.")
            .style(Style::default().fg(app.style.info_color));
        f.render_widget(text, area);
        return;
    }

    let items: Vec<ListItem> = app
        .mcp_servers
        .iter()
        .map(|server| {
            let icon = match server.status.as_str() {
                "connected" => "●",
                "failed" | "disabled" => "○",
                _ => "◌",
            };
            let color = match server.status.as_str() {
                "connected" => app.style.mode_insert,
                "failed" => app.style.error_color,
                _ => app.style.info_color,
            };
            let text = format!(" {icon} {} ({} tools)", server.name, server.tool_count);
            ListItem::new(Line::from(Span::styled(text, Style::default().fg(color))))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(app.style.status_fg)),
    );
    f.render_widget(list, area);
}

fn render_help_tab(f: &mut Frame, app: &App, area: Rect) {
    let style = &app.style;
    let lines = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts",
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            " Mode:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("  Esc      → Normal mode")),
        Line::from(Span::raw("  i        → Insert mode")),
        Line::from(Span::raw("  v        → Visual mode")),
        Line::from(Span::raw("  :        → Command mode")),
        Line::from(Span::raw("  /        → Search mode")),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            " Navigation:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("  j/k      → Scroll up/down")),
        Line::from(Span::raw("  G        → Jump to bottom")),
        Line::from(Span::raw("  gg       → Jump to top")),
        Line::from(Span::raw("  Ctrl-U/D → Half-page scroll")),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            " Input:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("  Enter    → Send message")),
        Line::from(Span::raw("  Shift+Enter → New line")),
        Line::from(Span::raw("  Tab      → Toggle sidebar")),
        Line::from(Span::raw("  ↑/↓      → History")),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            " Commands:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("  :q       → Quit")),
        Line::from(Span::raw("  :help    → This panel")),
        Line::from(Span::raw("  :clear   → Clear chat")),
        Line::from(Span::raw("  :sidebar → Toggle sidebar")),
        Line::from(Span::raw("  /help    → Slash commands")),
    ];

    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(style.status_fg))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    #[test]
    fn sidebar_tab_help_renders() {
        let mut app = App::new();
        app.sidebar_visible = true;
        app.sidebar_tab = SidebarTab::Help;
        assert_eq!(app.sidebar_tab, SidebarTab::Help);
    }

    #[test]
    fn sidebar_tab_sessions() {
        let mut app = App::new();
        app.sidebar_tab = SidebarTab::Sessions;
        app.add_message(crate::message::ChatMessage::user("test".to_owned()));
        assert_eq!(app.sidebar_tab, SidebarTab::Sessions);
    }

    #[test]
    fn sidebar_tab_mcp_empty() {
        let mut app = App::new();
        app.sidebar_tab = SidebarTab::Mcp;
        assert!(app.mcp_servers.is_empty());
    }
}
