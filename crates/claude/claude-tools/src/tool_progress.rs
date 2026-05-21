//! Tool progress data types for real-time execution monitoring.
//!
//! Provides 10 progress kinds that tools can emit during execution to give
//! the UI rich, structured feedback: spinners, progress bars, file lists,
//! token counts, search results, build output, test results, LSP diagnostics,
//! MCP progress, and custom payloads.

use parking_lot::Mutex;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ToolProgressData – the 10 progress kinds
// ---------------------------------------------------------------------------

/// Tagged enum covering all structured progress payloads a tool can emit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolProgressData {
    /// A simple spinner with an optional status message.
    Spinner {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A determinate progress bar (0.0 – 1.0).
    ProgressBar {
        progress: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A list of files being processed.
    FileList {
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_index: Option<usize>,
    },
    /// Token counting progress (input / output tokens).
    TokenCount {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_estimate: Option<u64>,
    },
    /// Search results returned so far.
    SearchResults {
        query: String,
        results_so_far: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_estimate: Option<usize>,
    },
    /// Build / compilation output.
    BuildOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        errors: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warnings: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Test execution results.
    TestResults {
        passed: usize,
        failed: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skipped: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// LSP diagnostic summary.
    LSPDiagnostic {
        errors: usize,
        warnings: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        infos: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
    },
    /// MCP server progress notification.
    MCPProgress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_name: Option<String>,
    },
    /// Arbitrary custom progress payload.
    Custom {
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl ToolProgressData {
    /// Return the discriminant name (e.g. `"spinner"`, `"progress_bar"`).
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Spinner { .. } => "spinner",
            Self::ProgressBar { .. } => "progress_bar",
            Self::FileList { .. } => "file_list",
            Self::TokenCount { .. } => "token_count",
            Self::SearchResults { .. } => "search_results",
            Self::BuildOutput { .. } => "build_output",
            Self::TestResults { .. } => "test_results",
            Self::LSPDiagnostic { .. } => "lsp_diagnostic",
            Self::MCPProgress { .. } => "mcp_progress",
            Self::Custom { .. } => "custom",
        }
    }

    /// If this is a `ProgressBar`, return its fraction in `[0, 1]`.
    #[must_use]
    pub fn progress_fraction(&self) -> Option<f64> {
        match self {
            Self::ProgressBar { progress, .. } => Some(*progress),
            Self::MCPProgress {
                progress: Some(p), ..
            } => Some(*p),
            _ => None,
        }
    }

    /// Return the optional message field common to most variants.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Spinner { message } => message.as_deref(),
            Self::ProgressBar { message, .. } => message.as_deref(),
            Self::BuildOutput { message, .. } => message.as_deref(),
            Self::TestResults { message, .. } => message.as_deref(),
            Self::MCPProgress { message, .. } => message.as_deref(),
            Self::Custom { detail, .. } => detail.as_deref(),
            _ => None,
        }
    }

    /// Whether the progress represents an error-heavy state.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        match self {
            Self::BuildOutput {
                errors: Some(e), ..
            } => *e > 0,
            Self::TestResults { failed, .. } => *failed > 0,
            Self::LSPDiagnostic { errors, .. } => *errors > 0,
            _ => false,
        }
    }
}

