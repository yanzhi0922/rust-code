//! MCP (Model Context Protocol) settings panel for the TUI.
//!
//! Provides a full-screen panel for managing MCP server connections,
//! viewing available tools, and configuring server settings.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`McpServerInfo`] | Server connection info (name, type, status, tools) |
//! | [`McpViewState`] | Current view within the MCP panel |
//! | [`McpPanel`] | Top-level panel state |
//! | [`render_mcp_panel`] | Render the MCP panel into lines |
//! | [`render_mcp_server_list`] | Render server list view |
//! | [`render_mcp_tool_list`] | Render tool list for a server |
//! | [`render_mcp_tool_detail`] | Render detailed tool info |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// McpServerInfo
// ---------------------------------------------------------------------------

/// Connection type for an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerType {
    /// Local stdio-based server (spawned as child process).
    Stdio,
    /// Remote SSE (Server-Sent Events) server.
    Sse,
    /// Remote HTTP-based server.
    Http,
    /// Claude.ai proxy server.
    ClaudeAiProxy,
}

impl std::fmt::Display for McpServerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpServerType::Stdio => write!(f, "stdio"),
            McpServerType::Sse => write!(f, "sse"),
            McpServerType::Http => write!(f, "http"),
            McpServerType::ClaudeAiProxy => write!(f, "claudeai-proxy"),
        }
    }
}

/// Connection status of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStatus {
    /// Successfully connected and tools discovered.
    Connected,
    /// Currently connecting or reconnecting.
    Connecting,
    /// Connection failed or timed out.
    Disconnected,
    /// Authentication required but not completed.
    AuthRequired,
}

impl McpServerStatus {
    /// Returns the status indicator character.
    pub fn indicator(&self) -> &'static str {
        match self {
            McpServerStatus::Connected => "●",
            McpServerStatus::Connecting => "◐",
            McpServerStatus::Disconnected => "○",
            McpServerStatus::AuthRequired => "🔒",
        }
    }

    /// Returns the color for the status indicator.
    pub fn color(&self) -> Color {
        match self {
            McpServerStatus::Connected => Color::Green,
            McpServerStatus::Connecting => Color::Yellow,
            McpServerStatus::Disconnected => Color::Red,
            McpServerStatus::AuthRequired => Color::Magenta,
        }
    }
}

/// Information about an MCP tool.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Tool name (e.g., "search_files").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the tool requires user approval.
    pub requires_approval: bool,
}

/// Information about an MCP server.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    /// Server display name.
    pub name: String,
    /// Connection type.
    pub server_type: McpServerType,
    /// Current connection status.
    pub status: McpServerStatus,
    /// List of tools provided by this server.
    pub tools: Vec<McpToolInfo>,
    /// Configuration scope (project, user, etc.).
    pub scope: String,
    /// Whether the server is authenticated (for remote servers).
    pub is_authenticated: Option<bool>,
}

// ---------------------------------------------------------------------------
// McpViewState
// ---------------------------------------------------------------------------

/// Current view within the MCP settings panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpViewState {
    /// Show the list of all servers.
    List,
    /// Show tools for a specific server (index into servers list).
    ToolList { server_index: usize },
    /// Show detailed info for a specific tool.
    ToolDetail {
        server_index: usize,
        tool_index: usize,
    },
}

// ---------------------------------------------------------------------------
// McpPanel
// ---------------------------------------------------------------------------

/// Top-level MCP settings panel state.
#[derive(Debug, Clone)]
pub struct McpPanel {
    /// All known MCP servers.
    pub servers: Vec<McpServerInfo>,
    /// Current view state.
    pub view: McpViewState,
    /// Currently selected item index (for navigation).
    pub selected: usize,
    /// Scroll offset for long lists.
    pub scroll_offset: usize,
}

