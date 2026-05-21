//! Keybindings system for the TUI.
//!
//! Provides a flexible, context-aware keybinding registry inspired by
//! Claude Code's keybinding architecture. Supports:
//! - Multiple contexts (Global, Chat, Insert, Normal, Visual, Help, etc.)
//! - 30+ named actions (Submit, Cancel, HistoryUp, ScrollDown, etc.)
//! - Conflict detection and resolution
//! - Default keybindings matching Claude Code conventions
//! - Custom user overrides

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use anyhow::Result;

// ---------------------------------------------------------------------------
// Key representation
// ---------------------------------------------------------------------------

/// A parsed keystroke with modifiers.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Keystroke {
    /// The key name (e.g. "enter", "escape", "a", "up").
    pub key: String,
    /// Ctrl modifier.
    pub ctrl: bool,
    /// Alt/Meta modifier.
    pub alt: bool,
    /// Shift modifier.
    pub shift: bool,
}

impl Keystroke {
    /// Parse a keystroke string like `"ctrl+c"`, `"shift+tab"`, `"enter"`.
    pub fn parse(input: &str) -> Result<Self> {
        let lower = input.to_lowercase();
        let parts: Vec<&str> = lower.split('+').collect();
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key = String::new();

        for part in parts {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            match trimmed {
                "ctrl" => ctrl = true,
                "alt" | "meta" => alt = true,
                "shift" => shift = true,
                other => {
                    if !key.is_empty() {
                        return Err(anyhow::anyhow!(
                            "duplicate key in keystroke: '{key}' and '{other}'"
                        ));
                    }
                    key = other.to_string();
                }
            }
        }

        if key.is_empty() {
            return Err(anyhow::anyhow!("no key specified in keystroke: '{input}'"));
        }

        Ok(Keystroke {
            key,
            ctrl,
            alt,
            shift,
        })
    }

    /// Format as a human-readable shortcut string.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(self.format_key_name());
        parts.join("+")
    }

    fn format_key_name(&self) -> String {
        match self.key.as_str() {
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "tab" => "Tab".to_string(),
            "backspace" => "Backspace".to_string(),
            "delete" => "Del".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "pageup" => "PgUp".to_string(),
            "pagedown" => "PgDn".to_string(),
            "home" => "Home".to_string(),
            "end" => "End".to_string(),
            "space" => "Space".to_string(),
            other => {
                if other.len() == 1 {
                    other.to_uppercase()
                } else {
                    other.to_string()
                }
            }
        }
    }
}

impl Hash for Keystroke {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.ctrl.hash(state);
        self.alt.hash(state);
        self.shift.hash(state);
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Context in which a keybinding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyBindingContext {
    /// Active everywhere regardless of focus.
    Global,
    /// When the chat input is focused.
    Chat,
    /// Vim insert mode.
    Insert,
    /// Vim normal mode.
    Normal,
    /// Vim visual mode.
    Visual,
    /// When the help overlay is open.
    Help,
    /// When the command palette is open.
    CommandPalette,
    /// When autocomplete is visible.
    Autocomplete,
    /// When a confirmation dialog is shown.
    Confirmation,
    /// When viewing the transcript.
    Transcript,
    /// When searching history (ctrl+r).
    HistorySearch,
    /// When a task/agent is running.
    Task,
    /// When the settings menu is open.
    Settings,
    /// When tab navigation is active.
    Tabs,
}

impl KeyBindingContext {
    /// All context variants.
    pub fn all() -> &'static [KeyBindingContext] {
        &[
            KeyBindingContext::Global,
            KeyBindingContext::Chat,
            KeyBindingContext::Insert,
            KeyBindingContext::Normal,
            KeyBindingContext::Visual,
            KeyBindingContext::Help,
            KeyBindingContext::CommandPalette,
            KeyBindingContext::Autocomplete,
            KeyBindingContext::Confirmation,
            KeyBindingContext::Transcript,
            KeyBindingContext::HistorySearch,
            KeyBindingContext::Task,
            KeyBindingContext::Settings,
            KeyBindingContext::Tabs,
        ]
    }

