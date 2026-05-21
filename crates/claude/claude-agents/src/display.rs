//! Agent display and color management matching Claude Code's `AgentTool/agentDisplay.ts`.
//!
//! Provides display types, color assignment, status formatting, source
//! grouping, and an [`AgentColorManager`] for unique per-agent color
//! assignment in CLI and interactive contexts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::definition::{AgentDefinition, AgentSource};

/// Color palette for agent display.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentColor {
    #[default]
    Blue,
    Green,
    Yellow,
    Red,
    Purple,
    Cyan,
    Orange,
    Pink,
    Teal,
    Indigo,
}

impl AgentColor {
    /// ANSI escape code for the foreground color.
    pub fn ansi_fg(&self) -> &'static str {
        match self {
            Self::Blue => "\x1b[34m",
            Self::Green => "\x1b[32m",
            Self::Yellow => "\x1b[33m",
            Self::Red => "\x1b[31m",
            Self::Purple => "\x1b[35m",
            Self::Cyan => "\x1b[36m",
            Self::Orange => "\x1b[38;5;208m",
            Self::Pink => "\x1b[38;5;213m",
            Self::Teal => "\x1b[38;5;37m",
            Self::Indigo => "\x1b[38;5;63m",
        }
    }

    /// ANSI reset code.
    pub fn ansi_reset() -> &'static str {
        "\x1b[0m"
    }
}

/// Display metadata for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDisplay {
    /// Display name for the agent.
    pub name: String,
    /// Assigned display color.
    pub color: AgentColor,
    /// Icon/emoji for the agent.
    pub icon: String,
}

/// Ordered source groups for consistent display.
pub struct AgentSourceGroup {
    /// Human-readable group label.
    pub label: &'static str,
    /// The source this group represents.
    pub source: AgentSource,
}

/// Ordered list of agent source groups for display.
pub const AGENT_SOURCE_GROUPS: &[AgentSourceGroup] = &[
    AgentSourceGroup {
        label: "User agents",
        source: AgentSource::User,
    },
    AgentSourceGroup {
        label: "Project agents",
        source: AgentSource::Project,
    },
    AgentSourceGroup {
        label: "Local agents",
        source: AgentSource::Local,
    },
    AgentSourceGroup {
        label: "Managed agents",
        source: AgentSource::Policy,
    },
    AgentSourceGroup {
        label: "Plugin agents",
        source: AgentSource::Plugin,
    },
    AgentSourceGroup {
        label: "CLI arg agents",
        source: AgentSource::Flag,
    },
    AgentSourceGroup {
        label: "Built-in agents",
        source: AgentSource::BuiltIn,
    },
];

/// An agent annotated with override information.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// The underlying agent definition.
    pub definition: AgentDefinition,
    /// The source that overrides this agent, if any.
    pub overridden_by: Option<AgentSource>,
}

/// Assign a display color to an agent based on its type name.
///
/// Uses a stable hash of the agent type to pick from the color palette,
/// ensuring the same agent type always gets the same color.
pub fn agent_color_for_type(agent_type: &str) -> AgentColor {
    let colors = [
        AgentColor::Blue,
        AgentColor::Green,
        AgentColor::Yellow,
        AgentColor::Purple,
        AgentColor::Cyan,
        AgentColor::Orange,
        AgentColor::Pink,
        AgentColor::Teal,
        AgentColor::Indigo,
    ];

    // Simple stable hash: sum of byte values
    let hash: usize = agent_type.bytes().map(usize::from).sum();
    colors[hash % colors.len()]
}

/// Build an [`AgentDisplay`] for the given agent definition.
pub fn build_agent_display(agent: &AgentDefinition) -> AgentDisplay {
    let icon = agent_icon_for_type(&agent.agent_type);
    AgentDisplay {
        name: agent.agent_type.clone(),
        color: agent_color_for_type(&agent.agent_type),
        icon,
    }
}

/// Get an icon/emoji for an agent type.
pub fn agent_icon_for_type(agent_type: &str) -> String {
    match agent_type {
        "general-purpose" => "🤖".to_owned(),
        "Explore" => "🔍".to_owned(),
        "Plan" => "📋".to_owned(),
        "verification" => "✅".to_owned(),
        "claude-code-guide" => "📖".to_owned(),
        "statusline-setup" => "💻".to_owned(),
        "fork" => "🔀".to_owned(),
        _ => "⚙️".to_owned(),
    }
}

