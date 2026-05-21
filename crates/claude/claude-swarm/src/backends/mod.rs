//! Backend abstraction layer for the swarm system.
//!
//! Defines the [`PaneBackend`] trait and [`TeammateExecutor`] trait
//! that terminal backends must implement. Provides a factory function
//! for creating backends based on the detected environment.

pub mod in_process;
pub mod iterm;
pub mod pane_executor;
pub mod registry;
pub mod tmux;
pub mod types;

use crate::backends::types::{CreatePaneResult, PaneConfig, PaneId, PaneStatus};
use crate::error::SwarmResult;
use crate::types::SpawnConfig;
use async_trait::async_trait;

/// Trait for managing terminal panes.
///
/// Each backend (tmux, iTerm2, in-process) implements this trait
/// to provide pane creation, listing, and lifecycle management.
#[async_trait]
pub trait PaneBackend: Send + Sync {
    /// Get the name of this backend.
    fn backend_name(&self) -> &'static str;

    /// Create a new pane with the given configuration.
    async fn create_pane(&self, config: &PaneConfig) -> SwarmResult<CreatePaneResult>;

    /// Check the status of a pane.
    async fn pane_status(&self, pane_id: &PaneId) -> SwarmResult<PaneStatus>;

    /// List all pane IDs managed by this backend.
    async fn list_panes(&self) -> SwarmResult<Vec<PaneId>>;

    /// Destroy a pane.
    async fn destroy_pane(&self, pane_id: &PaneId) -> SwarmResult<()>;

    /// Send text input to a pane.
    async fn send_to_pane(&self, pane_id: &PaneId, text: &str) -> SwarmResult<()>;

    /// Check if this backend is available on the current system.
    async fn is_available(&self) -> bool;
}

/// Trait for executing a teammate agent.
///
/// This trait abstracts the execution of a teammate, whether
/// in-process or in a separate terminal pane.
#[async_trait]
pub trait TeammateExecutor: Send + Sync {
    /// Start a teammate with the given spawn configuration.
    async fn start_teammate(&self, config: &SpawnConfig) -> SwarmResult<CreatePaneResult>;

    /// Stop a teammate by pane ID.
    async fn stop_teammate(&self, pane_id: &PaneId) -> SwarmResult<()>;

    /// Check if a teammate is still running.
    async fn is_teammate_running(&self, pane_id: &PaneId) -> SwarmResult<bool>;
}

#[cfg(test)]
mod tests {
    // The trait tests are covered by the concrete backend implementations.
    // This module ensures the file compiles correctly.

    #[test]
    fn backend_module_compiles() {
        // If this compiles, the module structure is correct.
    }
}
