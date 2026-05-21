//! Color theme system for the TUI.

/// Color theme for the TUI.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Theme {
    pub name: String,
    pub prompt_color: crossterm::style::Color,
    pub system_color: crossterm::style::Color,
    pub assistant_color: crossterm::style::Color,
    pub tool_color: crossterm::style::Color,
    pub error_color: crossterm::style::Color,
    pub info_color: crossterm::style::Color,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            name: "dark".to_owned(),
            prompt_color: crossterm::style::Color::Green,
            system_color: crossterm::style::Color::Magenta,
            assistant_color: crossterm::style::Color::Cyan,
            tool_color: crossterm::style::Color::Yellow,
            error_color: crossterm::style::Color::Red,
            info_color: crossterm::style::Color::DarkGrey,
        }
    }

    pub fn light() -> Self {
        Theme {
            name: "light".to_owned(),
            prompt_color: crossterm::style::Color::DarkGreen,
            system_color: crossterm::style::Color::DarkMagenta,
            assistant_color: crossterm::style::Color::DarkCyan,
            tool_color: crossterm::style::Color::DarkYellow,
            error_color: crossterm::style::Color::DarkRed,
            info_color: crossterm::style::Color::Grey,
        }
    }

    pub fn monokai() -> Self {
        Theme {
            name: "monokai".to_owned(),
            prompt_color: crossterm::style::Color::Green,
            system_color: crossterm::style::Color::Magenta,
            assistant_color: crossterm::style::Color::Yellow,
            tool_color: crossterm::style::Color::DarkCyan,
            error_color: crossterm::style::Color::Red,
            info_color: crossterm::style::Color::DarkGrey,
        }
    }

    pub fn solarized() -> Self {
        Theme {
            name: "solarized".to_owned(),
            prompt_color: crossterm::style::Color::Green,
            system_color: crossterm::style::Color::Magenta,
            assistant_color: crossterm::style::Color::Blue,
            tool_color: crossterm::style::Color::Yellow,
            error_color: crossterm::style::Color::Red,
            info_color: crossterm::style::Color::DarkCyan,
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "monokai" => Some(Self::monokai()),
            "solarized" => Some(Self::solarized()),
            _ => None,
        }
    }

    pub fn all_names() -> Vec<&'static str> {
        vec!["dark", "light", "monokai", "solarized"]
    }
}
