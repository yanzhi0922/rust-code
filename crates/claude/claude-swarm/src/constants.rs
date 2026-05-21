//! Constants used across the swarm system.
//!
//! Defines team lead name, session name patterns, environment variable keys,
//! and default file-system paths for team data.

/// Default name for the team lead agent.
pub const TEAM_LEAD_NAME: &str = "lead";

/// Default session name prefix for swarm sessions.
pub const SESSION_NAME_PREFIX: &str = "rc-swarm";

/// File name for the team JSON data.
pub const TEAM_FILE_NAME: &str = "config.json";

/// Subdirectory name for permission sync files.
pub const PERMISSIONS_DIR_NAME: &str = "permissions";

/// Subdirectory name for mailbox data.
pub const MAILBOX_DIR_NAME: &str = "mailbox";

/// File extension for permission request files.
pub const PERMISSION_REQUEST_EXT: &str = ".req.json";

/// File extension for permission response files.
pub const PERMISSION_RESPONSE_EXT: &str = ".resp.json";

/// File extension for mailbox message files.
pub const MAILBOX_MESSAGE_EXT: &str = ".msg.json";

/// Environment variable: team name.
pub const ENV_TEAM_NAME: &str = "RC_SWARM_TEAM_NAME";

/// Environment variable: agent name within the team.
pub const ENV_AGENT_NAME: &str = "RC_SWARM_AGENT_NAME";

/// Environment variable: agent ID.
pub const ENV_AGENT_ID: &str = "RC_SWARM_AGENT_ID";

/// Environment variable: backend type.
pub const ENV_BACKEND_TYPE: &str = "RC_SWARM_BACKEND_TYPE";

/// Environment variable: lead agent ID.
pub const ENV_LEAD_AGENT_ID: &str = "RC_SWARM_LEAD_AGENT_ID";

/// Environment variable: team base directory.
pub const ENV_TEAM_DIR: &str = "RC_SWARM_TEAM_DIR";

/// Environment variable: permission mode.
pub const ENV_PERMISSION_MODE: &str = "RC_SWARM_PERMISSION_MODE";

/// Default base directory for team data (relative to home).
pub const DEFAULT_TEAMS_BASE_DIR: &str = ".claude/teams";

/// Maximum number of teammates allowed in a single team.
pub const MAX_TEAMMATES: usize = 10;

/// Timeout for permission requests (seconds).
pub const PERMISSION_REQUEST_TIMEOUT_SECS: u64 = 300;

/// Polling interval for permission responses (milliseconds).
pub const PERMISSION_POLL_INTERVAL_MS: u64 = 500;

/// Polling interval for mailbox messages (milliseconds).
pub const MAILBOX_POLL_INTERVAL_MS: u64 = 200;

/// Colors assigned to teammates for terminal display.
pub const TEAMMATE_COLORS: &[&str] = &[
    "cyan",
    "magenta",
    "yellow",
    "green",
    "blue",
    "red",
    "bright_cyan",
    "bright_magenta",
    "bright_yellow",
    "bright_green",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_non_empty() {
        assert!(!TEAM_LEAD_NAME.is_empty());
        assert!(!SESSION_NAME_PREFIX.is_empty());
        assert!(!TEAM_FILE_NAME.is_empty());
        assert!(!PERMISSIONS_DIR_NAME.is_empty());
        assert!(!MAILBOX_DIR_NAME.is_empty());
        assert!(!DEFAULT_TEAMS_BASE_DIR.is_empty());
    }

    #[test]
    fn env_vars_are_uppercase() {
        assert!(
            ENV_TEAM_NAME
                .chars()
                .filter(|c| c.is_ascii_lowercase())
                .count()
                == 0
        );
        assert!(
            ENV_AGENT_NAME
                .chars()
                .filter(|c| c.is_ascii_lowercase())
                .count()
                == 0
        );
        assert!(
            ENV_AGENT_ID
                .chars()
                .filter(|c| c.is_ascii_lowercase())
                .count()
                == 0
        );
    }

    #[test]
    fn teammate_colors_has_enough_entries() {
        assert!(TEAMMATE_COLORS.len() >= MAX_TEAMMATES);
    }

    #[test]
    fn file_extensions_include_dot() {
        assert!(PERMISSION_REQUEST_EXT.starts_with('.'));
        assert!(PERMISSION_RESPONSE_EXT.starts_with('.'));
        assert!(MAILBOX_MESSAGE_EXT.starts_with('.'));
    }

    #[test]
    fn timeout_values_are_reasonable() {
        const _: () = {
            assert!(PERMISSION_REQUEST_TIMEOUT_SECS > 0);
            assert!(PERMISSION_POLL_INTERVAL_MS > 0);
            assert!(MAILBOX_POLL_INTERVAL_MS > 0);
        };
    }
}