impl fmt::Display for ToolProgressData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spinner { message } => {
                write!(f, "⠋")?;
                if let Some(msg) = message {
                    write!(f, " {msg}")?;
                }
                Ok(())
            }
            Self::ProgressBar {
                progress, message, ..
            } => {
                let pct = (*progress * 100.0).clamp(0.0, 100.0) as u8;
                write!(f, "[{pct}%]")?;
                if let Some(msg) = message {
                    write!(f, " {msg}")?;
                }
                Ok(())
            }
            Self::FileList {
                files,
                current_index,
            } => {
                let idx = current_index.unwrap_or(0);
                write!(f, "Files {idx}/{}", files.len())
            }
            Self::TokenCount {
                input_tokens,
                output_tokens,
                total_estimate,
            } => {
                write!(f, "Tokens: {input_tokens}in/{output_tokens}out")?;
                if let Some(total) = total_estimate {
                    write!(f, " (~{total} total)")?;
                }
                Ok(())
            }
            Self::SearchResults {
                query,
                results_so_far,
                total_estimate,
            } => {
                write!(f, "Search \"{query}\": {results_so_far} results")?;
                if let Some(total) = total_estimate {
                    write!(f, " (~{total} total)")?;
                }
                Ok(())
            }
            Self::BuildOutput {
                errors,
                warnings,
                message,
            } => {
                if let Some(e) = errors {
                    write!(f, "{e} errors")?;
                }
                if let Some(w) = warnings {
                    write!(f, " {w} warnings")?;
                }
                if let Some(msg) = message {
                    write!(f, " – {msg}")?;
                }
                Ok(())
            }
            Self::TestResults {
                passed,
                failed,
                skipped,
                message,
            } => {
                write!(f, "Tests: {passed} passed, {failed} failed")?;
                if let Some(s) = skipped {
                    write!(f, ", {s} skipped")?;
                }
                if let Some(msg) = message {
                    write!(f, " – {msg}")?;
                }
                Ok(())
            }
            Self::LSPDiagnostic {
                errors,
                warnings,
                infos,
                file_path,
            } => {
                write!(f, "{errors} errors, {warnings} warnings")?;
                if let Some(i) = infos {
                    write!(f, ", {i} infos")?;
                }
                if let Some(fp) = file_path {
                    write!(f, " in {fp}")?;
                }
                Ok(())
            }
            Self::MCPProgress {
                progress,
                message,
                server_name,
            } => {
                if let Some(srv) = server_name {
                    write!(f, "[{srv}] ")?;
                }
                if let Some(p) = progress {
                    write!(f, "{:.0}%", p * 100.0)?;
                }
                if let Some(msg) = message {
                    write!(f, " {msg}")?;
                }
                Ok(())
            }
            Self::Custom { label, detail } => {
                write!(f, "{label}")?;
                if let Some(d) = detail {
                    write!(f, ": {d}")?;
                }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Progress callback type
// ---------------------------------------------------------------------------

/// Callback invoked every time a tool emits progress.
pub type ProgressCallback = Box<dyn Fn(&str, &ToolProgressData) + Send + Sync>;

// ---------------------------------------------------------------------------
// ProgressStream – shared-progress sink
// ---------------------------------------------------------------------------

/// A thread-safe sink that collects [`ToolProgressData`] events and
/// optionally forwards them to a registered callback.
#[derive(Debug, Clone)]
pub struct ProgressStream {
    inner: Arc<Mutex<ProgressStreamInner>>,
}

struct ProgressStreamInner {
    events: Vec<(String, ToolProgressData)>,
    callback: Option<ProgressCallback>,
    closed: bool,
}

impl std::fmt::Debug for ProgressStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressStreamInner")
            .field("events", &self.events.len())
            .field("callback", &self.callback.as_ref().map(|_| "..."))
            .field("closed", &self.closed)
            .finish()
    }
}

impl ProgressStream {
    /// Create an empty progress stream without a callback.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProgressStreamInner {
                events: Vec::new(),
                callback: None,
                closed: false,
            })),
        }
    }

    /// Create a progress stream that forwards events to `callback`.
    pub fn with_callback(callback: ProgressCallback) -> Self {
        let mut inner = ProgressStreamInner {
            events: Vec::new(),
            callback: Some(callback),
            closed: false,
        };
        // We can't know the exact type erasure worked until runtime,
        // but the Box is already the right type.
        let _ = &mut inner.callback;
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Emit a progress event for the given tool.
    pub fn emit(&self, tool_call_id: &str, data: ToolProgressData) {
        let mut inner = self.inner.lock();
        if inner.closed {
            return;
        }
        if let Some(ref cb) = inner.callback {
            cb(tool_call_id, &data);
        }
        inner.events.push((tool_call_id.to_owned(), data));
    }

    /// Drain all buffered events, returning them as a vector.
    pub fn drain(&self) -> Vec<(String, ToolProgressData)> {
        let mut inner = self.inner.lock();
        std::mem::take(&mut inner.events)
    }

    /// Return the number of buffered events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().events.len()
    }

    /// Return `true` if no events have been buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Close the stream – subsequent [`emit`](Self::emit) calls are no-ops.
    pub fn close(&self) {
        self.inner.lock().closed = true;
    }

    /// Whether the stream has been closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

impl Default for ProgressStream {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ToolProgressData construction & kind_name -------------------------

    #[test]
    fn spinner_kind_name() {
        let d = ToolProgressData::Spinner {
            message: Some("loading".into()),
        };
        assert_eq!(d.kind_name(), "spinner");
    }