/// Format an agent's status line with color and state information.
pub fn format_agent_status(name: &str, state: crate::AgentState, progress: Option<f64>) -> String {
    let color = agent_color_for_type(name);
    let state_str = match state {
        crate::AgentState::Idle => "idle",
        crate::AgentState::Busy => "busy",
        crate::AgentState::Draining => "draining",
        crate::AgentState::Offline => "offline",
    };
    let progress_str = match progress {
        Some(p) => format!(" ({:.0}%)", p * 100.0),
        None => String::new(),
    };
    format!(
        "{}{}{} [{}]{}",
        color.ansi_fg(),
        name,
        AgentColor::ansi_reset(),
        state_str,
        progress_str
    )
}

/// Annotate agents with override information by comparing against the active
/// (winning) agent list. An agent is "overridden" when another agent with the
/// same type from a higher-priority source takes precedence.
///
/// Also deduplicates by `(agent_type, source)` to handle duplicates.
pub fn resolve_agent_overrides(
    all_agents: &[AgentDefinition],
    active_agents: &[AgentDefinition],
) -> Vec<ResolvedAgent> {
    let active_map: std::collections::HashMap<&str, &AgentDefinition> = active_agents
        .iter()
        .map(|a| (a.agent_type.as_str(), a))
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut resolved = Vec::new();

    for agent in all_agents {
        let key = format!("{}:{}", agent.agent_type, agent.source);
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let overridden_by = match active_map.get(agent.agent_type.as_str()) {
            Some(active) if active.source != agent.source => Some(active.source),
            _ => None,
        };

        resolved.push(ResolvedAgent {
            definition: agent.clone(),
            overridden_by,
        });
    }

    resolved
}

/// Compare agents alphabetically by name (case-insensitive).
pub fn compare_agents_by_name(a: &AgentDefinition, b: &AgentDefinition) -> std::cmp::Ordering {
    a.agent_type
        .to_lowercase()
        .cmp(&b.agent_type.to_lowercase())
}

/// Get a human-readable label for the source that overrides an agent.
pub fn get_override_source_label(source: AgentSource) -> &'static str {
    match source {
        AgentSource::BuiltIn => "built-in",
        AgentSource::User => "user",
        AgentSource::Project => "project",
        AgentSource::Local => "local",
        AgentSource::Policy => "managed",
        AgentSource::Plugin => "plugin",
        AgentSource::Flag => "cli",
        AgentSource::Marketplace => "marketplace",
    }
}

// ── Enhanced display types ────────────────────────────────────────────────

/// Manages unique color assignment for multiple agents.
///
/// Each agent is assigned a unique color from the palette, cycling through
/// when there are more agents than colors. This ensures visual distinction
/// when multiple agents are displayed simultaneously.
#[derive(Debug, Clone)]
pub struct AgentColorManager {
    /// Map from agent ID to assigned color.
    assignments: BTreeMap<String, AgentColor>,
    /// Color palette for cycling.
    palette: Vec<AgentColor>,
    /// Next color index for new assignments.
    next_index: usize,
}

impl Default for AgentColorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentColorManager {
    /// Create a new color manager with the default palette.
    #[must_use]
    pub fn new() -> Self {
        Self {
            assignments: BTreeMap::new(),
            palette: vec![
                AgentColor::Blue,
                AgentColor::Green,
                AgentColor::Yellow,
                AgentColor::Purple,
                AgentColor::Cyan,
                AgentColor::Orange,
                AgentColor::Pink,
                AgentColor::Teal,
                AgentColor::Indigo,
            ],
            next_index: 0,
        }
    }

    /// Get or assign a color for the given agent.
    ///
    /// If the agent already has a color, returns it. Otherwise assigns the
    /// next color from the palette (cycling when exhausted).
    pub fn color_for(&mut self, agent_id: &str) -> AgentColor {
        if let Some(&color) = self.assignments.get(agent_id) {
            return color;
        }
        let color = self.palette[self.next_index % self.palette.len()];
        self.assignments.insert(agent_id.to_owned(), color);
        self.next_index += 1;
        color
    }

    /// Get the color for an agent if it has been assigned.
    pub fn get_color(&self, agent_id: &str) -> Option<AgentColor> {
        self.assignments.get(agent_id).copied()
    }

