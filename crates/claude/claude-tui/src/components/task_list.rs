//! Task list component for the TUI.
//!
//! Provides rendering for background tasks, async agent tasks, and
//! task management views. Mirrors the task components in
//! `cc-haha/src/components/tasks/`.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`TaskStatus`] | Status of a background task |
//! | [`TaskInfo`] | Information about a single task |
//! | [`TaskListView`] | Current view within the task panel |
//! | [`TaskListPanel`] | Top-level panel state |
//! | [`render_task_list`] | Render the task list |
//! | [`render_task_detail`] | Render a single task detail |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------------

/// Status of a background task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    /// Task is queued and waiting to start.
    Pending,
    /// Task is currently executing.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed(String),
    /// Task was cancelled by the user.
    Cancelled,
}

impl TaskStatus {
    /// Returns the display label.
    pub fn label(&self) -> &str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed(_) => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    /// Returns the indicator character.
    pub fn indicator(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "○",
            TaskStatus::Running => "◐",
            TaskStatus::Completed => "●",
            TaskStatus::Failed(_) => "✗",
            TaskStatus::Cancelled => "⊘",
        }
    }

    /// Returns the color for the status.
    pub fn color(&self) -> Color {
        match self {
            TaskStatus::Pending => Color::DarkGray,
            TaskStatus::Running => Color::Yellow,
            TaskStatus::Completed => Color::Green,
            TaskStatus::Failed(_) => Color::Red,
            TaskStatus::Cancelled => Color::DarkGray,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskInfo
// ---------------------------------------------------------------------------

/// Information about a background task.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable task name.
    pub name: String,
    /// Current status.
    pub status: TaskStatus,
    /// Task type (e.g., "shell", "agent", "workflow").
    pub task_type: String,
    /// Optional progress (0.0 to 1.0).
    pub progress: Option<f64>,
    /// Start time as unix timestamp ms.
    pub started_at: Option<u64>,
    /// Duration in milliseconds (if completed).
    pub duration_ms: Option<u64>,
    /// Number of output lines.
    pub output_lines: usize,
}

// ---------------------------------------------------------------------------
// TaskListView / TaskListPanel
// ---------------------------------------------------------------------------

/// Current view within the task list panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskListView {
    /// Show the list of all tasks.
    List,
    /// Show detail for a specific task.
    Detail { index: usize },
}

/// Top-level task list panel state.
#[derive(Debug, Clone)]
pub struct TaskListPanel {
    /// All tasks.
    pub tasks: Vec<TaskInfo>,
    /// Current view.
    pub view: TaskListView,
    /// Currently selected index.
    pub selected: usize,
    /// Scroll offset.
    pub scroll_offset: usize,
}

