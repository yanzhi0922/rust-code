//! Main application state machine.
//!
//! [`App`] holds all UI state and processes actions from the Vim state machine
//! and event handler. It is the single source of truth for what the TUI renders.

use crate::message::{
    ChatMessage, McpServerStatus, MessageRole, ModelInfo, PermissionRequest, StatusBarInfo,
    ToolCallInfo,
};
use crate::scroll::VirtualScroll;
use crate::style::StyleConfig;
use crate::vim::{VimAction, VimMode, VimStateMachine};

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Chat,
    Input,
    Sidebar,
}

/// Which tab is active in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Sessions,
    Tools,
    Mcp,
    Help,
}

impl SidebarTab {
    /// All tab labels in order.
    pub fn all() -> &'static [SidebarTab] {
        &[
            SidebarTab::Sessions,
            SidebarTab::Tools,
            SidebarTab::Mcp,
            SidebarTab::Help,
        ]
    }

    /// Short display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Tools => "Tools",
            Self::Mcp => "MCP",
            Self::Help => "Help",
        }
    }

    /// Cycle to the next tab.
    pub fn next(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }
}

/// Action returned by the App after processing input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    /// No action needed.
    None,
    /// User submitted input text.
    Submit(String),
    /// User requested quit.
    Quit,
    /// User cancelled current operation.
    Cancel,
    /// Slash command to execute.
    SlashCommand(String),
}

/// Main application state.
pub struct App {
    /// Vim mode state machine.
    pub vim: VimStateMachine,
    /// Chat messages.
    pub messages: Vec<ChatMessage>,
    /// Current text input buffer.
    pub input: String,
    /// Cursor position within the input buffer (byte index).
    pub cursor: usize,
    /// Virtual scroll state for the chat area.
    pub scroll: VirtualScroll,
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Currently selected sidebar tab.
    pub sidebar_tab: SidebarTab,
    /// Currently active panel.
    pub active_panel: ActivePanel,
    /// Status bar information.
    pub status: StatusBarInfo,
    /// Whether the app should exit.
    should_quit: bool,
    /// Pending permission request (if any).
    pub pending_permission: Option<PermissionRequest>,
    /// MCP server statuses.
    pub mcp_servers: Vec<McpServerStatus>,
    /// Model information.
    pub model_info: ModelInfo,
    /// Style configuration.
    pub style: StyleConfig,
    /// Input history for up/down navigation.
    pub input_history: Vec<String>,
    /// Current position in input history.
    pub history_index: usize,
    /// Saved buffer when navigating history.
    pub saved_buffer: String,
    /// Spinner frame counter for progress indication.
    pub spinner_frame: usize,
    /// Whether we are currently streaming a response.
    pub is_streaming: bool,
}

impl App {
    /// Create a new App with default state.
    pub fn new() -> Self {
        App {
            vim: VimStateMachine::new(),
            messages: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll: VirtualScroll::new(24),
            sidebar_visible: false,
            sidebar_tab: SidebarTab::Help,
            active_panel: ActivePanel::Input,
            status: StatusBarInfo::default(),
            should_quit: false,
            pending_permission: None,
            mcp_servers: Vec::new(),
            model_info: ModelInfo::default(),
            style: StyleConfig::dark(),
            input_history: Vec::new(),
            history_index: 0,
            saved_buffer: String::new(),
            spinner_frame: 0,
            is_streaming: false,
        }
    }

    /// Current Vim mode.
    pub fn mode(&self) -> VimMode {
        self.vim.mode()
    }

