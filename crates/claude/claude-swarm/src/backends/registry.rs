//! Backend registry and detection.
//!
//! Provides [`BackendRegistry`] for managing available backends and
//! [`detect_backend`] for auto-detecting the best backend for the
//! current environment.

use std::sync::Arc;

use crate::backends::PaneBackend;
use crate::backends::in_process::InProcessBackend;
use crate::backends::iterm::ItermBackend;
use crate::backends::tmux::TmuxBackend;
use crate::error::{SwarmError, SwarmResult};
use crate::types::BackendType;

/// Registry of available backend implementations.
pub struct BackendRegistry {
    backends: Vec<Arc<dyn PaneBackend>>,
}

impl std::fmt::Debug for BackendRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendRegistry")
            .field("count", &self.backends.len())
            .field("names", &self.backend_names())
            .finish()
    }
}

impl BackendRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Create a registry pre-loaded with all built-in backends.
    #[must_use]
    pub fn with_defaults(team_name: &str) -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(InProcessBackend::new()));
        registry.register(Arc::new(TmuxBackend::new(team_name)));
        registry.register(Arc::new(ItermBackend::new()));
        registry
    }

    /// Register a backend.
    pub fn register(&mut self, backend: Arc<dyn PaneBackend>) {
        self.backends.push(backend);
    }

    /// Find a backend by type.
    pub fn find(&self, backend_type: BackendType) -> Option<Arc<dyn PaneBackend>> {
        let name = backend_type.as_str();
        self.backends
            .iter()
            .find(|b| b.backend_name() == name)
            .cloned()
    }

    /// Get the first available backend.
    pub async fn first_available(&self) -> Option<Arc<dyn PaneBackend>> {
        for backend in &self.backends {
            if backend.is_available().await {
                return Some(Arc::clone(backend));
            }
        }
        None
    }

    /// List all registered backend names.
    #[must_use]
    pub fn backend_names(&self) -> Vec<&str> {
        self.backends.iter().map(|b| b.backend_name()).collect()
    }

    /// Count registered backends.
    #[must_use]
    pub fn count(&self) -> usize {
        self.backends.len()
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Detect the best backend for the current environment.
///
/// Priority order:
/// 1. Tmux (if inside a tmux session)
/// 2. iTerm2 (if on macOS with iTerm2)
/// 3. InProcess (always available as fallback)
pub async fn detect_backend() -> BackendType {
    // Check if we're inside tmux.
    if is_inside_tmux() {
        return BackendType::Tmux;
    }

    // Check if we're on macOS with iTerm2.
    if is_iterm2_available().await {
        return BackendType::ITerm2;
    }

    // Fallback to in-process.
    BackendType::InProcess
}

/// Detect the best backend from a registry, falling back to the preferred type.
pub async fn detect_backend_with_registry(
    registry: &BackendRegistry,
    preferred: Option<BackendType>,
) -> SwarmResult<Arc<dyn PaneBackend>> {
    // If a preferred type is specified, try it first.
    if let Some(bt) = preferred {
        if let Some(backend) = registry.find(bt) {
            if backend.is_available().await {
                return Ok(backend);
            }
            return Err(SwarmError::BackendUnavailable(bt.to_string()));
        }
        return Err(SwarmError::BackendUnavailable(bt.to_string()));
    }

    // Auto-detect.
    let detected = detect_backend().await;
    registry
        .find(detected)
        .ok_or_else(|| SwarmError::BackendDetectionFailed {
            message: format!("detected backend {} but it is not registered", detected),
        })
}

/// Check if we're running inside a tmux session.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Check if iTerm2 is available (macOS with it2 command).
pub async fn is_iterm2_available() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    tokio::process::Command::new("it2")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .expect_err("should not be true on Windows")
        .to_string()
        .contains("it2")
}

/// Check if the `TERM_PROGRAM` environment variable indicates iTerm2.
pub fn is_iterm2_term() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app")
}

/// Get the terminal program name from the environment.
pub fn terminal_program() -> Option<String> {
    std::env::var("TERM_PROGRAM").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_new_is_empty() {
        let registry = BackendRegistry::new();
        assert_eq!(registry.count(), 0);
        assert!(registry.backend_names().is_empty());
    }

    #[test]
    fn registry_default_is_empty() {
        let registry = BackendRegistry::default();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn registry_with_defaults() {
        let registry = BackendRegistry::with_defaults("test-team");
        assert_eq!(registry.count(), 3);
        let names = registry.backend_names();
        assert!(names.contains(&"in_process"));
        assert!(names.contains(&"tmux"));
        assert!(names.contains(&"iterm2"));
    }

    #[test]
    fn registry_find_in_process() {
        let registry = BackendRegistry::with_defaults("test-team");
        let backend = registry.find(BackendType::InProcess);
        assert!(backend.is_some());
        assert_eq!(backend.expect("found").backend_name(), "in_process");
    }

    #[test]
    fn registry_find_tmux() {
        let registry = BackendRegistry::with_defaults("test-team");
        let backend = registry.find(BackendType::Tmux);
        assert!(backend.is_some());
        assert_eq!(backend.expect("found").backend_name(), "tmux");
    }

    #[test]
    fn registry_find_iterm2() {
        let registry = BackendRegistry::with_defaults("test-team");
        let backend = registry.find(BackendType::ITerm2);
        assert!(backend.is_some());
        assert_eq!(backend.expect("found").backend_name(), "iterm2");
    }

    #[tokio::test]
    async fn registry_first_available() {
        let registry = BackendRegistry::with_defaults("test-team");
        let backend = registry.first_available().await;
        assert!(backend.is_some());
    }

    #[test]
    fn is_inside_tmux_env_check() {
        // This test just verifies the function doesn't panic.
        let _ = is_inside_tmux();
    }

    #[test]
    fn is_iterm2_term_check() {
        // This test just verifies the function doesn't panic.
        let _ = is_iterm2_term();
    }

    #[test]
    fn terminal_program_check() {
        // This test just verifies the function doesn't panic.
        let _ = terminal_program();
    }

    #[tokio::test]
    async fn detect_backend_returns_valid_type() {
        let backend = detect_backend().await;
        // Should always return a valid backend type.
        assert!(matches!(
            backend,
            BackendType::InProcess | BackendType::Tmux | BackendType::ITerm2
        ));
    }

    #[tokio::test]
    async fn detect_backend_with_registry_preferred() {
        let registry = BackendRegistry::with_defaults("test-team");
        let result = detect_backend_with_registry(&registry, Some(BackendType::InProcess)).await;
        assert!(result.is_ok());
        assert_eq!(result.expect("ok").backend_name(), "in_process");
    }

    #[tokio::test]
    async fn detect_backend_with_registry_auto() {
        let registry = BackendRegistry::with_defaults("test-team");
        let result = detect_backend_with_registry(&registry, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn detect_backend_with_registry_unavailable_preferred() {
        let registry = BackendRegistry::new();
        let result = detect_backend_with_registry(&registry, Some(BackendType::Tmux)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn detect_backend_with_registry_empty_auto() {
        let registry = BackendRegistry::new();
        let result = detect_backend_with_registry(&registry, None).await;
        assert!(result.is_err());
    }
}
