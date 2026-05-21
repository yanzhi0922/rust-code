//! Event handling for the TUI.
//!
//! Translates crossterm terminal events into [`AppEvent`] values
//! and processes them against the [`App`](crate::app::App) state.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::app::{App, AppAction};

/// Application-level event (abstracted from crossterm).
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Key press.
    Key(KeyEvent),
    /// Mouse event.
    Mouse(MouseEvent),
    /// Terminal resize.
    Resize(u16, u16),
}

/// Convert a crossterm event into an AppEvent.
pub fn convert_event(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(key) => {
            // Suppress key release events (some terminals send them).
            if key.kind == crossterm::event::KeyEventKind::Release {
                return None;
            }
            Some(AppEvent::Key(key))
        }
        Event::Mouse(mouse) => Some(AppEvent::Mouse(mouse)),
        Event::Resize(w, h) => Some(AppEvent::Resize(w, h)),
        _ => None,
    }
}

/// Process an AppEvent against the App state, returning the resulting action.
pub fn handle_event(app: &mut App, event: AppEvent) -> AppAction {
    match event {
        AppEvent::Key(key) => handle_key(app, key),
        AppEvent::Mouse(mouse) => handle_mouse(app, mouse),
        AppEvent::Resize(width, height) => {
            handle_resize(app, width, height);
            AppAction::None
        }
    }
}

/// Handle a key event.
fn handle_key(app: &mut App, key: KeyEvent) -> AppAction {
    use crate::vim::VimMode;

    let mode = app.mode();

    match mode {
        VimMode::Insert => handle_insert_key(app, key),
        VimMode::Normal => handle_normal_key(app, key),
        VimMode::Command | VimMode::Search => handle_buffer_key(app, key),
        VimMode::Visual => handle_visual_key(app, key),
    }
}

/// Handle keys in Insert mode.
fn handle_insert_key(app: &mut App, key: KeyEvent) -> AppAction {
    match key.code {
        KeyCode::Esc => {
            let action = app.vim.handle_key(key);
            app.handle_vim_action(action)
        }
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.insert_char('\n');
                AppAction::None
            } else {
                app.submit_input()
            }
        }
        KeyCode::Char(c) => {
            if c == 'c' && key.modifiers.contains(KeyModifiers::CONTROL) {
                if app.input.is_empty() {
                    app.quit();
                    return AppAction::Quit;
                }
                app.clear_input();
                return AppAction::Cancel;
            }
            if c == 'r' && key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+R reverse search — handled by history search.
                return AppAction::None;
            }
            app.insert_char(c);
            AppAction::None
        }
        KeyCode::Backspace => {
            app.backspace();
            AppAction::None
        }
        KeyCode::Delete => {
            app.delete_char();
            AppAction::None
        }
        KeyCode::Left => {
            app.cursor_left();
            AppAction::None
        }
        KeyCode::Right => {
            app.cursor_right();
            AppAction::None
        }
        KeyCode::Home => {
            app.cursor_home();
            AppAction::None
        }
        KeyCode::End => {
            app.cursor_end();
            AppAction::None
        }
        KeyCode::Up => {
            app.history_up();
            AppAction::None
        }
        KeyCode::Down => {
            app.history_down();
            AppAction::None
        }
        KeyCode::Tab => {
            // Tab completion — return None, caller handles completion.
            AppAction::None
        }
        _ => AppAction::None,
    }
}

/// Handle keys in Normal mode.
fn handle_normal_key(app: &mut App, key: KeyEvent) -> AppAction {
    // Special cases before passing to Vim state machine.
    match key.code {
        KeyCode::Char('q') => {
            app.quit();
            return AppAction::Quit;
        }
        KeyCode::Tab => {
            app.toggle_sidebar();
            return AppAction::None;
        }
        _ => {}
    }

    let action = app.vim.handle_key(key);
    app.handle_vim_action(action)
}

