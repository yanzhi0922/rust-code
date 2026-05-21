//! Team panel component for the TUI.
//!
//! Provides rendering for team management views, including team status,
//! teammate views, and team creation/deletion.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`TeammateStatus`] | Status of a teammate agent |
//! | [`TeammateInfo`] | Information about a teammate |
//! | [`TeamInfo`] | Information about a team |
//! | [`TeamPanel`] | Team panel state |
//! | [`render_team_panel`] | Render the team panel |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// TeammateStatus
// ---------------------------------------------------------------------------

/// Status of a teammate agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeammateStatus {
    /// Agent is idle and waiting for tasks.
    Idle,
    /// Agent is currently working on a task.
    Working,
    /// Agent has completed its task.
    Done,
    /// Agent encountered an error.
    Failed(String),
}

impl TeammateStatus {
    /// Returns the indicator character.
    pub fn indicator(&self) -> &'static str {
        match self {
            TeammateStatus::Idle => "○",
            TeammateStatus::Working => "◐",
            TeammateStatus::Done => "●",
            TeammateStatus::Failed(_) => "✗",
        }
    }

    /// Returns the color.
    pub fn color(&self) -> Color {
        match self {
            TeammateStatus::Idle => Color::DarkGray,
            TeammateStatus::Working => Color::Yellow,
            TeammateStatus::Done => Color::Green,
            TeammateStatus::Failed(_) => Color::Red,
        }
    }
}

// ---------------------------------------------------------------------------
// TeammateInfo
// ---------------------------------------------------------------------------

/// Information about a teammate agent.
#[derive(Debug, Clone)]
pub struct TeammateInfo {
    /// Teammate name/identifier.
    pub name: String,
    /// Current status.
    pub status: TeammateStatus,
    /// Current task description (if any).
    pub current_task: Option<String>,
    /// Number of messages exchanged.
    pub message_count: usize,
}

// ---------------------------------------------------------------------------
// TeamInfo
// ---------------------------------------------------------------------------

/// Information about a team.
#[derive(Debug, Clone)]
pub struct TeamInfo {
    /// Team identifier.
    pub id: String,
    /// Team name.
    pub name: String,
    /// List of teammates.
    pub teammates: Vec<TeammateInfo>,
    /// Whether the team is active.
    pub is_active: bool,
}

impl TeamInfo {
    /// Count of working teammates.
    pub fn working_count(&self) -> usize {
        self.teammates
            .iter()
            .filter(|t| t.status == TeammateStatus::Working)
            .count()
    }

    /// Count of done teammates.
    pub fn done_count(&self) -> usize {
        self.teammates
            .iter()
            .filter(|t| t.status == TeammateStatus::Done)
            .count()
    }
}

// ---------------------------------------------------------------------------
// TeamPanel
// ---------------------------------------------------------------------------

/// Team panel view state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamView {
    /// Show the list of teams.
    List,
    /// Show detail for a specific team.
    Detail { index: usize },
}

/// Team panel state.
#[derive(Debug, Clone)]
pub struct TeamPanel {
    /// All teams.
    pub teams: Vec<TeamInfo>,
    /// Current view.
    pub view: TeamView,
    /// Currently selected index.
    pub selected: usize,
}

impl TeamPanel {
    /// Create a new team panel.
    pub fn new(teams: Vec<TeamInfo>) -> Self {
        Self {
            teams,
            view: TeamView::List,
            selected: 0,
        }
    }

    /// Enter the selected team's detail view.
    pub fn enter_team(&mut self) {
        if self.selected < self.teams.len() {
            self.view = TeamView::Detail {
                index: self.selected,
            };
        }
    }