    /// Human-readable description of the context.
    pub fn description(self) -> &'static str {
        match self {
            Self::Global => "Active everywhere, regardless of focus",
            Self::Chat => "When the chat input is focused",
            Self::Insert => "Vim insert mode",
            Self::Normal => "Vim normal mode",
            Self::Visual => "Vim visual mode",
            Self::Help => "When the help overlay is open",
            Self::CommandPalette => "When the command palette is open",
            Self::Autocomplete => "When autocomplete menu is visible",
            Self::Confirmation => "When a confirmation dialog is shown",
            Self::Transcript => "When viewing the transcript",
            Self::HistorySearch => "When searching command history",
            Self::Task => "When a task/agent is running",
            Self::Settings => "When the settings menu is open",
            Self::Tabs => "When tab navigation is active",
        }
    }
}

impl fmt::Display for KeyBindingContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => write!(f, "Global"),
            Self::Chat => write!(f, "Chat"),
            Self::Insert => write!(f, "Insert"),
            Self::Normal => write!(f, "Normal"),
            Self::Visual => write!(f, "Visual"),
            Self::Help => write!(f, "Help"),
            Self::CommandPalette => write!(f, "CommandPalette"),
            Self::Autocomplete => write!(f, "Autocomplete"),
            Self::Confirmation => write!(f, "Confirmation"),
            Self::Transcript => write!(f, "Transcript"),
            Self::HistorySearch => write!(f, "HistorySearch"),
            Self::Task => write!(f, "Task"),
            Self::Settings => write!(f, "Settings"),
            Self::Tabs => write!(f, "Tabs"),
        }
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Named actions that can be triggered by keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    // App-level
    /// Interrupt the current operation (Ctrl+C).
    AppInterrupt,
    /// Exit the application (Ctrl+D).
    AppExit,
    /// Redraw the screen.
    AppRedraw,
    /// Toggle todo panel.
    ToggleTodos,
    /// Toggle transcript view.
    ToggleTranscript,
    /// Toggle teammate preview.
    ToggleTeammatePreview,
    /// Toggle terminal panel.
    ToggleTerminal,

    // History navigation
    /// Search history (Ctrl+R).
    HistorySearch,
    /// Previous history entry.
    HistoryPrevious,
    /// Next history entry.
    HistoryNext,

    // Chat input
    /// Submit the current input.
    ChatSubmit,
    /// Cancel current input/operation.
    ChatCancel,
    /// Insert a newline.
    ChatNewline,
    /// Open external editor.
    ChatExternalEditor,
    /// Cycle input mode (Normal/Agent/Plan).
    ChatCycleMode,
    /// Open model picker.
    ChatModelPicker,
    /// Toggle fast mode.
    ChatFastMode,
    /// Toggle thinking mode.
    ChatThinkingToggle,
    /// Undo last edit.
    ChatUndo,
    /// Stash current input.
    ChatStash,
    /// Paste image from clipboard.
    ChatImagePaste,

    // Autocomplete
    /// Accept autocomplete suggestion.
    AutocompleteAccept,
    /// Dismiss autocomplete.
    AutocompleteDismiss,
    /// Previous autocomplete suggestion.
    AutocompletePrevious,
    /// Next autocomplete suggestion.
    AutocompleteNext,

    // Confirmation dialog
    /// Confirm "yes".
    ConfirmYes,
    /// Confirm "no".
    ConfirmNo,
    /// Previous option in dialog.
    ConfirmPrevious,
    /// Next option in dialog.
    ConfirmNext,
    /// Toggle selection in dialog.
    ConfirmToggle,
    /// Cycle mode in confirmation.
    ConfirmCycleMode,

    // Scrolling
    /// Scroll up one line.
    ScrollUp,
    /// Scroll down one line.
    ScrollDown,
    /// Scroll up one page.
    ScrollPageUp,
    /// Scroll down one page.
    ScrollPageDown,
    /// Scroll to top.
    ScrollToTop,
    /// Scroll to bottom.
    ScrollToBottom,

    // Tabs
    /// Next tab.
    TabsNext,
    /// Previous tab.
    TabsPrevious,

    // Transcript
    /// Toggle show all in transcript.
    TranscriptToggleShowAll,
    /// Exit transcript view.
    TranscriptExit,

    // History search
    /// Next match in history search.
    HistorySearchNext,
    /// Accept history search match.
    HistorySearchAccept,
    /// Cancel history search.
    HistorySearchCancel,
    /// Execute history search result.
    HistorySearchExecute,

    // Task
    /// Background the current task.
    TaskBackground,

    // Help
    /// Dismiss help overlay.
    HelpDismiss,

    // Vim
    /// Enter insert mode.
    VimInsertMode,
    /// Enter normal mode.
    VimNormalMode,
    /// Enter visual mode.
    VimVisualMode,
    /// Enter command mode.
    VimCommandMode,
    /// Enter search mode.
    VimSearchMode,
}

