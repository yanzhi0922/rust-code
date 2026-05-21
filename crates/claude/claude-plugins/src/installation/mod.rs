//! Plugin installation system.
//!
//! Provides installation management, helpers, and install count tracking.

pub mod counts;
pub mod helpers;
pub mod manager;

pub use counts::PluginInstallCounts;
pub use helpers::{compute_install_path, download_plugin, extract_plugin, verify_plugin};
pub use manager::{InstallationProgress, PluginInstallationManager};
