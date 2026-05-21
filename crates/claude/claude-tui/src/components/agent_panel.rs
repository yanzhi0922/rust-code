//! Agent management panel component.
//!
//! Renders a list of agents with status icons and a detailed view for a
//! single agent entry.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Status of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Killed,
}

impl AgentStatus {
    /// Icon character used to visually represent the status.
    pub fn icon(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "⏸",
            AgentStatus::Running => "▶",
            AgentStatus::Completed => "✔",
            AgentStatus::Failed => "✖",
            AgentStatus::Killed => "☠",
        }
    }

    /// Colour associated with the status.
    pub fn color(&self) -> Color {
        match self {
            AgentStatus::Idle => Color::DarkGray,
            AgentStatus::Running => Color::Cyan,
            AgentStatus::Completed => Color::Green,
            AgentStatus::Failed => Color::Red,
            AgentStatus::Killed => Color::Magenta,
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "Idle",
            AgentStatus::Running => "Running",
            AgentStatus::Completed => "Completed",
            AgentStatus::Failed => "Failed",
            AgentStatus::Killed => "Killed",
        }
    }
}

/// A single agent entry for display.
#[derive(Debug, Clone)]
pub struct AgentEntry {
    pub name: String,
    pub agent_type: String,
    pub status: AgentStatus,
    pub tool_count: usize,
}

// ---------------------------------------------------------------------------
// AgentListRenderer
// ---------------------------------------------------------------------------

/// Renders a list of agents with status icons.
pub struct AgentListRenderer;

impl AgentListRenderer {
    /// Header line for the agent list.
    fn header() -> Line<'static> {
        Line::from(vec![Span::styled(
            "Agents".to_string(),
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )])
    }

    /// Render the full agent list as lines.
    pub fn render(agents: &[AgentEntry]) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(Self::header());

        if agents.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No agents active".to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            return lines;
        }

        for agent in agents {
            let icon_span = Span::styled(
                format!(" {} ", agent.status.icon()),
                Style::default().fg(agent.status.color()),
            );
            let name_span = Span::styled(
                agent.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            );
            let detail_span = Span::styled(
                format!(" [{}] tools:{}", agent.agent_type, agent.tool_count),
                Style::default().fg(Color::DarkGray),
            );
            lines.push(Line::from(vec![icon_span, name_span, detail_span]));
        }

        lines
    }
}

// ---------------------------------------------------------------------------
// AgentDetailRenderer
// ---------------------------------------------------------------------------

/// Renders a detailed view for a single agent.
pub struct AgentDetailRenderer;

impl AgentDetailRenderer {
    /// Render detailed information about one agent.
    pub fn render(agent: &AgentEntry) -> Vec<Line<'static>> {
        vec![
            Line::from(vec![Span::styled(
                format!(" {} {}", agent.status.icon(), agent.name),
                Style::default()
                    .fg(agent.status.color())
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![
                Span::raw("  Type: "),
                Span::styled(agent.agent_type.clone(), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::raw("  Status: "),
                Span::styled(
                    agent.status.label().to_string(),
                    Style::default().fg(agent.status.color()),
                ),
            ]),
            Line::from(vec![
                Span::raw("  Tools used: "),
                Span::styled(
                    agent.tool_count.to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        ]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- AgentStatus tests --

    #[test]
    fn test_status_icons() {
        assert_eq!(AgentStatus::Idle.icon(), "⏸");
        assert_eq!(AgentStatus::Running.icon(), "▶");
        assert_eq!(AgentStatus::Completed.icon(), "✔");
        assert_eq!(AgentStatus::Failed.icon(), "✖");
        assert_eq!(AgentStatus::Killed.icon(), "☠");
    }

    #[test]
    fn test_status_colors() {
        assert_eq!(AgentStatus::Idle.color(), Color::DarkGray);
        assert_eq!(AgentStatus::Running.color(), Color::Cyan);
        assert_eq!(AgentStatus::Completed.color(), Color::Green);
        assert_eq!(AgentStatus::Failed.color(), Color::Red);
        assert_eq!(AgentStatus::Killed.color(), Color::Magenta);
    }

    #[test]
    fn test_status_labels() {
        assert_eq!(AgentStatus::Idle.label(), "Idle");
        assert_eq!(AgentStatus::Running.label(), "Running");
        assert_eq!(AgentStatus::Completed.label(), "Completed");
        assert_eq!(AgentStatus::Failed.label(), "Failed");
        assert_eq!(AgentStatus::Killed.label(), "Killed");
    }

    // -- AgentListRenderer tests --

    #[test]
    fn test_list_render_empty() {
        let lines = AgentListRenderer::render(&[]);
        assert!(lines.len() >= 2); // header + empty message
        let empty_line = &lines[1];
        let content: String = empty_line.spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("No agents active"));
    }

    #[test]
    fn test_list_render_header() {
        let lines = AgentListRenderer::render(&[]);
        let header_content: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(header_content, "Agents");
    }

    #[test]
    fn test_list_render_with_agents() {
        let agents = vec![
            AgentEntry {
                name: "coder".to_string(),
                agent_type: "build".to_string(),
                status: AgentStatus::Running,
                tool_count: 3,
            },
            AgentEntry {
                name: "tester".to_string(),
                agent_type: "test".to_string(),
                status: AgentStatus::Idle,
                tool_count: 0,
            },
        ];
        let lines = AgentListRenderer::render(&agents);
        // header + 2 agents = 3 lines
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_list_agent_line_contains_name() {
        let agents = vec![AgentEntry {
            name: "builder".to_string(),
            agent_type: "general".to_string(),
            status: AgentStatus::Running,
            tool_count: 5,
        }];
        let lines = AgentListRenderer::render(&agents);
        let agent_line = &lines[1];
        let content: String = agent_line.spans.iter().map(|s| s.content.clone()).collect();
        assert!(content.contains("builder"));
    }

    // -- AgentDetailRenderer tests --

    #[test]
    fn test_detail_render_line_count() {
        let agent = AgentEntry {
            name: "coder".to_string(),
            agent_type: "build".to_string(),
            status: AgentStatus::Running,
            tool_count: 7,
        };
        let lines = AgentDetailRenderer::render(&agent);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn test_detail_render_contains_name() {
        let agent = AgentEntry {
            name: "reviewer".to_string(),
            agent_type: "review".to_string(),
            status: AgentStatus::Completed,
            tool_count: 2,
        };
        let lines = AgentDetailRenderer::render(&agent);
        let header: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(header.contains("reviewer"));
    }

    #[test]
    fn test_detail_render_contains_type() {
        let agent = AgentEntry {
            name: "agent1".to_string(),
            agent_type: "special".to_string(),
            status: AgentStatus::Idle,
            tool_count: 0,
        };
        let lines = AgentDetailRenderer::render(&agent);
        let type_line: String = lines[1].spans.iter().map(|s| s.content.clone()).collect();
        assert!(type_line.contains("special"));
    }

    #[test]
    fn test_detail_render_contains_tool_count() {
        let agent = AgentEntry {
            name: "agent2".to_string(),
            agent_type: "general".to_string(),
            status: AgentStatus::Failed,
            tool_count: 42,
        };
        let lines = AgentDetailRenderer::render(&agent);
        let tool_line: String = lines[3].spans.iter().map(|s| s.content.clone()).collect();
        assert!(tool_line.contains("42"));
    }
}