impl KeyAction {
    /// All action variants.
    pub fn all() -> &'static [KeyAction] {
        &[
            KeyAction::AppInterrupt,
            KeyAction::AppExit,
            KeyAction::AppRedraw,
            KeyAction::ToggleTodos,
            KeyAction::ToggleTranscript,
            KeyAction::ToggleTeammatePreview,
            KeyAction::ToggleTerminal,
            KeyAction::HistorySearch,
            KeyAction::HistoryPrevious,
            KeyAction::HistoryNext,
            KeyAction::ChatSubmit,
            KeyAction::ChatCancel,
            KeyAction::ChatNewline,
            KeyAction::ChatExternalEditor,
            KeyAction::ChatCycleMode,
            KeyAction::ChatModelPicker,
            KeyAction::ChatFastMode,
            KeyAction::ChatThinkingToggle,
            KeyAction::ChatUndo,
            KeyAction::ChatStash,
            KeyAction::ChatImagePaste,
            KeyAction::AutocompleteAccept,
            KeyAction::AutocompleteDismiss,
            KeyAction::AutocompletePrevious,
            KeyAction::AutocompleteNext,
            KeyAction::ConfirmYes,
            KeyAction::ConfirmNo,
            KeyAction::ConfirmPrevious,
            KeyAction::ConfirmNext,
            KeyAction::ConfirmToggle,
            KeyAction::ConfirmCycleMode,
            KeyAction::ScrollUp,
            KeyAction::ScrollDown,
            KeyAction::ScrollPageUp,
            KeyAction::ScrollPageDown,
            KeyAction::ScrollToTop,
            KeyAction::ScrollToBottom,
            KeyAction::TabsNext,
            KeyAction::TabsPrevious,
            KeyAction::TranscriptToggleShowAll,
            KeyAction::TranscriptExit,
            KeyAction::HistorySearchNext,
            KeyAction::HistorySearchAccept,
            KeyAction::HistorySearchCancel,
            KeyAction::HistorySearchExecute,
            KeyAction::TaskBackground,
            KeyAction::HelpDismiss,
            KeyAction::VimInsertMode,
            KeyAction::VimNormalMode,
            KeyAction::VimVisualMode,
            KeyAction::VimCommandMode,
            KeyAction::VimSearchMode,
        ]
    }

    /// Human-readable name for the action.
    pub fn name(self) -> &'static str {
        match self {
            Self::AppInterrupt => "app:interrupt",
            Self::AppExit => "app:exit",
            Self::AppRedraw => "app:redraw",
            Self::ToggleTodos => "app:toggleTodos",
            Self::ToggleTranscript => "app:toggleTranscript",
            Self::ToggleTeammatePreview => "app:toggleTeammatePreview",
            Self::ToggleTerminal => "app:toggleTerminal",
            Self::HistorySearch => "history:search",
            Self::HistoryPrevious => "history:previous",
            Self::HistoryNext => "history:next",
            Self::ChatSubmit => "chat:submit",
            Self::ChatCancel => "chat:cancel",
            Self::ChatNewline => "chat:newline",
            Self::ChatExternalEditor => "chat:externalEditor",
            Self::ChatCycleMode => "chat:cycleMode",
            Self::ChatModelPicker => "chat:modelPicker",
            Self::ChatFastMode => "chat:fastMode",
            Self::ChatThinkingToggle => "chat:thinkingToggle",
            Self::ChatUndo => "chat:undo",
            Self::ChatStash => "chat:stash",
            Self::ChatImagePaste => "chat:imagePaste",
            Self::AutocompleteAccept => "autocomplete:accept",
            Self::AutocompleteDismiss => "autocomplete:dismiss",
            Self::AutocompletePrevious => "autocomplete:previous",
            Self::AutocompleteNext => "autocomplete:next",
            Self::ConfirmYes => "confirm:yes",
            Self::ConfirmNo => "confirm:no",
            Self::ConfirmPrevious => "confirm:previous",
            Self::ConfirmNext => "confirm:next",
            Self::ConfirmToggle => "confirm:toggle",
            Self::ConfirmCycleMode => "confirm:cycleMode",
            Self::ScrollUp => "scroll:up",
            Self::ScrollDown => "scroll:down",
            Self::ScrollPageUp => "scroll:pageUp",
            Self::ScrollPageDown => "scroll:pageDown",
            Self::ScrollToTop => "scroll:toTop",
            Self::ScrollToBottom => "scroll:toBottom",
            Self::TabsNext => "tabs:next",
            Self::TabsPrevious => "tabs:previous",
            Self::TranscriptToggleShowAll => "transcript:toggleShowAll",
            Self::TranscriptExit => "transcript:exit",
            Self::HistorySearchNext => "historySearch:next",
            Self::HistorySearchAccept => "historySearch:accept",
            Self::HistorySearchCancel => "historySearch:cancel",
            Self::HistorySearchExecute => "historySearch:execute",
            Self::TaskBackground => "task:background",
            Self::HelpDismiss => "help:dismiss",
            Self::VimInsertMode => "vim:insertMode",
            Self::VimNormalMode => "vim:normalMode",
            Self::VimVisualMode => "vim:visualMode",
            Self::VimCommandMode => "vim:commandMode",
            Self::VimSearchMode => "vim:searchMode",
        }
    }
}

