//! General-purpose utility modules for remote-code-rust.
//!
//! This crate provides reusable utility modules that are shared across
//! the workspace, including Git filesystem operations, memory types,
//! session teleport, secure storage, diff parsing, markdown rendering,
//! cron expressions, image processing, code indexing detection,
//! deep link support, computer use / screen control, platform installer
//! detection, Chrome extension integration, session restore, context
//! analysis, and conversation export.

pub mod chrome_extension;
pub mod code_indexing;
pub mod computer_use;
pub mod context_analysis;
pub mod cron;
pub mod deep_link;
pub mod diff;
pub mod git_fs;
pub mod image_resizer;
pub mod markdown;
pub mod memory_store;
pub mod memory_types;
pub mod output_export;
pub mod platform_installer;
pub mod secure_storage;
pub mod session_restore;
pub mod teleport;