impl McpPanel {
    /// Create a new MCP panel starting at the server list view.
    pub fn new(servers: Vec<McpServerInfo>) -> Self {
        Self {
            servers,
            view: McpViewState::List,
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Navigate into the selected server's tool list.
    pub fn enter_server(&mut self) {
        if self.selected < self.servers.len() {
            self.view = McpViewState::ToolList {
                server_index: self.selected,
            };
            self.selected = 0;
            self.scroll_offset = 0;
        }
    }

    /// Navigate into the selected tool's detail view.
    pub fn enter_tool(&mut self) {
        if let McpViewState::ToolList { server_index } = self.view
            && let Some(server) = self.servers.get(server_index)
            && self.selected < server.tools.len()
        {
            self.view = McpViewState::ToolDetail {
                server_index,
                tool_index: self.selected,
            };
        }
    }

    /// Go back to the previous view.
    pub fn go_back(&mut self) {
        self.view = match &self.view {
            McpViewState::List => McpViewState::List,
            McpViewState::ToolList { .. } => McpViewState::List,
            McpViewState::ToolDetail { server_index, .. } => McpViewState::ToolList {
                server_index: *server_index,
            },
        };
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = match &self.view {
            McpViewState::List => self.servers.len(),
            McpViewState::ToolList { server_index } => {
                self.servers.get(*server_index).map_or(0, |s| s.tools.len())
            }
            McpViewState::ToolDetail { .. } => 0,
        };
        if self.selected + 1 < max {
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

/// Render the MCP settings panel.
pub fn render_mcp_panel(panel: &McpPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    match &panel.view {
        McpViewState::List => render_mcp_server_list(panel, style),
        McpViewState::ToolList { server_index } => {
            render_mcp_tool_list(panel, *server_index, style)
        }
        McpViewState::ToolDetail {
            server_index,
            tool_index,
        } => render_mcp_tool_detail(panel, *server_index, *tool_index, style),
    }
}

/// Render the server list view.
pub fn render_mcp_server_list(panel: &McpPanel, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Header
    lines.push(Line::from(vec![
        header_span(" MCP Servers", style),
        dim_span(&format!(" ({})", panel.servers.len())),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if panel.servers.is_empty() {
        lines.push(Line::from(dim_span("   No MCP servers configured.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(
            "   Add servers in your settings file or via /mcp command.",
        )));
    } else {
        for (i, server) in panel.servers.iter().enumerate() {
            let is_selected = i == panel.selected;
            let status_color = server.status.color();

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

            // Status dot
            spans.push(Span::styled(
                server.status.indicator().to_owned(),
                Style::default().fg(status_color),
            ));

            spans.push(Span::styled(" ".to_owned(), Style::default()));

            // Server name
            if is_selected {
                spans.push(Span::styled(
                    server.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    server.name.clone(),
                    Style::default().fg(style.status_fg),
                ));
            }

            // Type badge
            spans.push(Span::styled(
                format!(" [{}]", server.server_type),
                Style::default().add_modifier(Modifier::DIM),
            ));

            // Tool count
            let tool_count = server.tools.len();
            if tool_count > 0 {
                spans.push(Span::styled(
                    format!(
                        " {tool_count} tool{}",
                        if tool_count != 1 { "s" } else { "" }
                    ),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }

            // Auth status for remote servers
            if let Some(auth) = server.is_authenticated
                && !auth
            {
                spans.push(Span::styled(
                    " ⚠ not authenticated".to_owned(),
                    Style::default().fg(Color::Yellow),
                ));
            }

            // Scope
            spans.push(Span::styled(
                format!(" ({})", server.scope),
                Style::default().add_modifier(Modifier::DIM),
            ));

            lines.push(Line::from(spans));
        }
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter view tools │ Esc back │ q close",
    )));

    lines
}

/// Render the tool list for a specific server.
pub fn render_mcp_tool_list(
    panel: &McpPanel,
    server_index: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let server = match panel.servers.get(server_index) {
        Some(s) => s,
        None => {
            lines.push(Line::from("Server not found."));
            return lines;
        }
    };

    // Header
    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(&format!("{} — Tools", server.name), style),
        dim_span(&format!(" ({})", server.tools.len())),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    if server.tools.is_empty() {
        lines.push(Line::from(dim_span("   No tools available.")));
        lines.push(Line::from(""));
        lines.push(Line::from(dim_span(
            "   The server may still be connecting or has not exposed any tools.",
        )));
    } else {
        for (i, tool) in server.tools.iter().enumerate() {
            let is_selected = i == panel.selected;

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

            // Tool name
            if is_selected {
                spans.push(Span::styled(
                    tool.name.clone(),
                    Style::default()
                        .fg(style.accent_color)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    tool.name.clone(),
                    Style::default().fg(style.status_fg),
                ));
            }

            // Approval badge
            if tool.requires_approval {
                spans.push(Span::styled(
                    " [requires approval]".to_owned(),
                    Style::default().fg(Color::Yellow),
                ));
            }

            lines.push(Line::from(spans));

            // Description (truncated)
            if !tool.description.is_empty() {
                let desc_preview = if tool.description.len() > 60 {
                    format!("{}…", &tool.description[..59])
                } else {
                    tool.description.clone()
                };
                lines.push(Line::from(dim_span(&format!("     {desc_preview}"))));
            }
        }
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ navigate │ Enter tool detail │ Esc back",
    )));

    lines
}

/// Render detailed info for a specific tool.
pub fn render_mcp_tool_detail(
    panel: &McpPanel,
    server_index: usize,
    tool_index: usize,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let server = match panel.servers.get(server_index) {
        Some(s) => s,
        None => {
            lines.push(Line::from("Server not found."));
            return lines;
        }
    };

    let tool = match server.tools.get(tool_index) {
        Some(t) => t,
        None => {
            lines.push(Line::from("Tool not found."));
            return lines;
        }
    };

    // Header
    lines.push(Line::from(vec![
        Span::styled(" ◀ ".to_owned(), Style::default().fg(style.accent_color)),
        header_span(&format!("{} → {}", server.name, tool.name), style),
    ]));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    // Tool info
    lines.push(Line::from(vec![
        Span::styled(
            "  Name:  ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(tool.name.clone(), Style::default().fg(style.accent_color)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Server: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(server.name.clone()),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Type:  ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(server.server_type.to_string()),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            "  Approval: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        if tool.requires_approval {
            Span::styled("required".to_owned(), Style::default().fg(Color::Yellow))
        } else {
            Span::styled("auto".to_owned(), Style::default().fg(Color::Green))
        },
    ]));

    lines.push(Line::from(""));

    // Description
    if !tool.description.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Description:".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for line in tool.description.lines() {
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

    fn sample_server(name: &str, tool_count: usize) -> McpServerInfo {
        McpServerInfo {
            name: name.to_owned(),
            server_type: McpServerType::Stdio,
            status: McpServerStatus::Connected,
            tools: (0..tool_count)
                .map(|i| McpToolInfo {
                    name: format!("tool_{i}"),
                    description: format!("Tool number {i}"),
                    requires_approval: i % 2 == 0,
                })
                .collect(),
            scope: "project".to_owned(),
            is_authenticated: None,
        }
    }

    fn sample_panel() -> McpPanel {
        McpPanel::new(vec![
            sample_server("filesystem", 3),
            sample_server("github", 2),
            McpServerInfo {
                name: "remote-api".to_owned(),
                server_type: McpServerType::Sse,
                status: McpServerStatus::AuthRequired,
                tools: vec![],
                scope: "user".to_owned(),
                is_authenticated: Some(false),
            },
        ])
    }

    // -- McpServerType Display --

    #[test]
    fn server_type_display() {
        assert_eq!(McpServerType::Stdio.to_string(), "stdio");
        assert_eq!(McpServerType::Sse.to_string(), "sse");
        assert_eq!(McpServerType::Http.to_string(), "http");
        assert_eq!(McpServerType::ClaudeAiProxy.to_string(), "claudeai-proxy");
    }

    // -- McpServerStatus --

    #[test]
    fn status_indicator_connected() {
        assert_eq!(McpServerStatus::Connected.indicator(), "●");
    }

    #[test]
    fn status_indicator_connecting() {
        assert_eq!(McpServerStatus::Connecting.indicator(), "◐");
    }

    #[test]
    fn status_indicator_disconnected() {
        assert_eq!(McpServerStatus::Disconnected.indicator(), "○");
    }

    #[test]
    fn status_indicator_auth_required() {
        assert_eq!(McpServerStatus::AuthRequired.indicator(), "🔒");
    }

    #[test]
    fn status_color_connected() {
        assert_eq!(McpServerStatus::Connected.color(), Color::Green);
    }

    #[test]
    fn status_color_disconnected() {
        assert_eq!(McpServerStatus::Disconnected.color(), Color::Red);
    }

    // -- McpPanel navigation --

    #[test]
    fn new_panel_starts_at_list() {
        let panel = sample_panel();
        assert_eq!(panel.view, McpViewState::List);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn move_down_increments_selected() {
        let mut panel = sample_panel();
        panel.move_down();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_down_clamps_at_max() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_down();
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn move_up_decrements_selected() {
        let mut panel = sample_panel();
        panel.selected = 2;
        panel.move_up();
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let mut panel = sample_panel();
        panel.move_up();
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn enter_server_transitions_to_tool_list() {
        let mut panel = sample_panel();
        panel.selected = 1;
        panel.enter_server();
        assert_eq!(panel.view, McpViewState::ToolList { server_index: 1 });
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn enter_tool_transitions_to_tool_detail() {
        let mut panel = sample_panel();
        panel.enter_server(); // Enter first server
        panel.selected = 1;
        panel.enter_tool();
        assert!(matches!(
            panel.view,
            McpViewState::ToolDetail {
                server_index: 0,
                tool_index: 1
            }
        ));
    }

    #[test]
    fn go_back_from_tool_list_goes_to_list() {
        let mut panel = sample_panel();
        panel.enter_server();
        panel.go_back();
        assert_eq!(panel.view, McpViewState::List);
    }

    #[test]
    fn go_back_from_tool_detail_goes_to_tool_list() {
        let mut panel = sample_panel();
        panel.enter_server();
        panel.enter_tool();
        panel.go_back();
        assert!(matches!(panel.view, McpViewState::ToolList { .. }));
    }

    // -- Rendering --

    #[test]
    fn render_server_list_contains_server_names() {
        let panel = sample_panel();
        let lines = render_mcp_server_list(&panel, &test_style());
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let combined = text.join("\n");
        assert!(combined.contains("filesystem"));
        assert!(combined.contains("github"));
        assert!(combined.contains("remote-api"));
    }

    #[test]
    fn render_server_list_shows_tool_count() {
        let panel = sample_panel();
        let lines = render_mcp_server_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("3 tools"));
        assert!(combined.contains("2 tools"));
    }

    #[test]
    fn render_server_list_empty() {
        let panel = McpPanel::new(vec![]);
        let lines = render_mcp_server_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No MCP servers configured"));
    }

    #[test]
    fn render_tool_list_shows_tool_names() {
        let panel = sample_panel();
        let lines = render_mcp_tool_list(&panel, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("tool_0"));
        assert!(combined.contains("tool_1"));
        assert!(combined.contains("tool_2"));
    }

    #[test]
    fn render_tool_list_empty_server() {
        let panel = sample_panel();
        let lines = render_mcp_tool_list(&panel, 2, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("No tools available"));
    }

    #[test]
    fn render_tool_detail_shows_name_and_description() {
        let panel = sample_panel();
        let lines = render_mcp_tool_detail(&panel, 0, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("tool_0"));
        assert!(combined.contains("Tool number 0"));
    }

    #[test]
    fn render_tool_detail_approval_badge() {
        let panel = sample_panel();
        let lines = render_mcp_tool_detail(&panel, 0, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("required"));
    }

    #[test]
    fn render_tool_detail_auto_approval() {
        let panel = sample_panel();
        let lines = render_mcp_tool_detail(&panel, 0, 1, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("auto"));
    }

    #[test]
    fn render_tool_detail_invalid_server() {
        let panel = sample_panel();
        let lines = render_mcp_tool_detail(&panel, 99, 0, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Server not found"));
    }

    #[test]
    fn render_tool_detail_invalid_tool() {
        let panel = sample_panel();
        let lines = render_mcp_tool_detail(&panel, 0, 99, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Tool not found"));
    }

    #[test]
    fn render_panel_dispatches_to_list() {
        let panel = sample_panel();
        let lines = render_mcp_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("MCP Servers"));
    }

    #[test]
    fn render_panel_dispatches_to_tool_list() {
        let mut panel = sample_panel();
        panel.view = McpViewState::ToolList { server_index: 0 };
        let lines = render_mcp_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("filesystem"));
        assert!(combined.contains("tool_0"));
    }

    #[test]
    fn render_panel_dispatches_to_tool_detail() {
        let mut panel = sample_panel();
        panel.view = McpViewState::ToolDetail {
            server_index: 0,
            tool_index: 0,
        };
        let lines = render_mcp_panel(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("tool_0"));
    }

    #[test]
    fn auth_required_shows_warning() {
        let panel = McpPanel::new(vec![McpServerInfo {
            name: "auth-server".to_owned(),
            server_type: McpServerType::Sse,
            status: McpServerStatus::AuthRequired,
            tools: vec![],
            scope: "user".to_owned(),
            is_authenticated: Some(false),
        }]);
        let lines = render_mcp_server_list(&panel, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("not authenticated"));
    }
}
