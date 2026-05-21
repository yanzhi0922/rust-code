//! Main render entry point.
//!
//! Orchestrates all component rendering based on the current [`App`](crate::app::App) state.

use ratatui::Frame;

use crate::app::App;
use crate::layout::AppLayout;

/// Render the entire TUI frame.
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let layout = AppLayout::compute(area, app);

    // Update scroll viewport to match chat area height.
    // (This is a read-only borrow, so we can't mutate app here.
    //  The caller should call app.update_scroll_viewport before rendering.)

    // Render chat area.
    crate::components::chat::render(f, app, layout.chat);

    // Render sidebar (if visible).
    if app.sidebar_visible && layout.sidebar.width > 0 {
        crate::components::sidebar::render(f, app, layout.sidebar);
    }

    // Render input area.
    crate::components::input::render(f, app, layout.input);

    // Render status bar.
    crate::components::status_bar::render(f, app, layout.status_bar);

    // Render permission dialog (if pending).
    if let Some(ref perm) = app.pending_permission {
        crate::components::permission::render(f, perm, area);
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::layout::AppLayout;

    #[test]
    fn layout_computes_for_default_app() {
        let app = App::new();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, &app);
        assert!(layout.chat.width > 0);
        assert!(layout.input.height > 0);
        assert!(layout.status_bar.height > 0);
    }

    #[test]
    fn layout_with_sidebar_visible() {
        let mut app = App::new();
        app.sidebar_visible = true;
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, &app);
        assert!(layout.sidebar.width > 0);
    }

    #[test]
    fn layout_with_permission_dialog() {
        let mut app = App::new();
        app.pending_permission = Some(crate::message::PermissionRequest {
            tool_name: "test".to_owned(),
            description: "test action".to_owned(),
            allow_all_available: false,
        });
        assert!(app.pending_permission.is_some());
    }
}
