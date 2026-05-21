//! Vim-like mode state machine for the TUI.
//!
//! Supports Normal, Insert, Command (`:`), Visual, and Search (`/`) modes.
//! Produces [`VimAction`] values that the [`App`](crate::app::App) interprets.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Vim editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    /// Normal mode — single-key commands for navigation.
    Normal,
    /// Insert mode — text input.
    Insert,
    /// Command mode — `:` prefix commands.
    Command,
    /// Visual mode — text selection.
    Visual,
    /// Search mode — `/` forward search.
    Search,
}

impl VimMode {
    /// Short uppercase label for display in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Command => "COMMAND",
            Self::Visual => "VISUAL",
            Self::Search => "SEARCH",
        }
    }
}

/// Direction for character-find motions (`f`, `F`, `t`, `T`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDirection {
    /// `f` — find forward.
    Forward,
    /// `F` — find backward.
    Backward,
    /// `t` — till forward (stop one before).
    TillForward,
    /// `T` — till backward (stop one after).
    TillBackward,
}

/// State for the last character-find motion (for `;` / `,` repeat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindState {
    pub direction: FindDirection,
    pub char: char,
}

/// Action produced by the Vim state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimAction {
    /// Move cursor up.
    MoveUp,
    /// Move cursor down.
    MoveDown,
    /// Move cursor left.
    MoveLeft,
    /// Move cursor right.
    MoveRight,
    /// Move to top (`gg`).
    MoveTop,
    /// Move to bottom (`G`).
    MoveBottom,
    /// Scroll half-page up (`Ctrl-U`).
    PageUp,
    /// Scroll half-page down (`Ctrl-D`).
    PageDown,
    /// Enter insert mode (`i`).
    EnterInsert,
    /// Enter insert mode after cursor (`a`).
    EnterInsertAfter,
    /// Enter insert mode at line end (`A`).
    EnterInsertLineEnd,
    /// Enter insert mode at line start (`I`).
    EnterInsertLineStart,
    /// Enter insert mode on new line below (`o`).
    EnterInsertNewLineBelow,
    /// Enter insert mode on new line above (`O`).
    EnterInsertNewLineAbove,
    /// Return to normal mode (`Esc`).
    ExitToNormal,
    /// Delete current line (`dd`).
    DeleteLine,
    /// Yank current line (`yy`).
    YankLine,
    /// Paste after (`p`).
    PasteAfter,
    /// Paste before (`P`).
    PasteBefore,
    /// Undo (`u`).
    Undo,
    /// Find character motion.
    FindChar(FindState),
    /// Repeat last find (`;`).
    RepeatFind,
    /// Repeat last find in reverse (`,`).
    RepeatFindReverse,
    /// Enter command mode (`:`).
    EnterCommand,
    /// Enter search mode (`/`).
    EnterSearch,
    /// Submit a command string.
    CommandSubmit(String),
    /// Submit a search string.
    SearchSubmit(String),
    /// Delete one character backward in command/search buffer.
    BufferBackspace,
    /// Append character to command/search buffer.
    BufferChar(char),
    /// No action (key consumed but no effect).
    Noop,
}

/// Vim mode state machine.
#[derive(Debug, Clone)]
pub struct VimStateMachine {
    mode: VimMode,
    /// Pending key for two-character combos (e.g. `g` waiting for second `g`).
    pending_g: bool,
    /// Pending `d` for `dd`.
    pending_d: bool,
    /// Pending `y` for `yy`.
    pending_y: bool,
    /// Pending `f`/`F`/`t`/`T` — waiting for the target character.
    pending_find: Option<FindDirection>,
    /// Command buffer (`:` prefix).
    command_buffer: String,
    /// Search buffer (`/` prefix).
    search_buffer: String,
    /// Last character-find state for `;` / `,`.
    last_find: Option<FindState>,
}

impl VimStateMachine {
    /// Create a new state machine starting in Insert mode.
    pub fn new() -> Self {
        VimStateMachine {
            mode: VimMode::Insert,
            pending_g: false,
            pending_d: false,
            pending_y: false,
            pending_find: None,
            command_buffer: String::new(),
            search_buffer: String::new(),
            last_find: None,
        }
    }

    /// Current mode.
    pub fn mode(&self) -> VimMode {
        self.mode
    }

    /// Command buffer content (for rendering the `:` prompt).
    pub fn command_buffer(&self) -> &str {
        &self.command_buffer
    }

    /// Search buffer content (for rendering the `/` prompt).
    pub fn search_buffer(&self) -> &str {
        &self.search_buffer
    }

    /// Reset all state to Insert mode.
    pub fn reset(&mut self) {
        self.mode = VimMode::Insert;
        self.pending_g = false;
        self.pending_d = false;
        self.pending_y = false;
        self.pending_find = None;
        self.command_buffer.clear();
        self.search_buffer.clear();
        self.last_find = None;
    }