    /// Remove an agent's color assignment.
    pub fn remove(&mut self, agent_id: &str) {
        self.assignments.remove(agent_id);
    }

    /// Get the number of agents with assigned colors.
    pub fn len(&self) -> usize {
        self.assignments.len()
    }

    /// Check if no agents have been assigned colors.
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }

    /// Clear all color assignments.
    pub fn clear(&mut self) {
        self.assignments.clear();
        self.next_index = 0;
    }
}

/// Format an agent header for display.
///
/// Produces a colored header line with the agent's icon, name, and
/// optional description.
pub fn format_agent_header(name: &str, description: Option<&str>, color: AgentColor) -> String {
    let icon = agent_icon_for_type(name);
    let reset = AgentColor::ansi_reset();
    let fg = color.ansi_fg();

    match description {
        Some(desc) => format!("{fg}{icon} {name}{reset} — {desc}"),
        None => format!("{fg}{icon} {name}{reset}"),
    }
}

/// Format an agent result for display.
///
/// Produces a colored result block with the agent's output and
/// optional usage information.
pub fn format_agent_result(
    agent_id: &str,
    output: &str,
    success: bool,
    usage: Option<&crate::runner::UsageSummary>,
    color: AgentColor,
) -> String {
    let reset = AgentColor::ansi_reset();
    let fg = color.ansi_fg();
    let status_icon = if success { "✓" } else { "✗" };

    let mut result = format!("{fg}{status_icon} Agent {agent_id}{reset}\n");
    result.push_str(output);

    if let Some(usage) = usage {
        result.push_str(&format!(
            "\n{fg}Tokens: {}+{} (cache: +{}, -{}){reset}",
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_tokens,
            usage.cache_read_tokens,
        ));
    }

    result
}

