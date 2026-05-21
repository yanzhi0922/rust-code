//! IDE integration for the TUI.
//!
//! Provides an IDE client abstraction for communicating with
//! connected editors (VS Code, JetBrains, etc.) via MCP-like
//! protocol. Supports:
//! - Connection management with status tracking
//! - IDE actions (open file, show diff, apply edit, diagnostics)
//! - Selection and cursor tracking
//! - Connection lifecycle events

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;

// ---------------------------------------------------------------------------
// Connection Status
// ---------------------------------------------------------------------------

/// Status of the IDE connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeConnectionStatus {
    /// No IDE connected.
    Disconnected,
    /// Attempting to connect.
    Connecting,
    /// Successfully connected.
    Connected,
}

impl fmt::Display for IdeConnectionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
        }
    }
}

// ---------------------------------------------------------------------------
// IDE Type
// ---------------------------------------------------------------------------

/// Supported IDE types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdeType {
    /// Visual Studio Code.
    VsCode,
    /// JetBrains IDEs (IntelliJ, WebStorm, etc.).
    JetBrains,
    /// Neovim.
    Neovim,
    /// Unknown IDE.
    Other,
}

impl IdeType {
    /// Parse from IDE name string.
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            n if n.contains("vscode") || n.contains("vs code") => IdeType::VsCode,
            n if n.contains("jetbrains")
                || n.contains("intellij")
                || n.contains("webstorm")
                || n.contains("rustrover") =>
            {
                IdeType::JetBrains
            }
            n if n.contains("neovim") || n.contains("nvim") => IdeType::Neovim,
            _ => IdeType::Other,
        }
    }
}

impl fmt::Display for IdeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VsCode => write!(f, "VS Code"),
            Self::JetBrains => write!(f, "JetBrains"),
            Self::Neovim => write!(f, "Neovim"),
            Self::Other => write!(f, "Other"),
        }
    }
}

// ---------------------------------------------------------------------------
// IDE Actions
// ---------------------------------------------------------------------------

/// Actions that can be sent to the IDE.
#[derive(Debug, Clone, PartialEq)]
pub enum IdeAction {
    /// Open a file at a specific location.
    OpenFile {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
    },
    /// Show a diff view for a file.
    ShowDiff {
        path: PathBuf,
        old_content: String,
        new_content: String,
    },
    /// Apply an edit to a file.
    ApplyEdit { path: PathBuf, content: String },
    /// Get diagnostics for a file.
    GetDiagnostics { path: PathBuf },
    /// Get the current selection from the IDE.
    GetSelection,
    /// Send a notification to the IDE.
    Notify { message: String },
}

// ---------------------------------------------------------------------------
// IDE Responses
// ---------------------------------------------------------------------------

/// Response from an IDE action.
#[derive(Debug, Clone, PartialEq)]
pub enum IdeResponse {
    /// Action completed successfully.
    Ok,
    /// Diagnostics result.
    Diagnostics(Vec<Diagnostic>),
    /// Selection result.
    Selection {
        text: String,
        file_path: Option<PathBuf>,
        start_line: Option<usize>,
        end_line: Option<usize>,
    },
    /// Error from the IDE.
    Error(String),
}

// ---------------------------------------------------------------------------
// Diagnostic
// ---------------------------------------------------------------------------

/// A diagnostic item from the IDE.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// File path.
    pub file_path: PathBuf,
    /// Line number (1-based).
    pub line: usize,
    /// Column number (1-based).
    pub column: usize,
    /// Severity level.
    pub severity: DiagnosticSeverity,
    /// Diagnostic message.
    pub message: String,
    /// Source of the diagnostic (e.g. "rustc", "eslint").
    pub source: Option<String>,
}

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    /// Informational.
    Hint,
    /// Warning.
    Warning,
    /// Error.
    Error,
}

impl fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hint => write!(f, "hint"),
            Self::Warning => write!(f, "warning"),
            Self::Error => write!(f, "error"),
        }
    }
}

// ---------------------------------------------------------------------------
// IDE Client
// ---------------------------------------------------------------------------

/// Client for communicating with a connected IDE.
///
/// Tracks connection state and provides methods to send actions.
#[derive(Debug, Clone)]
pub struct IdeClient {
    /// The IDE type.
    ide_type: IdeType,
    /// Current connection status.
    status: IdeConnectionStatus,
    /// Connection URL (if applicable).
    url: Option<String>,
    /// Authentication token.
    auth_token: Option<String>,
    /// Last activity timestamp.
    last_activity: Option<Instant>,
    /// Number of actions sent.
    action_count: usize,
    /// Pending action queue (for testing / simulation).
    pending_actions: Vec<IdeAction>,
    /// Whether the IDE is running in Windows.
    ide_running_in_windows: bool,
}

