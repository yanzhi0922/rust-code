//! # rc-swarm
//!
//! Swarm/Team multi-agent collaboration system.
//!
//! This crate implements a multi-agent collaboration system inspired by
//! Claude Code's Swarm architecture. It supports:
//!
//! - **Team management** — Create, read, update, and delete teams via JSON files
//! - **Permission synchronization** — File-system-based permission request/response
//! - **Mailbox** — File-system-based inter-agent message passing
//! - **Backend abstraction** — Pluggable terminal backends (InProcess, Tmux, iTerm2)
//! - **Teammate lifecycle** — Init, reconnect, layout, spawn, and stop
//!
//! # Example
//!
//! ```rust,ignore
//! use claude_swarm::team_helpers;
//! use claude_swarm::types::{TeamFile, BackendType};
//!
//! # async fn example() -> claude_swarm::error::SwarmResult<()> {
//! // Create a team
//! let team = TeamFile::new("my-team", "lead-agent-123");
//! team_helpers::create_team(&team).await?;
//!
//! // Read it back
//! let loaded = team_helpers::read_team("my-team").await?;
//! println!("Team: {} with {} members", loaded.name, loaded.members.len());
//! # Ok(())
//! # }
//! ```

pub mod backends;
pub mod constants;
pub mod detection;
pub mod error;
pub mod in_process_runner;
pub mod leader_bridge;
pub mod mailbox;
pub mod permission_sync;
pub mod reconnection;
pub mod spawn_utils;
pub mod team_helpers;
pub mod teammate_init;
pub mod teammate_layout;
pub mod teammate_model;
pub mod teammate_prompt;
pub mod types;

// Re-export key types.
pub use error::{SwarmError, SwarmResult};
pub use types::{
    BackendType, MailboxMessage, MailboxMessageType, PermissionDecision, SpawnConfig,
    SwarmPermissionRequest, TeamAllowedPath, TeamFile, TeamMember, TeammateIdentity, TeammateState,
};