/// Handle keys in Command/Search buffer mode.
fn handle_buffer_key(app: &mut App, key: KeyEvent) -> AppAction {
    let action = app.vim.handle_key(key);
    app.handle_vim_action(action)
}

/// Handle keys in Visual mode.
fn handle_visual_key(app: &mut App, key: KeyEvent) -> AppAction {
    let action = app.vim.handle_key(key);
    app.handle_vim_action(action)
}

/// Handle a mouse event.
fn handle_mouse(app: &mut App, mouse: MouseEvent) -> AppAction {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            app.scroll.scroll_up(3);
        }
        MouseEventKind::ScrollDown => {
            app.scroll.scroll_down(3);
        }
        _ => {}
    }
    AppAction::None
}

/// Handle a terminal resize event.
fn handle_resize(app: &mut App, _width: u16, height: u16) {
    // Recalculate scroll viewport — the render pass will update it.
    // For now, just ensure scroll doesn't overflow.
    app.scroll.set_viewport_height(
        height.saturating_sub(3).max(1) as usize, // minus status bar + input
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::CONTROL, KeyEventKind::Press)
    }

    #[test]
    fn convert_key_event() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        let app_event = convert_event(event);
        assert!(app_event.is_some());
    }

    #[test]
    fn convert_mouse_event() {
        // MouseEvent fields are public in crossterm 0.29.
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        let event = Event::Mouse(mouse);
        let app_event = convert_event(event);
        assert!(app_event.is_some());
    }

    #[test]
    fn convert_resize_event() {
        let event = Event::Resize(80, 24);
        let app_event = convert_event(event);
        assert!(matches!(app_event, Some(AppEvent::Resize(80, 24))));
    }

    #[test]
    fn insert_mode_enter_submits() {
        let mut app = App::new();
        app.input = "hello".to_owned();
        app.cursor = 5;
        let action = handle_key(&mut app, key(KeyCode::Enter));
        assert_eq!(action, AppAction::Submit("hello".to_owned()));
    }

    #[test]
    fn insert_mode_esc_to_normal() {
        let mut app = App::new();
        let _action = handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode(), crate::vim::VimMode::Normal);
    }

    #[test]
    fn insert_mode_typing() {
        let mut app = App::new();
        handle_key(&mut app, key(KeyCode::Char('h')));
        handle_key(&mut app, key(KeyCode::Char('i')));
        assert_eq!(app.input, "hi");
    }

    #[test]
    fn normal_mode_q_quits() {
        let mut app = App::new();
        handle_key(&mut app, key(KeyCode::Esc)); // -> Normal
        let action = handle_key(&mut app, key(KeyCode::Char('q')));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn mouse_scroll_adjusts_scroll() {
        let mut app = App::new();
        app.scroll.set_items(100);
        // Scroll down first so we can scroll up.
        app.scroll.scroll_down(10);
        let offset_before = app.scroll.scroll_offset();
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, mouse);
        assert!(app.scroll.scroll_offset() < offset_before);
    }

    #[test]
    fn ctrl_c_clears_input() {
        let mut app = App::new();
        app.input = "some text".to_owned();
        app.cursor = 9;
        let action = handle_key(&mut app, ctrl_key(KeyCode::Char('c')));
        assert_eq!(action, AppAction::Cancel);
        assert!(app.input.is_empty());
    }

    #[test]
    fn ctrl_c_empty_input_quits() {
        let mut app = App::new();
        let action = handle_key(&mut app, ctrl_key(KeyCode::Char('c')));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn resize_updates_viewport() {
        let mut app = App::new();
        handle_resize(&mut app, 120, 40);
        // viewport_height is a private field; just verify no panic.
        assert!(app.scroll.scroll_offset() == 0);
    }

    #[test]
    fn backspace_deletes_char() {
        let mut app = App::new();
        app.input = "ab".to_owned();
        app.cursor = 2;
        handle_key(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.input, "a");
    }
}