impl IdeClient {
    /// Create a new disconnected IDE client.
    pub fn new(ide_type: IdeType) -> Self {
        IdeClient {
            ide_type,
            status: IdeConnectionStatus::Disconnected,
            url: None,
            auth_token: None,
            last_activity: None,
            action_count: 0,
            pending_actions: Vec::new(),
            ide_running_in_windows: false,
        }
    }

    /// Get the IDE type.
    pub fn ide_type(&self) -> IdeType {
        self.ide_type
    }

    /// Get the connection status.
    pub fn status(&self) -> IdeConnectionStatus {
        self.status
    }

    /// Get the connection URL.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Get the number of actions sent.
    pub fn action_count(&self) -> usize {
        self.action_count
    }

    /// Check if the IDE is connected.
    pub fn is_connected(&self) -> bool {
        self.status == IdeConnectionStatus::Connected
    }

    /// Attempt to connect to the IDE.
    pub fn connect(&mut self, url: String, auth_token: Option<String>) -> Result<()> {
        if self.status == IdeConnectionStatus::Connected {
            return Err(anyhow::anyhow!("already connected to IDE"));
        }
        self.status = IdeConnectionStatus::Connecting;
        self.url = Some(url);
        self.auth_token = auth_token;
        // Simulate successful connection.
        self.status = IdeConnectionStatus::Connected;
        self.last_activity = Some(Instant::now());
        Ok(())
    }

    /// Disconnect from the IDE.
    pub fn disconnect(&mut self) {
        self.status = IdeConnectionStatus::Disconnected;
        self.url = None;
        self.auth_token = None;
        self.last_activity = None;
    }

    /// Send an action to the IDE.
    pub fn send_action(&mut self, action: IdeAction) -> Result<IdeResponse> {
        if self.status != IdeConnectionStatus::Connected {
            return Err(anyhow::anyhow!(
                "IDE not connected (status: {})",
                self.status
            ));
        }
        self.action_count += 1;
        self.last_activity = Some(Instant::now());
        self.pending_actions.push(action.clone());

        // Simulate responses based on action type.
        let response = match &action {
            IdeAction::GetDiagnostics { .. } => IdeResponse::Diagnostics(vec![]),
            IdeAction::GetSelection => IdeResponse::Selection {
                text: String::new(),
                file_path: None,
                start_line: None,
                end_line: None,
            },
            _ => IdeResponse::Ok,
        };
        Ok(response)
    }

    /// Get the current selection from the IDE.
    pub fn get_selection(&mut self) -> Result<IdeResponse> {
        self.send_action(IdeAction::GetSelection)
    }

    /// Open a file in the IDE.
    pub fn open_file(&mut self, path: PathBuf, line: Option<usize>) -> Result<IdeResponse> {
        self.send_action(IdeAction::OpenFile {
            path,
            line,
            column: None,
        })
    }

    /// Show a diff in the IDE.
    pub fn show_diff(
        &mut self,
        path: PathBuf,
        old_content: String,
        new_content: String,
    ) -> Result<IdeResponse> {
        self.send_action(IdeAction::ShowDiff {
            path,
            old_content,
            new_content,
        })
    }

    /// Apply an edit in the IDE.
    pub fn apply_edit(&mut self, path: PathBuf, content: String) -> Result<IdeResponse> {
        self.send_action(IdeAction::ApplyEdit { path, content })
    }

    /// Get diagnostics for a file.
    pub fn get_diagnostics(&mut self, path: PathBuf) -> Result<IdeResponse> {
        self.send_action(IdeAction::GetDiagnostics { path })
    }

    /// Get pending actions (for testing).
    pub fn pending_actions(&self) -> &[IdeAction] {
        &self.pending_actions
    }

    /// Clear pending actions.
    pub fn clear_pending(&mut self) {
        self.pending_actions.clear();
    }

    /// Time since last activity.
    pub fn idle_duration(&self) -> Option<Duration> {
        self.last_activity.map(|t| t.elapsed())
    }

    /// Set the IDE running-in-Windows flag.
    pub fn set_running_in_windows(&mut self, value: bool) {
        self.ide_running_in_windows = value;
    }

