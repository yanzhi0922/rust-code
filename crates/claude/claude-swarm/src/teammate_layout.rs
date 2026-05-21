//! Teammate layout management.
//!
//! Handles color assignment and terminal layout for teammates.

use crate::constants::TEAMMATE_COLORS;
use crate::types::TeamFile;

/// Assign a color to a teammate based on their index in the team.
#[must_use]
pub fn assign_color(index: usize) -> &'static str {
    TEAMMATE_COLORS[index % TEAMMATE_COLORS.len()]
}

/// Assign colors to all team members.
///
/// Returns a mapping from agent name to color.
#[must_use]
pub fn assign_colors(team: &TeamFile) -> Vec<(String, String)> {
    team.members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.name.clone(), assign_color(i).to_owned()))
        .collect()
}

/// Get the next available color for a new teammate.
#[must_use]
pub fn next_color(team: &TeamFile) -> &'static str {
    assign_color(team.members.len())
}

/// Build a layout summary string for display.
#[must_use]
pub fn layout_summary(team: &TeamFile) -> String {
    let mut summary = String::new();
    summary.push_str(&format!("Team: {}\n", team.name));
    summary.push_str(&format!("Lead: {}\n", team.lead_agent_id));
    if team.members.is_empty() {
        summary.push_str("No teammates.\n");
    } else {
        summary.push_str("Teammates:\n");
        for (i, member) in team.members.iter().enumerate() {
            let color = member
                .color
                .as_deref()
                .expect("should have color")
                .to_owned();
            summary.push_str(&format!(
                "  {} [{}] - {} ({})\n",
                i + 1,
                color,
                member.name,
                member.backend_type.map_or("unknown", |b| b.as_str()),
            ));
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BackendType, TeamMember};

    #[test]
    fn assign_color_first() {
        assert_eq!(assign_color(0), "cyan");
    }

    #[test]
    fn assign_color_wraps_around() {
        let color_count = TEAMMATE_COLORS.len();
        assert_eq!(assign_color(color_count), TEAMMATE_COLORS[0]);
        assert_eq!(assign_color(color_count + 1), TEAMMATE_COLORS[1]);
    }

    #[test]
    fn assign_colors_empty_team() {
        let team = TeamFile::new("test", "lead-1");
        let colors = assign_colors(&team);
        assert!(colors.is_empty());
    }

    #[test]
    fn assign_colors_multiple_members() {
        let mut team = TeamFile::new("test", "lead-1");
        team.members.push(TeamMember::new("a1", "w1", "p1", "/tmp"));
        team.members.push(TeamMember::new("a2", "w2", "p2", "/tmp"));

        let colors = assign_colors(&team);
        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0].1, "cyan");
        assert_eq!(colors[1].1, "magenta");
    }

    #[test]
    fn next_color_empty_team() {
        let team = TeamFile::new("test", "lead-1");
        assert_eq!(next_color(&team), "cyan");
    }

    #[test]
    fn next_color_with_members() {
        let mut team = TeamFile::new("test", "lead-1");
        team.members.push(TeamMember::new("a1", "w1", "p1", "/tmp"));
        assert_eq!(next_color(&team), "magenta");
    }

    #[test]
    fn layout_summary_empty() {
        let team = TeamFile::new("test", "lead-1");
        let summary = layout_summary(&team);
        assert!(summary.contains("test"));
        assert!(summary.contains("No teammates"));
    }

    #[test]
    fn layout_summary_with_members() {
        let mut team = TeamFile::new("test", "lead-1");
        let mut m = TeamMember::new("a1", "worker-1", "p1", "/tmp");
        m.color = Some("cyan".to_owned());
        m.backend_type = Some(BackendType::InProcess);
        team.members.push(m);

        let summary = layout_summary(&team);
        assert!(summary.contains("worker-1"));
        assert!(summary.contains("cyan"));
        assert!(summary.contains("in_process"));
    }

    #[test]
    fn all_colors_are_valid() {
        for i in 0..20 {
            let color = assign_color(i);
            assert!(!color.is_empty());
        }
    }
}