    /// Go back to the list view.
    pub fn go_back(&mut self) {
        self.view = TeamView::List;
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.teams.len() {
            self.selected += 1;
        }
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

/// Render the team panel.
pub fn render_team_panel(panel: &TeamPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    match &panel.view {
        TeamView::List => render_team_list(panel, style),
        TeamView::Detail { index } => render_team_detail(panel, *index, style),
    }
}

/// Render the team list.
pub fn render_team_list(panel: &TeamPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Teams", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.teams.is_empty() {
        lines.push(Line::from(dim_span("   No active teams.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(
            "   Use /team create to start a new team.",
        )));
    } else {
        for (i, team) in panel.teams.iter().enumerate() {
            let is_selected = i == panel.selected;

            let mut spans = Vec::new();

            if is_selected {
                spans.push(Span::styled(
                    " ❯ ".to_owned(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("   ", Style::default()));
            }

            // Active indicator
            if team.is_active {
                spans.push(Span::styled(
                    "●".to_owned(),
                    Style::default().fg(Color::Green),
                ));
            } else {
                spans.push(Span::styled(
                    "○".to_owned(),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            spans.push(Span::styled(" ".to_owned(), Style::default()));

            // Team name
            spans.push(if is_selected {
                Span::styled(
                    team.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(team.name.clone(), Style::default().fg(style.status_fg))
            });

            // Teammate count
            let tm_count = team.teammates.len();
            spans.push(dim_span(&format!(
                " ({tm_count} member{}, {} working, {} done)",
                if tm_count != 1 { "s" } else { "" },
                team.working_count(),
                team.done_count()
            )));

            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter detail │ Esc back │ q close",
    )));

    lines
}

/// Render team detail view.
pub fn render_team_detail(
    panel: &TeamPanel,
    index: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let team = match panel.teams.get(index) {
        Some(t) => t,
        None => {
            lines.push(Line::from("Team not found."));
            return lines;
        }
    };

    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(&team.name, style),
        dim_span(&format!(" ({})", team.id)),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if team.is_active {
        lines.push(Line::from(Span::styled(
            "  Status: active".to_owned(),
            Style::default().fg(Color::Green),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Status: inactive".to_owned(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));

    if team.teammates.is_empty() {
        lines.push(Line::from(dim_span("  No teammates.")));
    } else {
        lines.push(Line::from(Span::styled(
            "  Teammates:".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )));

        for tm in &team.teammates {
            let status_color = tm.status.color();

            let mut spans = vec![
                Span::styled("    ".to_owned(), Style::default()),
                Span::styled(
                    tm.status.indicator().to_owned(),
                    Style::default().fg(status_color),
                ),
                Span::styled(" ".to_owned(), Style::default()),
                Span::styled(tm.name.clone(), Style::default().fg(style.status_fg)),
            ];

            if let Some(task) = &tm.current_task {
                spans.push(dim_span(&format!(" — {task}")));
            }

            if tm.message_count > 0 {
                spans.push(dim_span(&format!(" ({} msgs)", tm.message_count)));
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

    fn sample_teammate(name: &str, status: TeammateStatus) -> TeammateInfo {
        let is_working = matches!(status, TeammateStatus::Working);
        TeammateInfo {
            name: name.to_owned(),
            status,
            current_task: if is_working {
                Some("analyzing code".to_owned())
            } else {
                None
            },
            message_count: 5,
        }
    }

    fn sample_team(name: &str) -> TeamInfo {
        TeamInfo {
            id: format!("team-{name}"),
            name: name.to_owned(),
            teammates: vec![
                sample_teammate("agent-1", TeammateStatus::Working),
                sample_teammate("agent-2", TeammateStatus::Done),
                sample_teammate("agent-3", TeammateStatus::Idle),
            ],
            is_active: true,
        }
    }

    fn sample_panel() -> TeamPanel {
        TeamPanel::new(vec![
            sample_team("alpha"),
            TeamInfo {
                id: "team-beta".to_owned(),
                name: "beta".to_owned(),
                teammates: vec![],
                is_active: false,
            },
        ])
    }

    // -- TeammateStatus --

    #[test]
    fn teammate_status_indicator() {
        assert_eq!(TeammateStatus::Idle.indicator(), "○");
        assert_eq!(TeammateStatus::Working.indicator(), "◐");
        assert_eq!(TeammateStatus::Done.indicator(), "●");
        assert_eq!(TeammateStatus::Failed("err".to_owned()).indicator(), "✗");
    }

    #[test]
    fn teammate_status_color() {
        assert_eq!(TeammateStatus::Idle.color(), Color::DarkGray);
        assert_eq!(TeammateStatus::Working.color(), Color::Yellow);
        assert_eq!(TeammateStatus::Done.color(), Color::Green);
    }

    // -- TeamInfo --

    #[test]
    fn team_working_count() {
        let team = sample_team("test");
        assert_eq!(team.working_count(), 1);
    }

    #[test]
    fn team_done_count() {
        let team = sample_team("test");
        assert_eq!(team.done_count(), 1);
    }

    // -- TeamPanel navigation --

    #[test]
    fn new_panel_starts_at_list() {
        let panel = sample_panel();
        assert_eq!(panel.view, TeamView::List);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn move_down_increments() {
        let mut panel = sample_panel();
        panel.move_down();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_up_clamps() {
        let mut panel = sample_panel();
        panel.move_up();
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn enter_team_transitions() {
        let mut panel = sample_panel();
        panel.enter_team();
        assert_eq!(panel.view, TeamView::Detail { index: 0 });
    }

    #[test]
    fn go_back_returns_to_list() {
        let mut panel = sample_panel();
        panel.enter_team();
        panel.go_back();
        assert_eq!(panel.view, TeamView::List);
    }

    // -- Rendering --

    #[test]
    fn render_list_contains_team_names() {
        let panel = sample_panel();
        let lines = render_team_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("alpha"));
        assert!(combined.contains("beta"));
    }

    #[test]
    fn render_list_shows_member_count() {
        let panel = sample_panel();
        let lines = render_team_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("3 members"));
        assert!(combined.contains("1 working"));
        assert!(combined.contains("1 done"));
    }

    #[test]
    fn render_list_empty() {
        let panel = TeamPanel::new(vec![]);
        let lines = render_team_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No active teams"));
    }

    #[test]
    fn render_detail_shows_team_name() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("alpha"));
        assert!(combined.contains("active"));
    }

    #[test]
    fn render_detail_shows_teammates() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("agent-1"));
        assert!(combined.contains("agent-2"));
        assert!(combined.contains("agent-3"));
    }

    #[test]
    fn render_detail_shows_task() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("analyzing code"));
    }

    #[test]
    fn render_detail_shows_message_count() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("5 msgs"));
    }

    #[test]
    fn render_detail_invalid() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 99, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Team not found"));
    }

    #[test]
    fn render_detail_empty_team() {
        let panel = sample_panel();
        let lines = render_team_detail(&panel, 1, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No teammates"));
        assert!(combined.contains("inactive"));
    }

    #[test]
    fn render_panel_dispatches_to_list() {
        let panel = sample_panel();
        let lines = render_team_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Teams"));
    }

    #[test]
    fn render_panel_dispatches_to_detail() {
        let mut panel = sample_panel();
        panel.view = TeamView::Detail { index: 0 };
        let lines = render_team_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("alpha"));
    }
}
