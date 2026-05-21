//! Style system for ratatui-based TUI rendering.
//!
//! Provides color themes and style presets using `ratatui::style::Color`,
//! independent of the legacy `theme::Theme` (which uses crossterm colors
//! for the command subsystem).

use ratatui::style::Color;

/// Complete style configuration for the TUI.
#[derive(Debug, Clone)]
pub struct StyleConfig {
    /// Theme name.
    pub name: String,
    /// User message text color.
    pub user_color: Color,
    /// Assistant message text color.
    pub assistant_color: Color,
    /// System message text color.
    pub system_color: Color,
    /// Tool output text color.
    pub tool_color: Color,
    /// Error text color.
    pub error_color: Color,
    /// Info / dimmed text color.
    pub info_color: Color,
    /// Accent color (highlights, borders).
    pub accent_color: Color,
    /// Status bar background.
    pub status_bg: Color,
    /// Status bar foreground.
    pub status_fg: Color,
    /// Sidebar background.
    pub sidebar_bg: Color,
    /// Sidebar border color.
    pub sidebar_border: Color,
    /// Input area border color.
    pub input_border: Color,
    /// Code block background.
    pub code_bg: Color,
    /// Code block foreground.
    pub code_fg: Color,
    /// Vim mode — Normal indicator color.
    pub mode_normal: Color,
    /// Vim mode — Insert indicator color.
    pub mode_insert: Color,
    /// Vim mode — Command indicator color.
    pub mode_command: Color,
    /// Vim mode — Visual indicator color.
    pub mode_visual: Color,
    /// Vim mode — Search indicator color.
    pub mode_search: Color,
}

impl StyleConfig {
    /// Dark theme (default).
    pub fn dark() -> Self {
        StyleConfig {
            name: "dark".to_owned(),
            user_color: Color::Green,
            assistant_color: Color::Cyan,
            system_color: Color::Magenta,
            tool_color: Color::Yellow,
            error_color: Color::Red,
            info_color: Color::DarkGray,
            accent_color: Color::Blue,
            status_bg: Color::DarkGray,
            status_fg: Color::White,
            sidebar_bg: Color::Black,
            sidebar_border: Color::DarkGray,
            input_border: Color::DarkGray,
            code_bg: Color::Black,
            code_fg: Color::Gray,
            mode_normal: Color::Green,
            mode_insert: Color::Blue,
            mode_command: Color::Yellow,
            mode_visual: Color::Magenta,
            mode_search: Color::Cyan,
        }
    }

    /// Light theme.
    #[allow(dead_code)]
    pub fn light() -> Self {
        StyleConfig {
            name: "light".to_owned(),
            user_color: Color::Green,
            assistant_color: Color::Cyan,
            system_color: Color::Magenta,
            tool_color: Color::Yellow,
            error_color: Color::Red,
            info_color: Color::Gray,
            accent_color: Color::Blue,
            status_bg: Color::White,
            status_fg: Color::Black,
            sidebar_bg: Color::Gray,
            sidebar_border: Color::DarkGray,
            input_border: Color::DarkGray,
            code_bg: Color::Gray,
            code_fg: Color::Black,
            mode_normal: Color::Green,
            mode_insert: Color::Blue,
            mode_command: Color::Yellow,
            mode_visual: Color::Magenta,
            mode_search: Color::Cyan,
        }
    }

    /// Monokai-inspired theme.
    #[allow(dead_code)]
    pub fn monokai() -> Self {
        StyleConfig {
            name: "monokai".to_owned(),
            user_color: Color::Green,
            assistant_color: Color::Yellow,
            system_color: Color::Magenta,
            tool_color: Color::Cyan,
            error_color: Color::Red,
            info_color: Color::DarkGray,
            accent_color: Color::Magenta,
            status_bg: Color::DarkGray,
            status_fg: Color::White,
            sidebar_bg: Color::Black,
            sidebar_border: Color::DarkGray,
            input_border: Color::DarkGray,
            code_bg: Color::Black,
            code_fg: Color::White,
            mode_normal: Color::Green,
            mode_insert: Color::Yellow,
            mode_command: Color::Magenta,
            mode_visual: Color::Cyan,
            mode_search: Color::Magenta,
        }
    }

    /// Solarized-inspired theme.
    #[allow(dead_code)]
    pub fn solarized() -> Self {
        StyleConfig {
            name: "solarized".to_owned(),
            user_color: Color::Green,
            assistant_color: Color::Blue,
            system_color: Color::Magenta,
            tool_color: Color::Yellow,
            error_color: Color::Red,
            info_color: Color::Cyan,
            accent_color: Color::Cyan,
            status_bg: Color::DarkGray,
            status_fg: Color::White,
            sidebar_bg: Color::Black,
            sidebar_border: Color::DarkGray,
            input_border: Color::DarkGray,
            code_bg: Color::Black,
            code_fg: Color::White,
            mode_normal: Color::Green,
            mode_insert: Color::Blue,
            mode_command: Color::Yellow,
            mode_visual: Color::Magenta,
            mode_search: Color::Cyan,
        }
    }

    /// Look up a theme by name.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "monokai" => Some(Self::monokai()),
            "solarized" => Some(Self::solarized()),
            _ => None,
        }
    }

    /// All available theme names.
    pub fn all_names() -> Vec<&'static str> {
        vec!["dark", "light", "monokai", "solarized"]
    }

    /// Return the mode indicator color for a given Vim mode label.
    pub fn mode_color(&self, mode_label: &str) -> Color {
        match mode_label {
            "NORMAL" => self.mode_normal,
            "INSERT" => self.mode_insert,
            "COMMAND" => self.mode_command,
            "VISUAL" => self.mode_visual,
            "SEARCH" => self.mode_search,
            _ => self.info_color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_has_expected_defaults() {
        let s = StyleConfig::dark();
        assert_eq!(s.name, "dark");
        assert_eq!(s.user_color, Color::Green);
        assert_eq!(s.assistant_color, Color::Cyan);
    }

    #[test]
    fn by_name_returns_correct_theme() {
        let s = StyleConfig::by_name("monokai").expect("monokai should exist");
        assert_eq!(s.name, "monokai");
        assert!(StyleConfig::by_name("nonexistent").is_none());
    }

    #[test]
    fn all_names_contains_expected_themes() {
        let names = StyleConfig::all_names();
        assert!(names.contains(&"dark"));
        assert!(names.contains(&"light"));
        assert!(names.contains(&"monokai"));
        assert!(names.contains(&"solarized"));
    }

    #[test]
    fn mode_color_returns_correct_mapping() {
        let s = StyleConfig::dark();
        assert_eq!(s.mode_color("NORMAL"), Color::Green);
        assert_eq!(s.mode_color("INSERT"), Color::Blue);
        assert_eq!(s.mode_color("COMMAND"), Color::Yellow);
        assert_eq!(s.mode_color("VISUAL"), Color::Magenta);
        assert_eq!(s.mode_color("SEARCH"), Color::Cyan);
    }

    #[test]
    fn light_theme_has_distinct_status_colors() {
        let s = StyleConfig::light();
        assert_eq!(s.status_bg, Color::White);
        assert_eq!(s.status_fg, Color::Black);
    }
}
