//! Sandbox panel component for the TUI.
//!
//! Provides rendering for sandbox configuration and management views.
//! Mirrors `cc-haha/src/components/sandbox/`.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`SandboxConfig`] | Sandbox configuration |
//! | [`SandboxStatus`] | Sandbox running status |
//! | [`SandboxPanel`] | Sandbox panel state |
//! | [`render_sandbox_panel`] | Render the sandbox panel |
//! | [`render_sandbox_doctor`] | Render sandbox diagnostics |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// SandboxConfig
// ---------------------------------------------------------------------------

/// Sandbox configuration.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Whether sandboxing is enabled.
    pub enabled: bool,
    /// Sandbox type (e.g., "docker", "nsjail", "bubblewrap").
    pub sandbox_type: String,
    /// Allowed network access.
    pub network_allowed: bool,
    /// Memory limit in MB.
    pub memory_limit_mb: Option<u32>,
    /// CPU time limit in seconds.
    pub cpu_time_limit_s: Option<u32>,
    /// List of writable paths.
    pub writable_paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// SandboxStatus
// ---------------------------------------------------------------------------

/// Status of the sandbox environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxStatus {
    /// Sandbox is not configured.
    NotConfigured,
    /// Sandbox is configured but not running.
    Configured,
    /// Sandbox is running and ready.
    Running,
    /// Sandbox has an error.
    Error(String),
}

impl SandboxStatus {
    /// Returns the indicator character.
    pub fn indicator(&self) -> &'static str {
        match self {
            SandboxStatus::NotConfigured => "○",
            SandboxStatus::Configured => "◐",
            SandboxStatus::Running => "●",
            SandboxStatus::Error(_) => "✗",
        }
    }

    /// Returns the color.
    pub fn color(&self) -> Color {
        match self {
            SandboxStatus::NotConfigured => Color::DarkGray,
            SandboxStatus::Configured => Color::Yellow,
            SandboxStatus::Running => Color::Green,
            SandboxStatus::Error(_) => Color::Red,
        }
    }
}

// ---------------------------------------------------------------------------
// SandboxPanel
// ---------------------------------------------------------------------------

/// Sandbox panel state.
#[derive(Debug, Clone)]
pub struct SandboxPanel {
    /// Current configuration.
    pub config: SandboxConfig,
    /// Current status.
    pub status: SandboxStatus,
    /// Doctor check results.
    pub doctor_checks: Vec<DoctorCheck>,
}

