//! Terminal environment detection.
//!
//! Provides functions to detect the current terminal environment,
//! including whether we're inside tmux, iTerm2, or a plain terminal.

use crate::types::BackendType;

/// Information about the detected terminal environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEnvironment {
    /// Whether we're inside a tmux session.
    pub is_tmux: bool,
    /// Whether we're inside iTerm2.
    pub is_iterm2: bool,
    /// Whether we're inside Windows Terminal.
    pub is_windows_terminal: bool,
    /// Whether we're inside VS Code's integrated terminal.
    pub is_vscode: bool,
    /// The TERM_PROGRAM environment variable.
    pub term_program: Option<String>,
    /// The TERM environment variable.
    pub term: Option<String>,
}

impl TerminalEnvironment {
    /// Detect the current terminal environment.
    pub fn detect() -> Self {
        Self {
            is_tmux: std::env::var("TMUX").is_ok(),
            is_iterm2: std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app"),
            is_windows_terminal: std::env::var("WT_SESSION").is_ok(),
            is_vscode: std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "vscode"),
            term_program: std::env::var("TERM_PROGRAM").ok(),
            term: std::env::var("TERM").ok(),
        }
    }

    /// Recommend the best backend based on the detected environment.
    #[must_use]
    pub fn recommended_backend(&self) -> BackendType {
        if self.is_tmux {
            BackendType::Tmux
        } else if self.is_iterm2 {
            BackendType::ITerm2
        } else {
            BackendType::InProcess
        }
    }

    /// Get a human-readable description of the environment.
    #[must_use]
    pub fn description(&self) -> String {
        if self.is_tmux {
            "tmux session".to_owned()
        } else if self.is_iterm2 {
            "iTerm2".to_owned()
        } else if self.is_vscode {
            "VS Code integrated terminal".to_owned()
        } else if self.is_windows_terminal {
            "Windows Terminal".to_owned()
        } else {
            "plain terminal".to_owned()
        }
    }
}

/// Quick check: are we inside tmux?
#[must_use]
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Quick check: are we inside iTerm2?
#[must_use]
pub fn is_inside_iterm2() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app")
}

/// Quick check: are we inside VS Code terminal?
#[must_use]
pub fn is_inside_vscode() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "vscode")
}

/// Quick check: are we inside Windows Terminal?
#[must_use]
pub fn is_inside_windows_terminal() -> bool {
    std::env::var("WT_SESSION").is_ok()
}

/// Detect the best backend for the current environment.
#[must_use]
pub fn detect_best_backend() -> BackendType {
    let env = TerminalEnvironment::detect();
    env.recommended_backend()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_environment_detect() {
        let env = TerminalEnvironment::detect();
        // Should not panic.
        let _ = env.is_tmux;
        let _ = env.is_iterm2;
    }

    #[test]
    fn terminal_environment_description() {
        let env = TerminalEnvironment::detect();
        let desc = env.description();
        assert!(!desc.is_empty());
    }

    #[test]
    fn terminal_environment_recommended_backend() {
        let env = TerminalEnvironment::detect();
        let backend = env.recommended_backend();
        assert!(matches!(
            backend,
            BackendType::InProcess | BackendType::Tmux | BackendType::ITerm2
        ));
    }

    #[test]
    fn is_inside_tmux_does_not_panic() {
        let _ = is_inside_tmux();
    }

    #[test]
    fn is_inside_iterm2_does_not_panic() {
        let _ = is_inside_iterm2();
    }

    #[test]
    fn is_inside_vscode_does_not_panic() {
        let _ = is_inside_vscode();
    }

    #[test]
    fn is_inside_windows_terminal_does_not_panic() {
        let _ = is_inside_windows_terminal();
    }

    #[test]
    fn detect_best_backend_returns_valid() {
        let backend = detect_best_backend();
        assert!(matches!(
            backend,
            BackendType::InProcess | BackendType::Tmux | BackendType::ITerm2
        ));
    }

    #[test]
    fn terminal_environment_tmux_recommendation() {
        let env = TerminalEnvironment {
            is_tmux: true,
            is_iterm2: false,
            is_windows_terminal: false,
            is_vscode: false,
            term_program: Some("tmux".to_owned()),
            term: Some("screen-256color".to_owned()),
        };
        assert_eq!(env.recommended_backend(), BackendType::Tmux);
        assert_eq!(env.description(), "tmux session");
    }

    #[test]
    fn terminal_environment_iterm2_recommendation() {
        let env = TerminalEnvironment {
            is_tmux: false,
            is_iterm2: true,
            is_windows_terminal: false,
            is_vscode: false,
            term_program: Some("iTerm.app".to_owned()),
            term: Some("xterm-256color".to_owned()),
        };
        assert_eq!(env.recommended_backend(), BackendType::ITerm2);
        assert_eq!(env.description(), "iTerm2");
    }

    #[test]
    fn terminal_environment_plain_recommendation() {
        let env = TerminalEnvironment {
            is_tmux: false,
            is_iterm2: false,
            is_windows_terminal: false,
            is_vscode: false,
            term_program: None,
            term: Some("xterm".to_owned()),
        };
        assert_eq!(env.recommended_backend(), BackendType::InProcess);
        assert_eq!(env.description(), "plain terminal");
    }

    #[test]
    fn terminal_environment_vscode_description() {
        let env = TerminalEnvironment {
            is_tmux: false,
            is_iterm2: false,
            is_windows_terminal: false,
            is_vscode: true,
            term_program: Some("vscode".to_owned()),
            term: Some("xterm-256color".to_owned()),
        };
        assert_eq!(env.description(), "VS Code integrated terminal");
    }

    #[test]
    fn terminal_environment_windows_terminal_description() {
        let env = TerminalEnvironment {
            is_tmux: false,
            is_iterm2: false,
            is_windows_terminal: true,
            is_vscode: false,
            term_program: None,
            term: None,
        };
        assert_eq!(env.description(), "Windows Terminal");
    }
}