    /// Process a Vim action and update state.
    pub fn handle_vim_action(&mut self, action: VimAction) -> AppAction {
        match action {
            VimAction::ExitToNormal => {
                self.status.mode_label = "NORMAL".to_owned();
                AppAction::None
            }
            VimAction::EnterInsert
            | VimAction::EnterInsertAfter
            | VimAction::EnterInsertLineEnd
            | VimAction::EnterInsertLineStart
            | VimAction::EnterInsertNewLineBelow
            | VimAction::EnterInsertNewLineAbove => {
                self.status.mode_label = "INSERT".to_owned();
                self.active_panel = ActivePanel::Input;
                AppAction::None
            }
            VimAction::MoveUp | VimAction::MoveDown => {
                // In normal mode, scroll the chat area.
                match action {
                    VimAction::MoveUp => self.scroll.scroll_up(1),
                    VimAction::MoveDown => self.scroll.scroll_down(1),
                    _ => {}
                }
                AppAction::None
            }
            VimAction::MoveTop => {
                self.scroll.scroll_to_top();
                AppAction::None
            }
            VimAction::MoveBottom => {
                self.scroll.scroll_to_bottom();
                AppAction::None
            }
            VimAction::PageUp => {
                self.scroll.page_up();
                AppAction::None
            }
            VimAction::PageDown => {
                self.scroll.page_down();
                AppAction::None
            }
            VimAction::CommandSubmit(cmd) => {
                let trimmed = cmd.trim();
                match trimmed {
                    "q" | "quit" | "exit" => {
                        self.should_quit = true;
                        AppAction::Quit
                    }
                    "help" => {
                        self.sidebar_visible = true;
                        self.sidebar_tab = SidebarTab::Help;
                        AppAction::None
                    }
                    "sidebar" | "sb" => {
                        self.sidebar_visible = !self.sidebar_visible;
                        AppAction::None
                    }
                    "clear" => AppAction::SlashCommand("/clear".to_owned()),
                    _ => AppAction::SlashCommand(format!("/{trimmed}")),
                }
            }
            VimAction::SearchSubmit(_query) => {
                // Search is handled by the caller.
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    /// Add a message and update scroll state.
    pub fn add_message(&mut self, message: ChatMessage) {
        let height = message.estimated_height(80);
        self.messages.push(message);
        self.scroll.set_items(self.messages.len());
        let last_idx = self.messages.len().saturating_sub(1);
        self.scroll.set_item_height(last_idx, height);
    }

    /// Append streaming content to the last assistant message.
    pub fn append_to_last_assistant(&mut self, content: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == MessageRole::Assistant
        {
            last.content.push_str(content);
        }
    }

    /// Add a tool call to the last assistant message.
    pub fn add_tool_call(&mut self, tool_call: ToolCallInfo) {
        if let Some(last) = self.messages.last_mut()
            && last.role == MessageRole::Assistant
        {
            last.tool_calls.push(tool_call);
        }
    }

    /// Get the current input text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Clear the input buffer and reset cursor.
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    /// Submit the current input, returning it as an action.
    pub fn submit_input(&mut self) -> AppAction {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return AppAction::None;
        }

        // Save to history.
        const MAX_HISTORY: usize = 1000;
        if self.input_history.last() != Some(&text) {
            self.input_history.push(text.clone());
            if self.input_history.len() > MAX_HISTORY {
                self.input_history.remove(0);
            }
        }
        self.history_index = self.input_history.len();
        self.saved_buffer.clear();

        self.input.clear();
        self.cursor = 0;

        if text.starts_with('/') {
            AppAction::SlashCommand(text)
        } else {
            AppAction::Submit(text)
        }
    }

    /// Navigate to the previous entry in input history.
    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.history_index == self.input_history.len() {
            self.saved_buffer = self.input.clone();
        }
        if self.history_index > 0 {
            self.history_index -= 1;
            self.input = self.input_history[self.history_index].clone();
            self.cursor = self.input.len();
        }
    }

    /// Navigate to the next entry in input history.
    pub fn history_down(&mut self) {
        if self.history_index < self.input_history.len() {
            self.history_index += 1;
            if self.history_index == self.input_history.len() {
                self.input = self.saved_buffer.clone();
            } else {
                self.input = self.input_history[self.history_index].clone();
            }
            self.cursor = self.input.len();
        }
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.input.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.input.is_empty() {
            // Find the previous character boundary.
            let prev = self
                .input
                .char_indices()
                .rev()
                .find(|(i, _)| *i < self.cursor)
                .map(|(i, _)| i);
            if let Some(prev_idx) = prev {
                self.input.drain(prev_idx..self.cursor);
                self.cursor = prev_idx;
            }
        }
    }

    /// Delete the character at the cursor.
    pub fn delete_char(&mut self) {
        if self.cursor < self.input.len() {
            let next = self
                .input
                .char_indices()
                .find(|(i, _)| *i > self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.input.len());
            self.input.drain(self.cursor..next);
        }
    }

    /// Move cursor left.
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            let prev = self
                .input
                .char_indices()
                .rev()
                .find(|(i, _)| *i < self.cursor)
                .map(|(i, _)| i);
            if let Some(prev_idx) = prev {
                self.cursor = prev_idx;
            }
        }
    }

    /// Move cursor right.
    pub fn cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            let next = self
                .input
                .char_indices()
                .find(|(i, _)| *i > self.cursor)
                .map(|(i, _)| i);
            if let Some(next_idx) = next {
                self.cursor = next_idx;
            }
        }
    }

    /// Move cursor to start of input.
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end of input.
    pub fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Whether the app should exit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Signal the app to quit.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Toggle sidebar visibility.
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
    }

    /// Cycle to the next sidebar tab.
    pub fn next_sidebar_tab(&mut self) {
        self.sidebar_tab = self.sidebar_tab.next();
    }

    /// Advance the spinner frame.
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 8;
    }

    /// Get the current spinner character.
    pub fn spinner_char(&self) -> &str {
        const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        SPINNER[self.spinner_frame % SPINNER.len()]
    }

    /// Update viewport height for scroll calculations.
    pub fn update_scroll_viewport(&mut self, height: usize) {
        self.scroll.set_viewport_height(height);
    }

    /// Reset transient UI state for a newly-created session after `/clear` or
    /// other session-switching flows that should not leak the previous
    /// session's visible state.
    pub fn reset_for_new_session(&mut self) {
        self.messages.clear();
        self.scroll.set_items(0);
        self.scroll.scroll_to_top();
        self.pending_permission = None;
        self.mcp_servers.clear();
        self.clear_input();
        self.history_index = self.input_history.len();
        self.saved_buffer.clear();
        self.is_streaming = false;
        self.spinner_frame = 0;
        self.active_panel = ActivePanel::Input;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_starts_in_insert() {
        let app = App::new();
        assert_eq!(app.mode(), VimMode::Insert);
        assert!(app.input.is_empty());
        assert!(!app.should_quit());
    }

    #[test]
    fn add_message_updates_scroll() {
        let mut app = App::new();
        app.add_message(ChatMessage::user("hello".to_owned()));
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn submit_input_returns_action() {
        let mut app = App::new();
        app.input = "test message".to_owned();
        app.cursor = app.input.len();
        let action = app.submit_input();
        assert_eq!(action, AppAction::Submit("test message".to_owned()));
        assert!(app.input.is_empty());
    }

    #[test]
    fn submit_slash_command() {
        let mut app = App::new();
        app.input = "/help".to_owned();
        app.cursor = app.input.len();
        let action = app.submit_input();
        assert_eq!(action, AppAction::SlashCommand("/help".to_owned()));
    }

    #[test]
    fn insert_and_backspace() {
        let mut app = App::new();
        app.insert_char('a');
        app.insert_char('b');
        app.insert_char('c');
        assert_eq!(app.input, "abc");
        app.backspace();
        assert_eq!(app.input, "ab");
    }

    #[test]
    fn cursor_navigation() {
        let mut app = App::new();
        app.input = "hello".to_owned();
        app.cursor = app.input.len();
        // Move left from end.
        app.cursor_left();
        assert!(
            app.cursor < app.input.len(),
            "cursor should move left from end"
        );
        // Move right — should advance by one char.
        let before_right = app.cursor;
        app.cursor_right();
        assert!(app.cursor >= before_right, "cursor should not go backwards");
        // Home should go to 0.
        app.cursor_home();
        assert_eq!(app.cursor, 0, "home should set cursor to 0");
        // End should go to len.
        app.cursor_end();
        assert_eq!(
            app.cursor,
            app.input.len(),
            "end should set cursor to input len"
        );
    }

    #[test]
    fn history_navigation() {
        let mut app = App::new();
        app.input = "first".to_owned();
        app.cursor = 5;
        app.submit_input();
        app.input = "second".to_owned();
        app.cursor = 6;
        app.submit_input();

        app.history_up();
        assert_eq!(app.input, "second");
        app.history_up();
        assert_eq!(app.input, "first");
        app.history_down();
        assert_eq!(app.input, "second");
    }

    #[test]
    fn toggle_sidebar() {
        let mut app = App::new();
        assert!(!app.sidebar_visible);
        app.toggle_sidebar();
        assert!(app.sidebar_visible);
        app.toggle_sidebar();
        assert!(!app.sidebar_visible);
    }

    #[test]
    fn sidebar_tab_cycles() {
        let mut app = App::new();
        assert_eq!(app.sidebar_tab, SidebarTab::Help);
        app.next_sidebar_tab();
        assert_eq!(app.sidebar_tab, SidebarTab::Sessions);
    }

    #[test]
    fn append_to_last_assistant() {
        let mut app = App::new();
        app.add_message(ChatMessage::assistant("hello".to_owned()));
        app.append_to_last_assistant(" world");
        assert_eq!(app.messages[0].content, "hello world");
    }

    #[test]
    fn quit_sets_flag() {
        let mut app = App::new();
        app.quit();
        assert!(app.should_quit());
    }

    #[test]
    fn spinner_cycles() {
        let mut app = App::new();
        let first = app.spinner_char().to_owned();
        app.tick_spinner();
        let second = app.spinner_char().to_owned();
        assert_ne!(first, second);
    }

    #[test]
    fn command_submit_quit() {
        let mut app = App::new();
        let action = app.handle_vim_action(VimAction::CommandSubmit("q".to_owned()));
        assert_eq!(action, AppAction::Quit);
        assert!(app.should_quit());
    }

    #[test]
    fn empty_submit_returns_none() {
        let mut app = App::new();
        let action = app.submit_input();
        assert_eq!(action, AppAction::None);
    }

    #[test]
    fn command_submit_clear_does_not_eagerly_drop_messages() {
        let mut app = App::new();
        app.add_message(ChatMessage::system("keep until clear succeeds".to_owned()));

        let action = app.handle_vim_action(VimAction::CommandSubmit("clear".to_owned()));

        assert_eq!(action, AppAction::SlashCommand("/clear".to_owned()));
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn reset_for_new_session_clears_transient_state() {
        let mut app = App::new();
        app.add_message(ChatMessage::assistant("old".to_owned()));
        app.pending_permission = Some(PermissionRequest {
            tool_name: "bash".to_owned(),
            description: "run shell".to_owned(),
            allow_all_available: true,
        });
        app.mcp_servers.push(McpServerStatus {
            name: "context7".to_owned(),
            status: "connected".to_owned(),
            tool_count: 3,
        });
        app.input = "stale input".to_owned();
        app.cursor = app.input.len();
        app.is_streaming = true;
        app.spinner_frame = 3;

        app.reset_for_new_session();

        assert!(app.messages.is_empty());
        assert!(app.pending_permission.is_none());
        assert!(app.mcp_servers.is_empty());
        assert!(app.input.is_empty());
        assert_eq!(app.cursor, 0);
        assert!(!app.is_streaming);
        assert_eq!(app.spinner_frame, 0);
        assert_eq!(app.active_panel, ActivePanel::Input);
    }
}