    /// Check if the IDE is running in Windows.
    pub fn is_running_in_windows(&self) -> bool {
        self.ide_running_in_windows
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_ide_type_from_name() {
        assert_eq!(IdeType::from_name("vscode"), IdeType::VsCode);
        assert_eq!(IdeType::from_name("VS Code"), IdeType::VsCode);
        assert_eq!(IdeType::from_name("JetBrains IntelliJ"), IdeType::JetBrains);
        assert_eq!(IdeType::from_name("neovim"), IdeType::Neovim);
        assert_eq!(IdeType::from_name("emacs"), IdeType::Other);
    }

    #[test]
    fn test_ide_type_display() {
        assert_eq!(IdeType::VsCode.to_string(), "VS Code");
        assert_eq!(IdeType::JetBrains.to_string(), "JetBrains");
        assert_eq!(IdeType::Neovim.to_string(), "Neovim");
    }

    #[test]
    fn test_connection_status_display() {
        assert_eq!(
            IdeConnectionStatus::Disconnected.to_string(),
            "Disconnected"
        );
        assert_eq!(IdeConnectionStatus::Connecting.to_string(), "Connecting");
        assert_eq!(IdeConnectionStatus::Connected.to_string(), "Connected");
    }

    #[test]
    fn test_client_new() {
        let client = IdeClient::new(IdeType::VsCode);
        assert_eq!(client.ide_type(), IdeType::VsCode);
        assert_eq!(client.status(), IdeConnectionStatus::Disconnected);
        assert!(!client.is_connected());
        assert_eq!(client.action_count(), 0);
    }

    #[test]
    fn test_client_connect() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), Some("token".to_string()))
            .expect("connect");
        assert!(client.is_connected());
        assert_eq!(client.url(), Some("ws://localhost:1234"));
    }

    #[test]
    fn test_client_connect_already_connected() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.connect("ws://other:5678".to_string(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_client_disconnect() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        client.disconnect();
        assert!(!client.is_connected());
        assert_eq!(client.url(), None);
    }

    #[test]
    fn test_client_send_action_disconnected() {
        let mut client = IdeClient::new(IdeType::VsCode);
        let result = client.open_file(PathBuf::from("test.rs"), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_client_open_file() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.open_file(PathBuf::from("src/main.rs"), Some(42));
        assert!(result.is_ok());
        assert_eq!(client.action_count(), 1);
    }

    #[test]
    fn test_client_show_diff() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.show_diff(
            PathBuf::from("src/main.rs"),
            "old".to_string(),
            "new".to_string(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_client_apply_edit() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.apply_edit(PathBuf::from("src/main.rs"), "new content".to_string());
        assert!(result.is_ok());
        assert_eq!(result.expect("response"), IdeResponse::Ok);
    }

    #[test]
    fn test_client_get_selection() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.get_selection().expect("selection");
        match result {
            IdeResponse::Selection { text, .. } => assert!(text.is_empty()),
            other => panic!("expected Selection, got {other:?}"),
        }
    }

    #[test]
    fn test_client_get_diagnostics() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        let result = client.get_diagnostics(PathBuf::from("src/main.rs"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_client_pending_actions() {
        let mut client = IdeClient::new(IdeType::VsCode);
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        client.open_file(PathBuf::from("a.rs"), None).expect("ok");
        client.open_file(PathBuf::from("b.rs"), None).expect("ok");
        assert_eq!(client.pending_actions().len(), 2);
        client.clear_pending();
        assert!(client.pending_actions().is_empty());
    }

    #[test]
    fn test_client_idle_duration() {
        let mut client = IdeClient::new(IdeType::VsCode);
        assert!(client.idle_duration().is_none());
        client
            .connect("ws://localhost:1234".to_string(), None)
            .expect("connect");
        assert!(client.idle_duration().is_some());
    }

    #[test]
    fn test_client_running_in_windows() {
        let mut client = IdeClient::new(IdeType::VsCode);
        assert!(!client.is_running_in_windows());
        client.set_running_in_windows(true);
        assert!(client.is_running_in_windows());
    }

    #[test]
    fn test_diagnostic_severity_display() {
        assert_eq!(DiagnosticSeverity::Hint.to_string(), "hint");
        assert_eq!(DiagnosticSeverity::Warning.to_string(), "warning");
        assert_eq!(DiagnosticSeverity::Error.to_string(), "error");
    }

    #[test]
    fn test_ide_action_equality() {
        let a = IdeAction::OpenFile {
            path: PathBuf::from("test.rs"),
            line: Some(1),
            column: None,
        };
        let b = IdeAction::OpenFile {
            path: PathBuf::from("test.rs"),
            line: Some(1),
            column: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_diagnostic_construction() {
        let diag = Diagnostic {
            file_path: PathBuf::from("src/main.rs"),
            line: 10,
            column: 5,
            severity: DiagnosticSeverity::Error,
            message: "expected `;`".to_string(),
            source: Some("rustc".to_string()),
        };
        assert_eq!(diag.line, 10);
        assert_eq!(diag.severity, DiagnosticSeverity::Error);
        assert_eq!(diag.source.as_deref(), Some("rustc"));
    }
}