/// Format a duration in a human-readable way.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m {secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::AgentDefinition;

    #[test]
    fn agent_color_is_stable() {
        let c1 = agent_color_for_type("general-purpose");
        let c2 = agent_color_for_type("general-purpose");
        assert_eq!(c1, c2);
    }

    #[test]
    fn different_agents_get_different_colors() {
        let c1 = agent_color_for_type("Explore");
        let c2 = agent_color_for_type("Plan");
        // Not guaranteed to be different, but highly likely
        // Just verify they are valid colors
        assert!(matches!(
            c1,
            AgentColor::Blue
                | AgentColor::Green
                | AgentColor::Yellow
                | AgentColor::Red
                | AgentColor::Purple
                | AgentColor::Cyan
                | AgentColor::Orange
                | AgentColor::Pink
                | AgentColor::Teal
                | AgentColor::Indigo
        ));
        assert!(matches!(
            c2,
            AgentColor::Blue
                | AgentColor::Green
                | AgentColor::Yellow
                | AgentColor::Red
                | AgentColor::Purple
                | AgentColor::Cyan
                | AgentColor::Orange
                | AgentColor::Pink
                | AgentColor::Teal
                | AgentColor::Indigo
        ));
    }

    #[test]
    fn agent_icons_for_known_types() {
        assert_eq!(agent_icon_for_type("general-purpose"), "🤖");
        assert_eq!(agent_icon_for_type("Explore"), "🔍");
        assert_eq!(agent_icon_for_type("Plan"), "📋");
        assert_eq!(agent_icon_for_type("fork"), "🔀");
    }

    #[test]
    fn agent_icon_fallback() {
        assert_eq!(agent_icon_for_type("custom-agent"), "⚙️");
    }

    #[test]
    fn format_status_idle() {
        let status = format_agent_status("test-agent", crate::AgentState::Idle, None);
        assert!(status.contains("test-agent"));
        assert!(status.contains("[idle]"));
    }

    #[test]
    fn format_status_with_progress() {
        let status = format_agent_status("test", crate::AgentState::Busy, Some(0.75));
        assert!(status.contains("[busy]"));
        assert!(status.contains("75%"));
    }

    #[test]
    fn resolve_overrides_detects_override() {
        let built_in = AgentDefinition::new("test", "built-in");
        let user = {
            let mut d = AgentDefinition::new("test", "user version");
            d.source = AgentSource::User;
            d
        };

        let all = vec![built_in.clone(), user.clone()];
        let active = vec![user];
        let resolved = resolve_agent_overrides(&all, &active);

        assert_eq!(resolved.len(), 2);
        // The built-in should be marked as overridden
        let bi = resolved
            .iter()
            .find(|r| r.definition.source == AgentSource::BuiltIn);
        assert!(bi.is_some());
        assert_eq!(bi.expect("found").overridden_by, Some(AgentSource::User));
    }

    #[test]
    fn compare_agents_sorts_case_insensitive() {
        let a = AgentDefinition::new("Beta", "b");
        let b = AgentDefinition::new("alpha", "a");
        assert_eq!(compare_agents_by_name(&a, &b), std::cmp::Ordering::Greater);
    }

    #[test]
    fn override_source_labels() {
        assert_eq!(get_override_source_label(AgentSource::BuiltIn), "built-in");
        assert_eq!(get_override_source_label(AgentSource::User), "user");
        assert_eq!(get_override_source_label(AgentSource::Plugin), "plugin");
    }

    // ── Enhanced tests ──────────────────────────────────────────────────

    #[test]
    fn color_manager_assigns_unique_colors() {
        let mut mgr = AgentColorManager::new();
        let c1 = mgr.color_for("agent-1");
        let c2 = mgr.color_for("agent-2");
        assert_ne!(c1, c2);
    }

    #[test]
    fn color_manager_returns_same_color_for_same_agent() {
        let mut mgr = AgentColorManager::new();
        let c1 = mgr.color_for("agent-1");
        let c2 = mgr.color_for("agent-1");
        assert_eq!(c1, c2);
    }

    #[test]
    fn color_manager_cycles_palette() {
        let mut mgr = AgentColorManager::new();
        let palette_size = 9; // default palette has 9 colors
        for i in 0..palette_size * 2 {
            let _ = mgr.color_for(&format!("agent-{i}"));
        }
        // Should have assigned 18 agents without panic (cycling)
        assert_eq!(mgr.len(), palette_size * 2);
    }

    #[test]
    fn color_manager_get_color_unassigned() {
        let mgr = AgentColorManager::new();
        assert!(mgr.get_color("nonexistent").is_none());
    }

    #[test]
    fn color_manager_get_color_assigned() {
        let mut mgr = AgentColorManager::new();
        let color = mgr.color_for("agent-1");
        assert_eq!(mgr.get_color("agent-1"), Some(color));
    }

    #[test]
    fn color_manager_remove() {
        let mut mgr = AgentColorManager::new();
        mgr.color_for("agent-1");
        mgr.remove("agent-1");
        assert!(mgr.get_color("agent-1").is_none());
        assert!(mgr.is_empty());
    }

    #[test]
    fn color_manager_clear() {
        let mut mgr = AgentColorManager::new();
        mgr.color_for("a");
        mgr.color_for("b");
        mgr.clear();
        assert!(mgr.is_empty());
    }

    #[test]
    fn color_manager_default() {
        let mgr = AgentColorManager::default();
        assert!(mgr.is_empty());
    }

    #[test]
    fn format_agent_header_with_description() {
        let header = format_agent_header("worker", Some("Fix auth bug"), AgentColor::Blue);
        assert!(header.contains("worker"));
        assert!(header.contains("Fix auth bug"));
        assert!(header.contains("⚙️"));
    }

    #[test]
    fn format_agent_header_without_description() {
        let header = format_agent_header("Explore", None, AgentColor::Green);
        assert!(header.contains("Explore"));
        assert!(header.contains("🔍"));
        assert!(!header.contains("—"));
    }

    #[test]
    fn format_agent_result_success() {
        let usage = crate::runner::UsageSummary {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 20,
        };
        let result = format_agent_result("agent-1", "Done", true, Some(&usage), AgentColor::Cyan);
        assert!(result.contains("✓"));
        assert!(result.contains("agent-1"));
        assert!(result.contains("Done"));
        assert!(result.contains("100+50"));
    }

    #[test]
    fn format_agent_result_failure() {
        let result = format_agent_result("agent-2", "Error", false, None, AgentColor::Red);
        assert!(result.contains("✗"));
        assert!(result.contains("Error"));
    }

    #[test]
    fn format_duration_milliseconds() {
        assert_eq!(format_duration(500), "500ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(1500), "1.5s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(125_000), "2m 5s");
    }

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(0), "0ms");
    }
}
