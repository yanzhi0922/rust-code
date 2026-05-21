//! Agent editor component for the TUI.
//!
//! Provides a panel for creating, editing, and managing custom agents.
//! Mirrors the agent management components in `cc-haha/src/components/agents/`.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`AgentDefinition`] | Agent configuration definition |
//! | [`AgentEditorField`] | Editable fields in the agent editor |
//! | [`AgentEditor`] | Editor state machine |
//! | [`render_agent_editor`] | Render the agent editor |
//! | [`render_agent_list`] | Render the list of agents |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// AgentDefinition
// ---------------------------------------------------------------------------

/// A custom agent definition.
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    /// Agent name/identifier.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The model to use (e.g., "claude-sonnet-4-20250514").
    pub model: Option<String>,
    /// System prompt / instructions.
    pub instructions: String,
    /// List of allowed tool names.
    pub allowed_tools: Vec<String>,
    /// Whether the agent is enabled.
    pub enabled: bool,
    /// Source of the definition (e.g., "project", "user").
    pub source: String,
}

// ---------------------------------------------------------------------------
// AgentEditorField
// ---------------------------------------------------------------------------

/// Editable fields in the agent editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEditorField {
    /// Agent name.
    Name,
    /// Description.
    Description,
    /// Model selection.
    Model,
    /// Instructions / system prompt.
    Instructions,
    /// Tool selection.
    Tools,
    /// Enabled toggle.
    Enabled,
}

// ---------------------------------------------------------------------------
// AgentEditor
// ---------------------------------------------------------------------------

/// Current mode of the agent editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEditorMode {
    /// Browsing the list of agents.
    List,
    /// Editing an existing agent.
    Edit { index: usize },
    /// Creating a new agent.
    Create,
}

/// Agent editor state.
#[derive(Debug, Clone)]
pub struct AgentEditor {
    /// All agent definitions.
    pub agents: Vec<AgentDefinition>,
    /// Current mode.
    pub mode: AgentEditorMode,
    /// Currently selected item index.
    pub selected: usize,
    /// Currently focused field (in edit/create mode).
    pub focused_field: AgentEditorField,
    /// Draft agent being edited/created.
    pub draft: AgentDefinition,
}

impl AgentEditor {
    /// Create a new agent editor starting at the list view.
    pub fn new(agents: Vec<AgentDefinition>) -> Self {
        Self {
            agents,
            mode: AgentEditorMode::List,
            selected: 0,
            focused_field: AgentEditorField::Name,
            draft: AgentDefinition {
                name: String::new(),
                description: String::new(),
                model: None,
                instructions: String::new(),
                allowed_tools: vec![],
                enabled: true,
                source: "user".to_owned(),
            },
        }
    }

    /// Start editing the selected agent.
    pub fn edit_selected(&mut self) {
        if self.selected < self.agents.len() {
            self.draft = self.agents[self.selected].clone();
            self.mode = AgentEditorMode::Edit {
                index: self.selected,
            };
            self.focused_field = AgentEditorField::Name;
        }
    }

    /// Start creating a new agent.
    pub fn create_new(&mut self) {
        self.draft = AgentDefinition {
            name: String::new(),
            description: String::new(),
            model: None,
            instructions: String::new(),
            allowed_tools: vec![],
            enabled: true,
            source: "user".to_owned(),
        };
        self.mode = AgentEditorMode::Create;
        self.focused_field = AgentEditorField::Name;
    }

    /// Go back to the list view.
    pub fn go_back(&mut self) {
        self.mode = AgentEditorMode::List;
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.agents.len() {
            self.selected += 1;
        }
    }

    /// Cycle to the next field.
    pub fn next_field(&mut self) {
        self.focused_field = match &self.focused_field {
            AgentEditorField::Name => AgentEditorField::Description,
            AgentEditorField::Description => AgentEditorField::Model,
            AgentEditorField::Model => AgentEditorField::Instructions,
            AgentEditorField::Instructions => AgentEditorField::Tools,
            AgentEditorField::Tools => AgentEditorField::Enabled,
            AgentEditorField::Enabled => AgentEditorField::Name,
        };
    }

