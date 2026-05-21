//! Layout calculation for the TUI.
//!
//! Computes the position and size of each UI region based on terminal
//! dimensions and application state (sidebar visibility, etc.).

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::{ActivePanel, App, SidebarTab};

/// Computed layout regions for a single render pass.
#[derive(Debug, Clone)]
pub struct AppLayout {
    /// Main content area (chat or chat + sidebar).
    pub main: Rect,
    /// Chat message area.
    pub chat: Rect,
    /// Sidebar area (zero-width if hidden).
    pub sidebar: Rect,
    /// Input area.
    pub input: Rect,
    /// Status bar area.
    pub status_bar: Rect,
}

impl AppLayout {
    /// Compute the layout from the terminal area and app state.
    pub fn compute(area: Rect, app: &App) -> Self {
        // Vertical split: main | input (2 rows) | status bar (1 row)
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // main area
                Constraint::Length(2), // input area
                Constraint::Length(1), // status bar
            ])
            .split(area);

        let main = vertical[0];
        let input = vertical[1];
        let status_bar = vertical[2];

        // Horizontal split within main: chat | sidebar
        let (chat, sidebar) = if app.sidebar_visible {
            let horizontal = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(1),     // chat
                    Constraint::Length(30), // sidebar
                ])
                .split(main);
            (horizontal[0], horizontal[1])
        } else {
            (main, Rect::new(0, 0, 0, 0))
        };

        AppLayout {
            main,
            chat,
            sidebar,
            input,
            status_bar,
        }
    }
}

/// Calculate the input area inner dimensions (accounting for borders).
pub fn input_inner(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_sub(0),
        area.y,
        area.width.saturating_sub(2),
        area.height,
    )
}

/// Calculate the sidebar tab height.
pub fn sidebar_tab_height() -> u16 {
    1
}

/// Calculate the sidebar content area (below tabs).
pub fn sidebar_content_area(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    )
}

/// Determine if the given panel is focused.
pub fn is_panel_focused(app: &App, panel: ActivePanel) -> bool {
    app.active_panel == panel
}

/// Get the sidebar tab index.
pub fn sidebar_tab_index(tab: SidebarTab) -> usize {
    match tab {
        SidebarTab::Sessions => 0,
        SidebarTab::Tools => 1,
        SidebarTab::Mcp => 2,
        SidebarTab::Help => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app() -> App {
        App::new()
    }

    #[test]
    fn layout_without_sidebar() {
        let app = make_app();
        let area = Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, &app);
        assert!(!app.sidebar_visible);
        assert_eq!(layout.chat.width, 80);
        assert_eq!(layout.sidebar.width, 0);
        assert_eq!(layout.status_bar.height, 1);
        assert_eq!(layout.input.height, 2);
    }

    #[test]
    fn layout_with_sidebar() {
        let mut app = make_app();
        app.sidebar_visible = true;
        let area = Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, &app);
        assert_eq!(layout.chat.width, 50); // 80 - 30
        assert_eq!(layout.sidebar.width, 30);
    }

    #[test]
    fn layout_narrow_terminal() {
        let app = make_app();
        let area = Rect::new(0, 0, 20, 10);
        let layout = AppLayout::compute(area, &app);
        assert!(layout.chat.width > 0);
        assert!(layout.chat.height > 0);
    }

    #[test]
    fn input_inner_deducts_border() {
        let area = Rect::new(0, 20, 80, 2);
        let inner = input_inner(area);
        assert_eq!(inner.width, 78);
    }

    #[test]
    fn sidebar_content_area_below_tabs() {
        let area = Rect::new(50, 0, 30, 20);
        let content = sidebar_content_area(area);
        assert_eq!(content.y, 1);
        assert_eq!(content.height, 19);
    }

    #[test]
    fn sidebar_tab_index_values() {
        assert_eq!(sidebar_tab_index(SidebarTab::Sessions), 0);
        assert_eq!(sidebar_tab_index(SidebarTab::Tools), 1);
        assert_eq!(sidebar_tab_index(SidebarTab::Mcp), 2);
        assert_eq!(sidebar_tab_index(SidebarTab::Help), 3);
    }

    #[test]
    fn is_panel_focused_check() {
        let mut app = make_app();
        app.active_panel = ActivePanel::Input;
        assert!(is_panel_focused(&app, ActivePanel::Input));
        assert!(!is_panel_focused(&app, ActivePanel::Chat));
    }
}