    /// Process a key event and return the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> VimAction {
        match self.mode {
            VimMode::Normal => self.handle_normal(key),
            VimMode::Insert => self.handle_insert(key),
            VimMode::Command => self.handle_command(key),
            VimMode::Visual => self.handle_visual(key),
            VimMode::Search => self.handle_search(key),
        }
    }

    // -- Normal mode --------------------------------------------------------

    fn handle_normal(&mut self, key: KeyEvent) -> VimAction {
        // Handle pending combos first.
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                return VimAction::MoveTop;
            }
            // Second key wasn't 'g' — fall through to normal handling.
        }
        if self.pending_d {
            self.pending_d = false;
            if key.code == KeyCode::Char('d') {
                return VimAction::DeleteLine;
            }
        }
        if self.pending_y {
            self.pending_y = false;
            if key.code == KeyCode::Char('y') {
                return VimAction::YankLine;
            }
        }
        if let Some(direction) = self.pending_find.take()
            && let KeyCode::Char(ch) = key.code
        {
            let state = FindState {
                direction,
                char: ch,
            };
            self.last_find = Some(state);
            return VimAction::FindChar(state);
        }
        // Not a char key — cancel the pending find and fall through.

        match key.code {
            KeyCode::Char('h') | KeyCode::Left => VimAction::MoveLeft,
            KeyCode::Char('j') | KeyCode::Down => VimAction::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => VimAction::MoveUp,
            KeyCode::Char('l') | KeyCode::Right => VimAction::MoveRight,
            KeyCode::Char('G') => VimAction::MoveBottom,
            KeyCode::Char('g') => {
                self.pending_g = true;
                VimAction::Noop
            }
            KeyCode::Char('d') => {
                self.pending_d = true;
                VimAction::Noop
            }
            KeyCode::Char('y') => {
                self.pending_y = true;
                VimAction::Noop
            }
            KeyCode::Char('i') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsert
            }
            KeyCode::Char('a') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsertAfter
            }
            KeyCode::Char('A') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsertLineEnd
            }
            KeyCode::Char('I') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsertLineStart
            }
            KeyCode::Char('o') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsertNewLineBelow
            }
            KeyCode::Char('O') => {
                self.mode = VimMode::Insert;
                VimAction::EnterInsertNewLineAbove
            }
            KeyCode::Char('p') => VimAction::PasteAfter,
            KeyCode::Char('P') => VimAction::PasteBefore,
            KeyCode::Char('u') => VimAction::Undo,
            KeyCode::Char('v') => {
                self.mode = VimMode::Visual;
                VimAction::Noop
            }
            KeyCode::Char(':') => {
                self.mode = VimMode::Command;
                self.command_buffer.clear();
                VimAction::EnterCommand
            }
            KeyCode::Char('/') => {
                self.mode = VimMode::Search;
                self.search_buffer.clear();
                VimAction::EnterSearch
            }
            KeyCode::Char('f') => {
                self.pending_find = Some(FindDirection::Forward);
                VimAction::Noop
            }
            KeyCode::Char('F') => {
                self.pending_find = Some(FindDirection::Backward);
                VimAction::Noop
            }
            KeyCode::Char('t') => {
                self.pending_find = Some(FindDirection::TillForward);
                VimAction::Noop
            }
            KeyCode::Char('T') => {
                self.pending_find = Some(FindDirection::TillBackward);
                VimAction::Noop
            }
            KeyCode::Char(';') => {
                if self.last_find.is_some() {
                    VimAction::RepeatFind
                } else {
                    VimAction::Noop
                }
            }
            KeyCode::Char(',') => {
                if self.last_find.is_some() {
                    VimAction::RepeatFindReverse
                } else {
                    VimAction::Noop
                }
            }
            KeyCode::Char('q') => VimAction::ExitToNormal,
            KeyCode::Esc => VimAction::Noop,
            _ => VimAction::Noop,
        }
    }

    // -- Insert mode --------------------------------------------------------

    fn handle_insert(&mut self, key: KeyEvent) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                VimAction::ExitToNormal
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = VimMode::Normal;
                VimAction::ExitToNormal
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                VimAction::PageUp
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                VimAction::PageDown
            }
            _ => VimAction::Noop,
        }
    }

    // -- Command mode -------------------------------------------------------

    fn handle_command(&mut self, key: KeyEvent) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                self.command_buffer.clear();
                VimAction::ExitToNormal
            }
            KeyCode::Enter => {
                let cmd = self.command_buffer.clone();
                self.mode = VimMode::Normal;
                self.command_buffer.clear();
                VimAction::CommandSubmit(cmd)
            }
            KeyCode::Backspace => {
                if self.command_buffer.pop().is_none() {
                    self.mode = VimMode::Normal;
                }
                VimAction::BufferBackspace
            }
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
                VimAction::BufferChar(c)
            }
            _ => VimAction::Noop,
        }
    }

    // -- Visual mode --------------------------------------------------------

    fn handle_visual(&mut self, key: KeyEvent) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                VimAction::ExitToNormal
            }
            KeyCode::Char('h') | KeyCode::Left => VimAction::MoveLeft,
            KeyCode::Char('j') | KeyCode::Down => VimAction::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => VimAction::MoveUp,
            KeyCode::Char('l') | KeyCode::Right => VimAction::MoveRight,
            _ => VimAction::Noop,
        }
    }

    // -- Search mode --------------------------------------------------------

    fn handle_search(&mut self, key: KeyEvent) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                self.search_buffer.clear();
                VimAction::ExitToNormal
            }
            KeyCode::Enter => {
                let query = self.search_buffer.clone();
                self.mode = VimMode::Normal;
                self.search_buffer.clear();
                VimAction::SearchSubmit(query)
            }
            KeyCode::Backspace => {
                if self.search_buffer.pop().is_none() {
                    self.mode = VimMode::Normal;
                }
                VimAction::BufferBackspace
            }
            KeyCode::Char(c) => {
                self.search_buffer.push(c);
                VimAction::BufferChar(c)
            }
            _ => VimAction::Noop,
        }
    }
}