impl fmt::Display for KeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// KeyBinding
// ---------------------------------------------------------------------------

/// A single keybinding definition.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    /// The keystroke that triggers this binding.
    pub keystroke: Keystroke,
    /// The action to perform.
    pub action: KeyAction,
    /// The context where this binding applies.
    pub context: KeyBindingContext,
}

impl KeyBinding {
    /// Create a new keybinding.
    pub fn new(keystroke: Keystroke, action: KeyAction, context: KeyBindingContext) -> Self {
        KeyBinding {
            keystroke,
            action,
            context,
        }
    }

    /// Create from string representations.
    pub fn from_parts(
        key_str: &str,
        action: KeyAction,
        context: KeyBindingContext,
    ) -> Result<Self> {
        let keystroke = Keystroke::parse(key_str)?;
        Ok(KeyBinding::new(keystroke, action, context))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of keybindings with conflict detection.
#[derive(Debug, Clone)]
pub struct KeyBindingRegistry {
    /// Bindings organized by context.
    bindings: HashMap<KeyBindingContext, Vec<KeyBinding>>,
    /// Quick lookup: (context, key_hash) -> action.
    lookup: HashMap<(KeyBindingContext, Keystroke), KeyAction>,
}

impl KeyBindingRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        KeyBindingRegistry {
            bindings: HashMap::new(),
            lookup: HashMap::new(),
        }
    }

    /// Register a keybinding. Returns `Err` if it conflicts with an existing binding.
    pub fn register(&mut self, binding: KeyBinding) -> Result<()> {
        let context = binding.context;
        let keystroke = binding.keystroke.clone();
        let action = binding.action;

        // Check for conflicts within the same context.
        if let Some(existing) = self.lookup.get(&(context, keystroke.clone())) {
            if *existing != action {
                return Err(anyhow::anyhow!(
                    "conflict in context {}: '{}' already bound to {} (tried to bind to {})",
                    context,
                    keystroke.display(),
                    existing.name(),
                    action.name(),
                ));
            }
            // Same action — idempotent, no error.
            return Ok(());
        }

        self.bindings.entry(context).or_default().push(binding);
        self.lookup.insert((context, keystroke), action);
        Ok(())
    }

    /// Unregister a keybinding by context and keystroke.
    pub fn unregister(&mut self, context: KeyBindingContext, keystroke: &Keystroke) {
        self.lookup.remove(&(context, keystroke.clone()));
        if let Some(bindings) = self.bindings.get_mut(&context) {
            bindings.retain(|b| b.keystroke != *keystroke);
        }
    }