impl TaskListPanel {
    /// Create a new task list panel.
    pub fn new(tasks: Vec<TaskInfo>) -> Self {
        Self {
            tasks,
            view: TaskListView::List,
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Enter the selected task's detail view.
    pub fn enter_task(&mut self) {
        if self.selected < self.tasks.len() {
            self.view = TaskListView::Detail {
                index: self.selected,
            };
        }
    }

    /// Go back to the list view.
    pub fn go_back(&mut self) {
        self.view = TaskListView::List;
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.tasks.len() {
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

fn format_duration(ms: u64) -> String {
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

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

/// Render the task list panel.
pub fn render_task_list_panel(panel: &TaskListPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    match &panel.view {
        TaskListView::List => render_task_list(panel, style),
        TaskListView::Detail { index } => render_task_detail(panel, *index, style),
    }
}

/// Render the task list view.
pub fn render_task_list(panel: &TaskListPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Count by status
    let running = panel
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Running)
        .count();
    let completed = panel
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();

    lines.push(Line::from(vec![
        header_span(" Tasks", style),
        dim_span(&format!(
            " ({total} total, {running} running, {completed} done)",
            total = panel.tasks.len()
        )),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.tasks.is_empty() {
        lines.push(Line::from(dim_span("   No background tasks.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(
            "   Tasks will appear here when shell commands run in the background.",
        )));
    } else {
        for (i, task) in panel.tasks.iter().enumerate() {
            let is_selected = i == panel.selected;
            let status_color = task.status.color();

            let mut spans = Vec::new();

            // Selection indicator
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

            // Status indicator
            spans.push(Span::styled(
                task.status.indicator().to_owned(),
                Style::default().fg(status_color),
            ));
            spans.push(Span::styled(" ".to_owned(), Style::default()));

            // Task name
            spans.push(if is_selected {
                Span::styled(
                    task.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(task.name.clone(), Style::default().fg(style.status_fg))
            });

            // Type badge
            spans.push(dim_span(&format!(" [{}]", task.task_type)));

            // Progress
            if let Some(p) = task.progress {
                let pct = (p * 100.0) as usize;
                spans.push(dim_span(&format!(" {pct}%")));
            }

            // Duration
            if let Some(dur) = task.duration_ms {
                spans.push(dim_span(&format!(" ({})", format_duration(dur))));
            }

            // Output lines
            if task.output_lines > 0 {
                spans.push(dim_span(&format!(" {} lines", task.output_lines)));
            }

            lines.push(Line::from(spans));
        }
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter detail │ Esc back │ q close",
    )));

    lines
}

/// Render detail view for a specific task.
pub fn render_task_detail(
    panel: &TaskListPanel,
    index: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let task = match panel.tasks.get(index) {
        Some(t) => t,
        None => {
            lines.push(Line::from("Task not found."));
            return lines;
        }
    };

    // Header
    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(&task.name, style),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    // Details
    lines.push(Line::from(vec![
        Span::styled(
            "  ID:     ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            task.id.clone(),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Status: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            task.status.label().to_owned(),
            Style::default().fg(task.status.color()),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Type:   ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(task.task_type.clone()),
    ]));

    if let Some(p) = task.progress {
        let pct = (p * 100.0) as usize;
        lines.push(Line::from(vec![
            Span::styled(
                "  Progress: ".to_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{pct}%"), Style::default().fg(Color::Cyan)),
        ]));
    }

    if let Some(dur) = task.duration_ms {
        lines.push(Line::from(vec![
            Span::styled(
                "  Duration: ".to_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format_duration(dur)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled(
            "  Output:  ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{} lines", task.output_lines)),
    ]));

    // Error message
    if let TaskStatus::Failed(err) = &task.status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Error:".to_owned(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for line in err.lines() {
            lines.push(Line::from(format!("    {line}")));
        }
    }

    // Footer
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

    fn sample_task(name: &str, status: TaskStatus) -> TaskInfo {
        TaskInfo {
            id: format!("task-{name}"),
            name: name.to_owned(),
            status,
            task_type: "shell".to_owned(),
            progress: None,
            started_at: Some(1000),
            duration_ms: Some(500),
            output_lines: 10,
        }
    }

    fn sample_panel() -> TaskListPanel {
        TaskListPanel::new(vec![
            sample_task("build", TaskStatus::Completed),
            sample_task("test", TaskStatus::Running),
            sample_task("lint", TaskStatus::Failed("clippy error".to_owned())),
        ])
    }

    // -- TaskStatus --

    #[test]
    fn status_label() {
        assert_eq!(TaskStatus::Pending.label(), "pending");
        assert_eq!(TaskStatus::Running.label(), "running");
        assert_eq!(TaskStatus::Completed.label(), "completed");
        assert_eq!(TaskStatus::Failed("".to_owned()).label(), "failed");
        assert_eq!(TaskStatus::Cancelled.label(), "cancelled");
    }

    #[test]
    fn status_indicator() {
        assert_eq!(TaskStatus::Pending.indicator(), "○");
        assert_eq!(TaskStatus::Running.indicator(), "◐");
        assert_eq!(TaskStatus::Completed.indicator(), "●");
        assert_eq!(TaskStatus::Failed("".to_owned()).indicator(), "✗");
        assert_eq!(TaskStatus::Cancelled.indicator(), "⊘");
    }

    #[test]
    fn status_color() {
        assert_eq!(TaskStatus::Pending.color(), Color::DarkGray);
        assert_eq!(TaskStatus::Running.color(), Color::Yellow);
        assert_eq!(TaskStatus::Completed.color(), Color::Green);
        assert_eq!(TaskStatus::Failed("".to_owned()).color(), Color::Red);
    }

    // -- TaskListPanel navigation --

    #[test]
    fn new_panel_starts_at_list() {
        let panel = sample_panel();
        assert_eq!(panel.view, TaskListView::List);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn move_down_increments() {
        let mut panel = sample_panel();
        panel.move_down();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_down_clamps() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_down();
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn move_up_decrements() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_up();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_up_clamps() {
        let mut panel = sample_panel();
        panel.move_up();
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn enter_task_transitions() {
        let mut panel = sample_panel();
        panel.selected = 1;
        panel.enter_task();
        assert_eq!(panel.view, TaskListView::Detail { index: 1 });
    }

    #[test]
    fn go_back_returns_to_list() {
        let mut panel = sample_panel();
        panel.enter_task();
        panel.go_back();
        assert_eq!(panel.view, TaskListView::List);
    }

    // -- Rendering --

    #[test]
    fn render_list_contains_task_names() {
        let panel = sample_panel();
        let lines = render_task_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("build"));
        assert!(combined.contains("test"));
        assert!(combined.contains("lint"));
    }

    #[test]
    fn render_list_shows_counts() {
        let panel = sample_panel();
        let lines = render_task_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("3 total"));
        assert!(combined.contains("1 running"));
        assert!(combined.contains("1 done"));
    }

    #[test]
    fn render_list_empty() {
        let panel = TaskListPanel::new(vec![]);
        let lines = render_task_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No background tasks"));
    }

    #[test]
    fn render_list_shows_progress() {
        let mut panel = sample_panel();
        panel.tasks[1].progress = Some(0.75);
        let lines = render_task_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("75%"));
    }

    #[test]
    fn render_detail_shows_task_info() {
        let panel = sample_panel();
        let lines = render_task_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("build"));
        assert!(combined.contains("completed"));
        assert!(combined.contains("shell"));
    }

    #[test]
    fn render_detail_shows_error() {
        let panel = sample_panel();
        let lines = render_task_detail(&panel, 2, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Error"));
        assert!(combined.contains("clippy error"));
    }

    #[test]
    fn render_detail_invalid() {
        let panel = sample_panel();
        let lines = render_task_detail(&panel, 99, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Task not found"));
    }

    #[test]
    fn render_panel_dispatches_to_list() {
        let panel = sample_panel();
        let lines = render_task_list_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Tasks"));
    }

    #[test]
    fn render_panel_dispatches_to_detail() {
        let mut panel = sample_panel();
        panel.view = TaskListView::Detail { index: 0 };
        let lines = render_task_list_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("build"));
    }

    #[test]
    fn render_detail_shows_duration() {
        let panel = sample_panel();
        let lines = render_task_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("500ms"));
    }

    #[test]
    fn render_detail_shows_output_lines() {
        let panel = sample_panel();
        let lines = render_task_detail(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("10 lines"));
    }
}