/// A single doctor check result.
#[derive(Debug, Clone)]
pub struct DoctorCheck {
    /// Check name.
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Optional message.
    pub message: Option<String>,
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

/// Render the sandbox panel.
pub fn render_sandbox_panel(panel: &SandboxPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Sandbox Settings", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    // Status
    let status_color = panel.status.color();
    lines.push(Line::from(vec![
        Span::styled(
            "  Status: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            panel.status.indicator().to_owned(),
            Style::default().fg(status_color),
        ),
        Span::styled(" ".to_owned(), Style::default()),
        Span::styled(
            match &panel.status {
                SandboxStatus::NotConfigured => "Not configured".to_owned(),
                SandboxStatus::Configured => "Configured".to_owned(),
                SandboxStatus::Running => "Running".to_owned(),
                SandboxStatus::Error(e) => format!("Error: {e}"),
            },
            Style::default().fg(status_color),
        ),
    ]));

    lines.push(Line::from(""));

    // Configuration
    lines.push(Line::from(Span::styled(
        "  Configuration:".to_owned(),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    lines.push(Line::from(vec![
        Span::styled(
            "  Enabled: ".to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            if panel.config.enabled { "yes" } else { "no" }.to_owned(),
            if panel.config.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            },
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Type: ".to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(panel.config.sandbox_type.clone()),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Network: ".to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            if panel.config.network_allowed {
                "allowed"
            } else {
                "blocked"
            }
            .to_owned(),
            if panel.config.network_allowed {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
    ]));

    if let Some(mem) = panel.config.memory_limit_mb {
        lines.push(Line::from(vec![
            Span::styled(
                "  Memory: ".to_owned(),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw(format!("{mem}MB")),
        ]));
    }

    if let Some(cpu) = panel.config.cpu_time_limit_s {
        lines.push(Line::from(vec![
            Span::styled(
                "  CPU time: ".to_owned(),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw(format!("{cpu}s")),
        ]));
    }

    if !panel.config.writable_paths.is_empty() {
        lines.push(Line::from(dim_span(&format!(
            "  Writable paths: {}",
            panel.config.writable_paths.join(", ")
        ))));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span("   d run doctor │ e edit │ q close")));

    lines
}

/// Render sandbox doctor diagnostics.
pub fn render_sandbox_doctor(panel: &SandboxPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Sandbox Doctor", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.doctor_checks.is_empty() {
        lines.push(Line::from(dim_span("   No checks available.")));
    } else {
        for check in &panel.doctor_checks {
            let (icon, color) = if check.passed {
                ("✓", Color::Green)
            } else {
                ("✗", Color::Red)
            };

            let mut spans = vec![
                Span::styled(format!("  {icon} ").to_owned(), Style::default().fg(color)),
                Span::styled(check.name.clone(), Style::default().fg(style.status_fg)),
            ];

            if let Some(msg) = &check.message {
                spans.push(dim_span(&format!(" — {msg}")));
            }

            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span("   Esc back")));

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

    fn sample_panel() -> SandboxPanel {
        SandboxPanel {
            config: SandboxConfig {
                enabled: true,
                sandbox_type: "docker".to_owned(),
                network_allowed: false,
                memory_limit_mb: Some(512),
                cpu_time_limit_s: Some(30),
                writable_paths: vec!["/tmp".to_owned(), "/project".to_owned()],
            },
            status: SandboxStatus::Running,
            doctor_checks: vec![
                DoctorCheck {
                    name: "Docker installed".to_owned(),
                    passed: true,
                    message: None,
                },
                DoctorCheck {
                    name: "Docker daemon running".to_owned(),
                    passed: true,
                    message: None,
                },
                DoctorCheck {
                    name: "Network isolation".to_owned(),
                    passed: false,
                    message: Some("iptables not available".to_owned()),
                },
            ],
        }
    }

    #[test]
    fn sandbox_status_indicator() {
        assert_eq!(SandboxStatus::NotConfigured.indicator(), "○");
        assert_eq!(SandboxStatus::Running.indicator(), "●");
        assert_eq!(SandboxStatus::Error("x".to_owned()).indicator(), "✗");
    }

    #[test]
    fn sandbox_status_color() {
        assert_eq!(SandboxStatus::Running.color(), Color::Green);
        assert_eq!(SandboxStatus::Error("x".to_owned()).color(), Color::Red);
    }

    #[test]
    fn render_panel_shows_status() {
        let panel = sample_panel();
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Running"));
        assert!(combined.contains("docker"));
    }

    #[test]
    fn render_panel_shows_config() {
        let panel = sample_panel();
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("512MB"));
        assert!(combined.contains("30s"));
        assert!(combined.contains("blocked"));
    }

    #[test]
    fn render_panel_shows_paths() {
        let panel = sample_panel();
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("/tmp"));
        assert!(combined.contains("/project"));
    }

    #[test]
    fn render_panel_disabled() {
        let mut panel = sample_panel();
        panel.config.enabled = false;
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("no"));
    }

    #[test]
    fn render_panel_error_status() {
        let mut panel = sample_panel();
        panel.status = SandboxStatus::Error("docker not found".to_owned());
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Error"));
        assert!(combined.contains("docker not found"));
    }

    #[test]
    fn render_doctor_shows_checks() {
        let panel = sample_panel();
        let lines = render_sandbox_doctor(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Docker installed"));
        assert!(combined.contains("Docker daemon"));
        assert!(combined.contains("Network isolation"));
        assert!(combined.contains("iptables"));
    }

    #[test]
    fn render_doctor_empty() {
        let mut panel = sample_panel();
        panel.doctor_checks = vec![];
        let lines = render_sandbox_doctor(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No checks"));
    }

    #[test]
    fn render_panel_no_limits() {
        let mut panel = sample_panel();
        panel.config.memory_limit_mb = None;
        panel.config.cpu_time_limit_s = None;
        let lines = render_sandbox_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(!combined.contains("Memory:"));
        assert!(!combined.contains("CPU time:"));
    }
}