    /// Look up the action for a keystroke in a given context.
    /// Falls back to Global context if no context-specific binding is found.
    pub fn resolve(&self, context: KeyBindingContext, keystroke: &Keystroke) -> Option<KeyAction> {
        // Try the specific context first.
        if let Some(action) = self.lookup.get(&(context, keystroke.clone())) {
            return Some(*action);
        }
        // Fall back to Global.
        if context != KeyBindingContext::Global
            && let Some(action) = self
                .lookup
                .get(&(KeyBindingContext::Global, keystroke.clone()))
        {
            return Some(*action);
        }
        None
    }

    /// Find all conflicts across the registry.
    pub fn find_conflicts(&self) -> Vec<ConflictInfo> {
        let mut conflicts: Vec<ConflictInfo> = Vec::new();
        let all_contexts = KeyBindingContext::all();

        // Check for same keystroke in overlapping contexts.
        for i in 0..all_contexts.len() {
            for j in (i + 1)..all_contexts.len() {
                let ctx_a = all_contexts[i];
                let ctx_b = all_contexts[j];

                let bindings_a = self.bindings.get(&ctx_a);
                let bindings_b = self.bindings.get(&ctx_b);

                let Some(bindings_a) = bindings_a else {
                    continue;
                };
                let Some(bindings_b) = bindings_b else {
                    continue;
                };

                for ba in bindings_a {
                    for bb in bindings_b {
                        if ba.keystroke == bb.keystroke && ba.action != bb.action {
                            // Skip Global vs specific — that's intentional fallback.
                            if ctx_a == KeyBindingContext::Global
                                || ctx_b == KeyBindingContext::Global
                            {
                                continue;
                            }
                            conflicts.push(ConflictInfo {
                                keystroke: ba.keystroke.clone(),
                                context_a: ctx_a,
                                action_a: ba.action,
                                context_b: ctx_b,
                                action_b: bb.action,
                            });
                        }
                    }
                }
            }
        }

        conflicts
    }

    /// Get all bindings for a specific context.
    pub fn bindings_for_context(&self, context: KeyBindingContext) -> &[KeyBinding] {
        self.bindings.get(&context).map_or(&[], Vec::as_slice)
    }

    /// Get the total number of registered bindings.
    pub fn len(&self) -> usize {
        self.lookup.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.lookup.is_empty()
    }

    /// Get the shortcut display string for an action in a context.
    pub fn shortcut_for(&self, context: KeyBindingContext, action: KeyAction) -> Option<String> {
        if let Some(bindings) = self.bindings.get(&context) {
            for b in bindings {
                if b.action == action {
                    return Some(b.keystroke.display());
                }
            }
        }
        // Fall back to Global.
        if context != KeyBindingContext::Global
            && let Some(bindings) = self.bindings.get(&KeyBindingContext::Global)
        {
            for b in bindings {
                if b.action == action {
                    return Some(b.keystroke.display());
                }
            }
        }
        None
    }
}

impl Default for KeyBindingRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        register_defaults(&mut registry);
        registry
    }
}

/// Information about a keybinding conflict.
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    /// The conflicting keystroke.
    pub keystroke: Keystroke,
    /// First context.
    pub context_a: KeyBindingContext,
    /// Action in the first context.
    pub action_a: KeyAction,
    /// Second context.
    pub context_b: KeyBindingContext,
    /// Action in the second context.
    pub action_b: KeyAction,
}

// ---------------------------------------------------------------------------
// Default bindings
// ---------------------------------------------------------------------------