    /// Cycle to the previous field.
    pub fn prev_field(&mut self) {
        self.focused_field = match &self.focused_field {
            AgentEditorField::Name => AgentEditorField::Enabled,
            AgentEditorField::Description => AgentEditorField::Name,
            AgentEditorField::Model => AgentEditorField::Description,
            AgentEditorField::Instructions => AgentEditorField::Model,
            AgentEditorField::Tools => AgentEditorField::Instructions,
            AgentEditorField::Enabled => AgentEditorField::Tools,
        };
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

fn field_label(label: &str, focused: bool, style: &StyleConfig) -> Span<'static> {
    if focused {
        Span::styled(
            format!("  {label}: ").to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("  {label}: ").to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        )
    }
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render the agent editor panel.
pub fn render_agent_editor(editor: &AgentEditor, style: &StyleConfig) -> Vec<Line<'static>> {
    match &editor.mode {
        AgentEditorMode::List => render_agent_list(editor, style),
        AgentEditorMode::Edit { .. } | AgentEditorMode::Create => render_agent_form(editor, style),
    }
}

/// Render the list of agents.
pub fn render_agent_list(editor: &AgentEditor, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(" Agents", style)));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if editor.agents.is_empty() {
        lines.push(Line::from(dim_span("   No custom agents defined.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span("   Press 'n' to create a new agent.")));
    } else {
        for (i, agent) in editor.agents.iter().enumerate() {
            let is_selected = i == editor.selected;

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

            // Enabled indicator
            if agent.enabled {
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

            // Agent name
            spans.push(if is_selected {
                Span::styled(
                    agent.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(agent.name.clone(), Style::default().fg(style.status_fg))
            });

            // Source
            spans.push(dim_span(&format!(" [{}]", agent.source)));

            // Model
            if let Some(model) = &agent.model {
                spans.push(dim_span(&format!(" ({model})")));
            }

            // Tool count
            let tool_count = agent.allowed_tools.len();
            if tool_count > 0 {
                spans.push(dim_span(&format!(" {tool_count} tools")));
            }

            lines.push(Line::from(spans));

            // Description (truncated)
            if !agent.description.is_empty() {
                let desc = if agent.description.len() > 60 {
                    format!("{}…", &agent.description[..59])
                } else {
                    agent.description.clone()
                };
                lines.push(Line::from(dim_span(&format!("     {desc}"))));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ e edit │ n new │ d delete │ q close",
    )));

    lines
}

/// Render the agent edit/create form.
pub fn render_agent_form(editor: &AgentEditor, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let title = match &editor.mode {
        AgentEditorMode::Edit { .. } => "Edit Agent",
        AgentEditorMode::Create => "New Agent",
        AgentEditorMode::List => unreachable!(),
    };

    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(title, style),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    let draft = &editor.draft;

    // Name field
    lines.push(Line::from(vec![
        field_label(
            "Name",
            editor.focused_field == AgentEditorField::Name,
            style,
        ),
        Span::styled(
            if draft.name.is_empty() {
                "<enter name>".to_owned()
            } else {
                draft.name.clone()
            },
            if editor.focused_field == AgentEditorField::Name {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(style.status_fg)
            },
        ),
    ]));

    // Description field
    lines.push(Line::from(vec![
        field_label(
            "Description",
            editor.focused_field == AgentEditorField::Description,
            style,
        ),
        Span::styled(
            if draft.description.is_empty() {
                "<enter description>".to_owned()
            } else {
                draft.description.clone()
            },
            if editor.focused_field == AgentEditorField::Description {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(style.status_fg)
            },
        ),
    ]));

    // Model field
    lines.push(Line::from(vec![
        field_label(
            "Model",
            editor.focused_field == AgentEditorField::Model,
            style,
        ),
        Span::styled(
            draft.model.as_deref().unwrap_or("<default>").to_owned(),
            if editor.focused_field == AgentEditorField::Model {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            },
        ),
    ]));

    // Instructions field
    lines.push(Line::from(vec![
        field_label(
            "Instructions",
            editor.focused_field == AgentEditorField::Instructions,
            style,
        ),
        Span::styled(
            if draft.instructions.is_empty() {
                "<system prompt>".to_owned()
            } else if draft.instructions.len() > 40 {
                format!("{}…", &draft.instructions[..39])
            } else {
                draft.instructions.clone()
            },
            if editor.focused_field == AgentEditorField::Instructions {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            },
        ),
    ]));

    // Tools field
    lines.push(Line::from(vec![
        field_label(
            "Tools",
            editor.focused_field == AgentEditorField::Tools,
            style,
        ),
        Span::styled(
            if draft.allowed_tools.is_empty() {
                "<all tools>".to_owned()
            } else {
                format!("{} tools", draft.allowed_tools.len())
            },
            if editor.focused_field == AgentEditorField::Tools {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            },
        ),
    ]));

    // Enabled field
    lines.push(Line::from(vec![
        field_label(
            "Enabled",
            editor.focused_field == AgentEditorField::Enabled,
            style,
        ),
        Span::styled(
            if draft.enabled {
                "yes".to_owned()
            } else {
                "no".to_owned()
            },
            if draft.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            },
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   Tab next field │ Shift+Tab prev │ Enter save │ Esc cancel",
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

    fn sample_agent(name: &str) -> AgentDefinition {
        AgentDefinition {
            name: name.to_owned(),
            description: format!("Agent {name} description"),
            model: Some("claude-sonnet-4-20250514".to_owned()),
            instructions: "You are a helpful assistant.".to_owned(),
            allowed_tools: vec!["bash".to_owned(), "read".to_owned()],
            enabled: true,
            source: "project".to_owned(),
        }
    }

    fn sample_editor() -> AgentEditor {
        AgentEditor::new(vec![
            sample_agent("coder"),
            sample_agent("reviewer"),
            AgentDefinition {
                name: "disabled-agent".to_owned(),
                description: "A disabled agent".to_owned(),
                model: None,
                instructions: String::new(),
                allowed_tools: vec![],
                enabled: false,
                source: "user".to_owned(),
            },
        ])
    }

    #[test]
    fn new_editor_starts_at_list() {
        let editor = sample_editor();
        assert_eq!(editor.mode, AgentEditorMode::List);
        assert_eq!(editor.selected, 0);
    }

    #[test]
    fn move_down_increments() {
        let mut editor = sample_editor();
        editor.move_down();
        assert_eq!(editor.selected, 1);
    }

    #[test]
    fn move_up_clamps() {
        let mut editor = sample_editor();
        editor.move_up();
        assert_eq!(editor.selected, 0);
    }

    #[test]
    fn edit_selected_transitions() {
        let mut editor = sample_editor();
        editor.selected = 1;
        editor.edit_selected();
        assert!(matches!(editor.mode, AgentEditorMode::Edit { index: 1 }));
        assert_eq!(editor.draft.name, "reviewer");
    }

    #[test]
    fn create_new_transitions() {
        let mut editor = sample_editor();
        editor.create_new();
        assert_eq!(editor.mode, AgentEditorMode::Create);
        assert!(editor.draft.name.is_empty());
    }

    #[test]
    fn go_back_returns_to_list() {
        let mut editor = sample_editor();
        editor.edit_selected();
        editor.go_back();
        assert_eq!(editor.mode, AgentEditorMode::List);
    }

    #[test]
    fn next_field_cycles() {
        let mut editor = sample_editor();
        assert_eq!(editor.focused_field, AgentEditorField::Name);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Description);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Model);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Instructions);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Tools);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Enabled);
        editor.next_field();
        assert_eq!(editor.focused_field, AgentEditorField::Name);
    }

    #[test]
    fn prev_field_cycles() {
        let mut editor = sample_editor();
        editor.prev_field();
        assert_eq!(editor.focused_field, AgentEditorField::Enabled);
    }

    #[test]
    fn render_list_contains_names() {
        let editor = sample_editor();
        let lines = render_agent_list(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("coder"));
        assert!(combined.contains("reviewer"));
        assert!(combined.contains("disabled-agent"));
    }

    #[test]
    fn render_list_empty() {
        let editor = AgentEditor::new(vec![]);
        let lines = render_agent_list(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No custom agents"));
    }

    #[test]
    fn render_list_shows_model() {
        let editor = sample_editor();
        let lines = render_agent_list(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("claude-sonnet-4-20250514"));
    }

    #[test]
    fn render_list_shows_tool_count() {
        let editor = sample_editor();
        let lines = render_agent_list(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("2 tools"));
    }

    #[test]
    fn render_form_edit_shows_name() {
        let mut editor = sample_editor();
        editor.edit_selected();
        let lines = render_agent_form(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Edit Agent"));
        assert!(combined.contains("coder"));
    }

    #[test]
    fn render_form_create_shows_new() {
        let mut editor = sample_editor();
        editor.create_new();
        let lines = render_agent_form(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("New Agent"));
        assert!(combined.contains("<enter name>"));
    }

    #[test]
    fn render_form_shows_all_fields() {
        let mut editor = sample_editor();
        editor.edit_selected();
        let lines = render_agent_form(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Name:"));
        assert!(combined.contains("Description:"));
        assert!(combined.contains("Model:"));
        assert!(combined.contains("Instructions:"));
        assert!(combined.contains("Tools:"));
        assert!(combined.contains("Enabled:"));
    }

    #[test]
    fn render_form_disabled_shows_no() {
        let mut editor = sample_editor();
        editor.selected = 2;
        editor.edit_selected();
        let lines = render_agent_form(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("no"));
    }

    #[test]
    fn render_editor_dispatches_to_list() {
        let editor = sample_editor();
        let lines = render_agent_editor(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Agents"));
    }

    #[test]
    fn render_editor_dispatches_to_form() {
        let mut editor = sample_editor();
        editor.edit_selected();
        let lines = render_agent_editor(&editor, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Edit Agent"));
    }
}