    #[test]
    fn progress_bar_fraction() {
        let d = ToolProgressData::ProgressBar {
            progress: 0.42,
            message: None,
        };
        assert!((d.progress_fraction().expect("frac") - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn file_list_kind() {
        let d = ToolProgressData::FileList {
            files: vec!["a.rs".into(), "b.rs".into()],
            current_index: Some(0),
        };
        assert_eq!(d.kind_name(), "file_list");
        assert!(d.progress_fraction().is_none());
    }

    #[test]
    fn token_count_message_is_none() {
        let d = ToolProgressData::TokenCount {
            input_tokens: 100,
            output_tokens: 50,
            total_estimate: None,
        };
        assert!(d.message().is_none());
    }

    #[test]
    fn search_results_message() {
        let d = ToolProgressData::SearchResults {
            query: "fn main".into(),
            results_so_far: 5,
            total_estimate: Some(10),
        };
        assert!(d.message().is_none());
    }

    #[test]
    fn build_output_has_errors_true() {
        let d = ToolProgressData::BuildOutput {
            errors: Some(3),
            warnings: Some(1),
            message: None,
        };
        assert!(d.has_errors());
        assert_eq!(d.message(), None);
    }

    #[test]
    fn build_output_has_errors_false() {
        let d = ToolProgressData::BuildOutput {
            errors: Some(0),
            warnings: None,
            message: Some("ok".into()),
        };
        assert!(!d.has_errors());
        assert_eq!(d.message(), Some("ok"));
    }

    #[test]
    fn test_results_has_errors() {
        let d = ToolProgressData::TestResults {
            passed: 10,
            failed: 2,
            skipped: Some(1),
            message: None,
        };
        assert!(d.has_errors());
    }

    #[test]
    fn lsp_diagnostic_has_errors() {
        let d = ToolProgressData::LSPDiagnostic {
            errors: 0,
            warnings: 5,
            infos: Some(3),
            file_path: Some("main.rs".into()),
        };
        assert!(!d.has_errors());
    }

    #[test]
    fn mcp_progress_fraction() {
        let d = ToolProgressData::MCPProgress {
            progress: Some(0.75),
            message: Some("working".into()),
            server_name: Some("test-server".into()),
        };
        assert!((d.progress_fraction().expect("frac") - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn custom_kind_and_message() {
        let d = ToolProgressData::Custom {
            label: "deploying".into(),
            detail: Some("step 3".into()),
        };
        assert_eq!(d.kind_name(), "custom");
        assert_eq!(d.message(), Some("step 3"));
    }

    // -- Display impl -------------------------------------------------------

    #[test]
    fn display_spinner() {
        let d = ToolProgressData::Spinner {
            message: Some("thinking".into()),
        };
        assert_eq!(format!("{d}"), "⠋ thinking");
    }

    #[test]
    fn display_progress_bar() {
        let d = ToolProgressData::ProgressBar {
            progress: 0.5,
            message: Some("halfway".into()),
        };
        assert_eq!(format!("{d}"), "[50%] halfway");
    }

    #[test]
    fn display_token_count() {
        let d = ToolProgressData::TokenCount {
            input_tokens: 100,
            output_tokens: 50,
            total_estimate: Some(200),
        };
        let s = format!("{d}");
        assert!(s.contains("100in/50out"));
        assert!(s.contains("~200 total"));
    }

    // -- ProgressStream -----------------------------------------------------

    #[test]
    fn progress_stream_emit_and_drain() {
        let stream = ProgressStream::new();
        stream.emit(
            "tc-1",
            ToolProgressData::Spinner {
                message: Some("a".into()),
            },
        );
        stream.emit(
            "tc-2",
            ToolProgressData::ProgressBar {
                progress: 0.5,
                message: None,
            },
        );
        assert_eq!(stream.len(), 2);
        let events = stream.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(stream.len(), 0);
    }

    #[test]
    fn progress_stream_close_ignores_emit() {
        let stream = ProgressStream::new();
        stream.close();
        stream.emit("tc-1", ToolProgressData::Spinner { message: None });
        assert!(stream.is_empty());
        assert!(stream.is_closed());
    }

    #[test]
    fn progress_stream_callback_invoked() {
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_clone = Arc::clone(&collected);
        let stream = ProgressStream::with_callback(Box::new(move |_id, data| {
            collected_clone.lock().push(data.kind_name().to_owned());
        }));
        stream.emit("tc-1", ToolProgressData::Spinner { message: None });
        stream.emit(
            "tc-2",
            ToolProgressData::ProgressBar {
                progress: 1.0,
                message: None,
            },
        );
        let kinds = collected.lock();
        assert_eq!(&*kinds, &["spinner", "progress_bar"]);
    }

    #[test]
    fn progress_stream_default() {
        let stream = ProgressStream::default();
        assert!(stream.is_empty());
        assert!(!stream.is_closed());
    }
}
