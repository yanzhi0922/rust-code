//! Onboarding wizard component for the TUI.
//!
//! Provides a multi-step wizard for first-time user onboarding.
//! Mirrors `cc-haha/src/components/wizard/`.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`WizardStep`] | Steps in the onboarding wizard |
//! | [`WizardState`] | Wizard state machine |
//! | [`render_wizard`] | Render the current wizard step |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// WizardStep
// ---------------------------------------------------------------------------

/// Steps in the onboarding wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Welcome screen.
    Welcome,
    /// API key configuration.
    ApiKey,
    /// Provider selection.
    Provider,
    /// Theme selection.
    Theme,
    /// Shell preference.
    Shell,
    /// Done.
    Complete,
}

impl WizardStep {
    /// All steps in order.
    pub fn all() -> &'static [WizardStep] {
        &[
            WizardStep::Welcome,
            WizardStep::ApiKey,
            WizardStep::Provider,
            WizardStep::Theme,
            WizardStep::Shell,
            WizardStep::Complete,
        ]
    }

    /// Returns the step title.
    pub fn title(&self) -> &'static str {
        match self {
            WizardStep::Welcome => "Welcome to Remote Code",
            WizardStep::ApiKey => "API Key Setup",
            WizardStep::Provider => "Provider Selection",
            WizardStep::Theme => "Theme Selection",
            WizardStep::Shell => "Shell Configuration",
            WizardStep::Complete => "All Set!",
        }
    }

    /// Returns the step index (0-based).
    pub fn index(&self) -> usize {
        Self::all().iter().position(|s| s == self).unwrap_or(0)
    }

    /// Advance to the next step.
    pub fn next(self) -> Option<WizardStep> {
        let all = Self::all();
        let idx = self.index();
        if idx + 1 < all.len() {
            Some(all[idx + 1])
        } else {
            None
        }
    }

    /// Go back to the previous step.
    pub fn prev(self) -> Option<WizardStep> {
        let idx = self.index();
        if idx > 0 {
            Some(Self::all()[idx - 1])
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// WizardState
// ---------------------------------------------------------------------------

/// Wizard state machine.
#[derive(Debug, Clone)]
pub struct WizardState {
    /// Current step.
    pub current_step: WizardStep,
    /// Total steps.
    pub total_steps: usize,
    /// API key entered by user.
    pub api_key: String,
    /// Selected provider.
    pub provider: String,
    /// Selected theme.
    pub theme: String,
    /// Selected shell.
    pub shell: String,
}

impl Default for WizardState {
    fn default() -> Self {
        Self::new()
    }
}

impl WizardState {
    /// Create a new wizard starting at the welcome step.
    pub fn new() -> Self {
        Self {
            current_step: WizardStep::Welcome,
            total_steps: WizardStep::all().len(),
            api_key: String::new(),
            provider: "anthropic".to_owned(),
            theme: "dark".to_owned(),
            shell: "bash".to_owned(),
        }
    }

    /// Advance to the next step.
    pub fn next_step(&mut self) {
        if let Some(next) = self.current_step.next() {
            self.current_step = next;
        }
    }

    /// Go back to the previous step.
    pub fn prev_step(&mut self) {
        if let Some(prev) = self.current_step.prev() {
            self.current_step = prev;
        }
    }

    /// Whether the wizard is complete.
    pub fn is_complete(&self) -> bool {
        self.current_step == WizardStep::Complete
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn dim_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::DIM),
    )
}

fn header_span(text: &str, style: &StyleConfig) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default()
            .fg(style.accent_color)
            .add_modifier(Modifier::BOLD),
    )
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render the wizard.
pub fn render_wizard(state: &WizardState, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Progress bar
    let step_idx = state.current_step.index();
    let total = state.total_steps;
    let filled = if total > 1 {
        step_idx * 10 / (total - 1)
    } else {
        10
    };
    let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);
    lines.push(Line::from(vec![
        dim_span(" Step "),
        Span::styled(
            format!("{}/{}", step_idx + 1, total),
            Style::default().fg(style.accent_color),
        ),
        dim_span(" "),
        Span::styled(bar, Style::default().fg(style.accent_color)),
    ]));
    lines.push(Line::from(""));

    // Title
    lines.push(Line::from(header_span(
        &format!(" {}", state.current_step.title()),
        style,
    )));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    // Step content
    match state.current_step {
        WizardStep::Welcome => {
            lines.push(Line::from("  Welcome! Let's set up your environment."));
            lines.push(Line::from(""));
            lines.push(Line::from(dim_span(
                "  This wizard will guide you through:",
            )));
            lines.push(Line::from(dim_span("    1. API key configuration")));
            lines.push(Line::from(dim_span("    2. Provider selection")));
            lines.push(Line::from(dim_span("    3. Theme selection")));
            lines.push(Line::from(dim_span("    4. Shell configuration")));
        }
        WizardStep::ApiKey => {
            lines.push(Line::from("  Enter your API key:"));
            lines.push(Line::from(""));
            let display = if state.api_key.is_empty() {
                "<paste your key here>".to_owned()
            } else {
                // Mask the key
                let k = &state.api_key;
                if k.len() > 8 {
                    format!("{}…{}", &k[..4], &k[k.len() - 4..])
                } else {
                    "*".repeat(k.len())
                }
            };
            lines.push(Line::from(Span::styled(
                format!("  [{display}]"),
                Style::default().fg(Color::Cyan),
            )));
        }
        WizardStep::Provider => {
            lines.push(Line::from("  Select your default provider:"));
            lines.push(Line::from(""));
            for provider in &["anthropic", "openai", "bedrock", "vertex"] {
                let is_selected = *provider == state.provider;
                let mut spans = vec![Span::styled("   ".to_owned(), Style::default())];
                if is_selected {
                    spans.push(Span::styled(
                        "● ".to_owned(),
                        Style::default().fg(Color::Green),
                    ));
                } else {
                    spans.push(Span::styled(
                        "○ ".to_owned(),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                spans.push(Span::styled(
                    (*provider).to_owned(),
                    if is_selected {
                        Style::default()
                            .fg(style.accent_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(style.status_fg)
                    },
                ));
                lines.push(Line::from(spans));
            }
        }
        WizardStep::Theme => {
            lines.push(Line::from("  Choose your theme:"));
            lines.push(Line::from(""));
            for theme in &["dark", "light", "monokai", "solarized"] {
                let is_selected = *theme == state.theme;
                let mut spans = vec![Span::styled("   ".to_owned(), Style::default())];
                if is_selected {
                    spans.push(Span::styled(
                        "● ".to_owned(),
                        Style::default().fg(Color::Green),
                    ));
                } else {
                    spans.push(Span::styled(
                        "○ ".to_owned(),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                spans.push(Span::styled(
                    (*theme).to_owned(),
                    if is_selected {
                        Style::default()
                            .fg(style.accent_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(style.status_fg)
                    },
                ));
                lines.push(Line::from(spans));
            }
        }
        WizardStep::Shell => {
            lines.push(Line::from("  Select your default shell:"));
            lines.push(Line::from(""));
            for shell in &["bash", "zsh", "fish", "powershell"] {
                let is_selected = *shell == state.shell;
                let mut spans = vec![Span::styled("   ".to_owned(), Style::default())];
                if is_selected {
                    spans.push(Span::styled(
                        "● ".to_owned(),
                        Style::default().fg(Color::Green),
                    ));
                } else {
                    spans.push(Span::styled(
                        "○ ".to_owned(),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                spans.push(Span::styled(
                    (*shell).to_owned(),
                    if is_selected {
                        Style::default()
                            .fg(style.accent_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(style.status_fg)
                    },
                ));
                lines.push(Line::from(spans));
            }
        }
        WizardStep::Complete => {
            lines.push(Line::from("  ✅ You're all set!"));
            lines.push(Line::from(""));
            lines.push(Line::from(dim_span("  Configuration:")));
            lines.push(Line::from(dim_span(&format!(
                "    Provider: {}",
                state.provider
            ))));
            lines.push(Line::from(dim_span(&format!("    Theme: {}", state.theme))));
            lines.push(Line::from(dim_span(&format!("    Shell: {}", state.shell))));
            lines.push(Line::from(dim_span(&format!(
                "    API Key: {}",
                if state.api_key.is_empty() {
                    "not set"
                } else {
                    "configured"
                }
            ))));
        }
    }

    // Navigation footer
    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   Enter next │ Esc back │ q skip wizard",
    )));

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    #[test]
    fn wizard_step_titles() {
        assert_eq!(WizardStep::Welcome.title(), "Welcome to Remote Code");
        assert_eq!(WizardStep::Complete.title(), "All Set!");
    }

    #[test]
    fn wizard_step_index() {
        assert_eq!(WizardStep::Welcome.index(), 0);
        assert_eq!(WizardStep::Complete.index(), 5);
    }

    #[test]
    fn wizard_step_next() {
        assert_eq!(WizardStep::Welcome.next(), Some(WizardStep::ApiKey));
        assert_eq!(WizardStep::Complete.next(), None);
    }

    #[test]
    fn wizard_step_prev() {
        assert_eq!(WizardStep::Complete.prev(), Some(WizardStep::Shell));
        assert_eq!(WizardStep::Welcome.prev(), None);
    }

    #[test]
    fn wizard_state_new_starts_at_welcome() {
        let state = WizardState::new();
        assert_eq!(state.current_step, WizardStep::Welcome);
        assert!(!state.is_complete());
    }

    #[test]
    fn wizard_state_next_advances() {
        let mut state = WizardState::new();
        state.next_step();
        assert_eq!(state.current_step, WizardStep::ApiKey);
    }

    #[test]
    fn wizard_state_prev_goes_back() {
        let mut state = WizardState::new();
        state.next_step();
        state.prev_step();
        assert_eq!(state.current_step, WizardStep::Welcome);
    }

    #[test]
    fn wizard_state_complete() {
        let mut state = WizardState::new();
        for _ in 0..5 {
            state.next_step();
        }
        assert!(state.is_complete());
    }

    #[test]
    fn render_welcome_shows_welcome() {
        let state = WizardState::new();
        let lines = render_wizard(&state, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Welcome"));
    }

    #[test]
    fn render_shows_progress() {
        let state = WizardState::new();
        let lines = render_wizard(&state, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Step"));
        assert!(combined.contains("1/6"));
    }

    #[test]
    fn render_api_key_step() {
        let mut state = WizardState::new();
        state.next_step(); // ApiKey
        let lines = render_wizard(&state, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("API Key"));
    }

    #[test]
    fn render_complete_step() {
        let mut state = WizardState::new();
        for _ in 0..5 {
            state.next_step();
        }
        let lines = render_wizard(&state, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("All Set"));
        assert!(combined.contains("anthropic"));
    }
}