impl Default for VimStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn starts_in_insert_mode() {
        let vm = VimStateMachine::new();
        assert_eq!(vm.mode(), VimMode::Insert);
    }

    #[test]
    fn esc_switches_to_normal() {
        let mut vm = VimStateMachine::new();
        let action = vm.handle_key(key(KeyCode::Esc));
        assert_eq!(vm.mode(), VimMode::Normal);
        assert_eq!(action, VimAction::ExitToNormal);
    }

    #[test]
    fn normal_i_enters_insert() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc)); // -> Normal
        let action = vm.handle_key(key(KeyCode::Char('i')));
        assert_eq!(vm.mode(), VimMode::Insert);
        assert_eq!(action, VimAction::EnterInsert);
    }

    #[test]
    fn normal_gg_moves_top() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc)); // -> Normal
        vm.handle_key(key(KeyCode::Char('g'))); // pending
        let action = vm.handle_key(key(KeyCode::Char('g')));
        assert_eq!(action, VimAction::MoveTop);
    }

    #[test]
    fn normal_g_moves_bottom() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        let action = vm.handle_key(key(KeyCode::Char('G')));
        assert_eq!(action, VimAction::MoveBottom);
    }

    #[test]
    fn normal_dd_deletes_line() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char('d')));
        let action = vm.handle_key(key(KeyCode::Char('d')));
        assert_eq!(action, VimAction::DeleteLine);
    }

    #[test]
    fn command_mode_submit_and_clear() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char(':'))); // -> Command
        assert_eq!(vm.mode(), VimMode::Command);
        vm.handle_key(key(KeyCode::Char('q')));
        let action = vm.handle_key(key(KeyCode::Enter));
        assert_eq!(action, VimAction::CommandSubmit("q".to_owned()));
        assert_eq!(vm.mode(), VimMode::Normal);
    }

    #[test]
    fn command_mode_esc_cancels() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char(':')));
        vm.handle_key(key(KeyCode::Char('q')));
        vm.handle_key(key(KeyCode::Esc));
        assert_eq!(vm.mode(), VimMode::Normal);
        assert!(vm.command_buffer().is_empty());
    }

    #[test]
    fn search_mode_submit() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char('/')));
        assert_eq!(vm.mode(), VimMode::Search);
        vm.handle_key(key(KeyCode::Char('h')));
        vm.handle_key(key(KeyCode::Char('i')));
        let action = vm.handle_key(key(KeyCode::Enter));
        assert_eq!(action, VimAction::SearchSubmit("hi".to_owned()));
        assert_eq!(vm.mode(), VimMode::Normal);
    }

    #[test]
    fn ctrl_c_exits_insert() {
        let mut vm = VimStateMachine::new();
        let action = vm.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(vm.mode(), VimMode::Normal);
        assert_eq!(action, VimAction::ExitToNormal);
    }

    #[test]
    fn reset_clears_all_state() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char(':')));
        vm.command_buffer.push_str("test");
        vm.reset();
        assert_eq!(vm.mode(), VimMode::Insert);
        assert!(vm.command_buffer().is_empty());
        assert!(vm.search_buffer().is_empty());
    }

    #[test]
    fn visual_mode_navigation() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char('v')));
        assert_eq!(vm.mode(), VimMode::Visual);
        let action = vm.handle_key(key(KeyCode::Char('j')));
        assert_eq!(action, VimAction::MoveDown);
        vm.handle_key(key(KeyCode::Esc));
        assert_eq!(vm.mode(), VimMode::Normal);
    }

    #[test]
    fn mode_label_matches() {
        assert_eq!(VimMode::Normal.label(), "NORMAL");
        assert_eq!(VimMode::Insert.label(), "INSERT");
        assert_eq!(VimMode::Command.label(), "COMMAND");
        assert_eq!(VimMode::Visual.label(), "VISUAL");
        assert_eq!(VimMode::Search.label(), "SEARCH");
    }

    #[test]
    fn yy_yanks_line() {
        let mut vm = VimStateMachine::new();
        vm.handle_key(key(KeyCode::Esc));
        vm.handle_key(key(KeyCode::Char('y')));
        let action = vm.handle_key(key(KeyCode::Char('y')));
        assert_eq!(action, VimAction::YankLine);
    }
}