/// Register the default keybindings matching Claude Code conventions.
pub fn register_defaults(registry: &mut KeyBindingRegistry) {
    let defaults: Vec<(&str, KeyAction, KeyBindingContext)> = vec![
        // Global
        ("ctrl+c", KeyAction::AppInterrupt, KeyBindingContext::Global),
        ("ctrl+d", KeyAction::AppExit, KeyBindingContext::Global),
        ("ctrl+l", KeyAction::AppRedraw, KeyBindingContext::Global),
        ("ctrl+t", KeyAction::ToggleTodos, KeyBindingContext::Global),
        (
            "ctrl+o",
            KeyAction::ToggleTranscript,
            KeyBindingContext::Global,
        ),
        (
            "ctrl+r",
            KeyAction::HistorySearch,
            KeyBindingContext::Global,
        ),
        // Chat
        ("enter", KeyAction::ChatSubmit, KeyBindingContext::Chat),
        ("escape", KeyAction::ChatCancel, KeyBindingContext::Chat),
        ("up", KeyAction::HistoryPrevious, KeyBindingContext::Chat),
        ("down", KeyAction::HistoryNext, KeyBindingContext::Chat),
        (
            "ctrl+g",
            KeyAction::ChatExternalEditor,
            KeyBindingContext::Chat,
        ),
        ("ctrl+s", KeyAction::ChatStash, KeyBindingContext::Chat),
        // Autocomplete
        (
            "tab",
            KeyAction::AutocompleteAccept,
            KeyBindingContext::Autocomplete,
        ),
        (
            "escape",
            KeyAction::AutocompleteDismiss,
            KeyBindingContext::Autocomplete,
        ),
        (
            "up",
            KeyAction::AutocompletePrevious,
            KeyBindingContext::Autocomplete,
        ),
        (
            "down",
            KeyAction::AutocompleteNext,
            KeyBindingContext::Autocomplete,
        ),
        // Confirmation
        ("y", KeyAction::ConfirmYes, KeyBindingContext::Confirmation),
        ("n", KeyAction::ConfirmNo, KeyBindingContext::Confirmation),
        (
            "enter",
            KeyAction::ConfirmYes,
            KeyBindingContext::Confirmation,
        ),
        (
            "escape",
            KeyAction::ConfirmNo,
            KeyBindingContext::Confirmation,
        ),
        // Tabs
        ("tab", KeyAction::TabsNext, KeyBindingContext::Tabs),
        (
            "shift+tab",
            KeyAction::TabsPrevious,
            KeyBindingContext::Tabs,
        ),
        // Transcript
        (
            "escape",
            KeyAction::TranscriptExit,
            KeyBindingContext::Transcript,
        ),
        (
            "q",
            KeyAction::TranscriptExit,
            KeyBindingContext::Transcript,
        ),
        // HistorySearch
        (
            "ctrl+r",
            KeyAction::HistorySearchNext,
            KeyBindingContext::HistorySearch,
        ),
        (
            "escape",
            KeyAction::HistorySearchAccept,
            KeyBindingContext::HistorySearch,
        ),
        (
            "tab",
            KeyAction::HistorySearchAccept,
            KeyBindingContext::HistorySearch,
        ),
        (
            "ctrl+c",
            KeyAction::HistorySearchCancel,
            KeyBindingContext::HistorySearch,
        ),
        (
            "enter",
            KeyAction::HistorySearchExecute,
            KeyBindingContext::HistorySearch,
        ),
        // Task
        ("ctrl+b", KeyAction::TaskBackground, KeyBindingContext::Task),
        // Help
        ("escape", KeyAction::HelpDismiss, KeyBindingContext::Help),
        // Vim Normal mode
        ("i", KeyAction::VimInsertMode, KeyBindingContext::Normal),
        ("v", KeyAction::VimVisualMode, KeyBindingContext::Normal),
        (":", KeyAction::VimCommandMode, KeyBindingContext::Normal),
        ("/", KeyAction::VimSearchMode, KeyBindingContext::Normal),
        ("k", KeyAction::ScrollUp, KeyBindingContext::Normal),
        ("j", KeyAction::ScrollDown, KeyBindingContext::Normal),
        // Vim Insert mode
        (
            "escape",
            KeyAction::VimNormalMode,
            KeyBindingContext::Insert,
        ),
    ];

    for (key_str, action, context) in defaults {
        if let Ok(binding) = KeyBinding::from_parts(key_str, action, context) {
            // Best-effort: ignore errors for defaults (shouldn't happen).
            let _ = registry.register(binding);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keystroke_parse_simple() {
        let ks = Keystroke::parse("enter").expect("parse enter");
        assert_eq!(ks.key, "enter");
        assert!(!ks.ctrl);
        assert!(!ks.alt);
        assert!(!ks.shift);
    }

    #[test]
    fn test_keystroke_parse_ctrl() {
        let ks = Keystroke::parse("ctrl+c").expect("parse ctrl+c");
        assert_eq!(ks.key, "c");
        assert!(ks.ctrl);
        assert!(!ks.alt);
        assert!(!ks.shift);
    }

    #[test]
    fn test_keystroke_parse_complex() {
        let ks = Keystroke::parse("ctrl+shift+tab").expect("parse ctrl+shift+tab");
        assert_eq!(ks.key, "tab");
        assert!(ks.ctrl);
        assert!(ks.shift);
        assert!(!ks.alt);
    }

    #[test]
    fn test_keystroke_parse_alt_alias() {
        let ks = Keystroke::parse("meta+p").expect("parse meta+p");
        assert_eq!(ks.key, "p");
        assert!(ks.alt);
    }

    #[test]
    fn test_keystroke_parse_empty_error() {
        assert!(Keystroke::parse("").is_err());
        assert!(Keystroke::parse("ctrl+").is_err());
    }

    #[test]
    fn test_keystroke_display() {
        let ks = Keystroke::parse("ctrl+c").expect("parse");
        assert_eq!(ks.display(), "Ctrl+C");
        let ks2 = Keystroke::parse("shift+tab").expect("parse");
        assert_eq!(ks2.display(), "Shift+Tab");
    }

    #[test]
    fn test_keystroke_equality() {
        let a = Keystroke::parse("ctrl+c").expect("a");
        let b = Keystroke::parse("ctrl+c").expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn test_context_descriptions() {
        for ctx in KeyBindingContext::all() {
            let desc = ctx.description();
            assert!(!desc.is_empty(), "context {ctx} has no description");
        }
    }

    #[test]
    fn test_context_display() {
        assert_eq!(KeyBindingContext::Global.to_string(), "Global");
        assert_eq!(KeyBindingContext::Chat.to_string(), "Chat");
        assert_eq!(
            KeyBindingContext::HistorySearch.to_string(),
            "HistorySearch"
        );
    }

    #[test]
    fn test_action_names() {
        assert_eq!(KeyAction::AppInterrupt.name(), "app:interrupt");
        assert_eq!(KeyAction::ChatSubmit.name(), "chat:submit");
        assert_eq!(KeyAction::ScrollUp.name(), "scroll:up");
    }

    #[test]
    fn test_registry_register_and_resolve() {
        let mut registry = KeyBindingRegistry::new();
        let binding =
            KeyBinding::from_parts("ctrl+c", KeyAction::AppInterrupt, KeyBindingContext::Global)
                .expect("binding");
        registry.register(binding).expect("register");

        let ks = Keystroke::parse("ctrl+c").expect("keystroke");
        let action = registry.resolve(KeyBindingContext::Global, &ks);
        assert_eq!(action, Some(KeyAction::AppInterrupt));
    }

    #[test]
    fn test_registry_conflict_detection() {
        let mut registry = KeyBindingRegistry::new();
        let b1 = KeyBinding::from_parts("a", KeyAction::ChatSubmit, KeyBindingContext::Chat)
            .expect("b1");
        let b2 = KeyBinding::from_parts("a", KeyAction::ChatCancel, KeyBindingContext::Chat)
            .expect("b2");

        registry.register(b1).expect("register b1");
        let result = registry.register(b2);
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_fallback_to_global() {
        let mut registry = KeyBindingRegistry::new();
        let binding =
            KeyBinding::from_parts("ctrl+l", KeyAction::AppRedraw, KeyBindingContext::Global)
                .expect("binding");
        registry.register(binding).expect("register");

        let ks = Keystroke::parse("ctrl+l").expect("keystroke");
        // Should resolve from Chat context via Global fallback.
        let action = registry.resolve(KeyBindingContext::Chat, &ks);
        assert_eq!(action, Some(KeyAction::AppRedraw));
    }

    #[test]
    fn test_registry_unregister() {
        let mut registry = KeyBindingRegistry::new();
        let binding =
            KeyBinding::from_parts("ctrl+c", KeyAction::AppInterrupt, KeyBindingContext::Global)
                .expect("binding");
        registry.register(binding).expect("register");

        let ks = Keystroke::parse("ctrl+c").expect("keystroke");
        registry.unregister(KeyBindingContext::Global, &ks);
        assert!(registry.is_empty());
        assert_eq!(registry.resolve(KeyBindingContext::Global, &ks), None);
    }

    #[test]
    fn test_default_registry() {
        let registry = KeyBindingRegistry::default();
        assert!(!registry.is_empty());

        // Ctrl+C should be bound globally.
        let ks = Keystroke::parse("ctrl+c").expect("keystroke");
        assert_eq!(
            registry.resolve(KeyBindingContext::Global, &ks),
            Some(KeyAction::AppInterrupt),
        );
    }

    #[test]
    fn test_default_chat_bindings() {
        let registry = KeyBindingRegistry::default();
        let enter = Keystroke::parse("enter").expect("keystroke");
        assert_eq!(
            registry.resolve(KeyBindingContext::Chat, &enter),
            Some(KeyAction::ChatSubmit),
        );
    }

    #[test]
    fn test_bindings_for_context() {
        let registry = KeyBindingRegistry::default();
        let chat_bindings = registry.bindings_for_context(KeyBindingContext::Chat);
        assert!(!chat_bindings.is_empty());
        // Enter should be in Chat bindings.
        assert!(
            chat_bindings
                .iter()
                .any(|b| b.action == KeyAction::ChatSubmit)
        );
    }

    #[test]
    fn test_find_conflicts_none_for_custom() {
        let mut registry = KeyBindingRegistry::new();
        let b1 = KeyBinding::from_parts("ctrl+a", KeyAction::ScrollUp, KeyBindingContext::Chat)
            .expect("b1");
        let b2 = KeyBinding::from_parts("ctrl+b", KeyAction::ScrollDown, KeyBindingContext::Chat)
            .expect("b2");
        registry.register(b1).expect("register b1");
        registry.register(b2).expect("register b2");
        let conflicts = registry.find_conflicts();
        assert!(conflicts.is_empty(), "unexpected conflicts: {conflicts:?}");
    }

    #[test]
    fn test_find_conflicts_detects_real() {
        let mut registry = KeyBindingRegistry::new();
        let b1 = KeyBinding::from_parts("a", KeyAction::ChatSubmit, KeyBindingContext::Chat)
            .expect("b1");
        let b2 =
            KeyBinding::from_parts("a", KeyAction::ChatCancel, KeyBindingContext::Autocomplete)
                .expect("b2");
        registry.register(b1).expect("register b1");
        registry.register(b2).expect("register b2");
        let conflicts = registry.find_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].keystroke.key, "a");
    }

    #[test]
    fn test_shortcut_for() {
        let registry = KeyBindingRegistry::default();
        let shortcut = registry.shortcut_for(KeyBindingContext::Global, KeyAction::AppInterrupt);
        assert!(shortcut.is_some());
        assert_eq!(shortcut.expect("shortcut"), "Ctrl+C");
    }

    #[test]
    fn test_shortcut_for_missing() {
        let registry = KeyBindingRegistry::new();
        let shortcut = registry.shortcut_for(KeyBindingContext::Chat, KeyAction::ChatSubmit);
        assert!(shortcut.is_none());
    }

    #[test]
    fn test_registry_len() {
        let mut registry = KeyBindingRegistry::new();
        assert_eq!(registry.len(), 0);
        let b =
            KeyBinding::from_parts("a", KeyAction::ChatSubmit, KeyBindingContext::Chat).expect("b");
        registry.register(b).expect("register");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_idempotent_register() {
        let mut registry = KeyBindingRegistry::new();
        let b1 = KeyBinding::from_parts("a", KeyAction::ChatSubmit, KeyBindingContext::Chat)
            .expect("b1");
        let b2 = KeyBinding::from_parts("a", KeyAction::ChatSubmit, KeyBindingContext::Chat)
            .expect("b2");
        registry.register(b1).expect("register b1");
        registry.register(b2).expect("register b2 (idempotent)");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_keystroke_case_insensitive() {
        let a = Keystroke::parse("Ctrl+C").expect("a");
        let b = Keystroke::parse("ctrl+c").expect("b");
        assert_eq!(a, b);
    }
}
