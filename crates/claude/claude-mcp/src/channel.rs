//! Channel permissions (allowlist, notifications, permission management).
//!
//! Manages which MCP servers are allowed to communicate through channels,
//! provides notification types for channel messages, and supports custom
//! permission decisions via callbacks.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

// ── Channel allowlist ───────────────────────────────────────────────────────

/// A set of MCP server names that are allowed to use channels.
///
/// Servers not in the allowlist will be denied channel access unless
/// overridden by a permission callback.
#[derive(Debug, Clone, Default)]
pub struct ChannelAllowlist {
    allowed_servers: HashSet<String>,
}

impl ChannelAllowlist {
    /// Create a new empty allowlist.
    #[must_use]
    pub fn new() -> Self {
        Self {
            allowed_servers: HashSet::new(),
        }
    }

    /// Check if a server is in the allowlist.
    #[must_use]
    pub fn is_allowed(&self, server_name: &str) -> bool {
        self.allowed_servers.contains(server_name)
    }

    /// Add a server to the allowlist.
    pub fn add(&mut self, server_name: String) {
        self.allowed_servers.insert(server_name);
    }

    /// Remove a server from the allowlist.
    pub fn remove(&mut self, server_name: &str) {
        self.allowed_servers.remove(server_name);
    }

    /// Get the number of allowed servers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.allowed_servers.len()
    }

    /// Check if the allowlist is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allowed_servers.is_empty()
    }

    /// Get an iterator over the allowed server names.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.allowed_servers.iter().map(|s| s.as_str())
    }
}

// ── Channel message ─────────────────────────────────────────────────────────

/// A message sent through a channel by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMessage {
    /// Name of the MCP server sending the message.
    pub server_name: String,
    /// Channel identifier.
    pub channel: String,
    /// Message content.
    pub content: String,
    /// Timestamp as Unix epoch seconds.
    pub timestamp: i64,
}

impl ChannelMessage {
    /// Create a new channel message with the current timestamp.
    #[must_use]
    pub fn new(server_name: String, channel: String, content: String) -> Self {
        Self {
            server_name,
            channel,
            content,
            timestamp: epoch_seconds(),
        }
    }

    /// Create a channel message with a specific timestamp (for testing).
    #[must_use]
    pub fn with_timestamp(
        server_name: String,
        channel: String,
        content: String,
        timestamp: i64,
    ) -> Self {
        Self {
            server_name,
            channel,
            content,
            timestamp,
        }
    }
}

// ── Channel permission decision ─────────────────────────────────────────────

/// Decision made by the permission manager for a channel message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelPermissionDecision {
    /// Allow the message through.
    Allow,
    /// Deny the message with a reason.
    Deny {
        /// Reason for denial.
        reason: String,
    },
    /// Defer the decision to the user.
    AskUser,
}

// ── Channel permission manager ──────────────────────────────────────────────

/// Manages channel permissions by combining an allowlist with an optional
/// callback for custom permission logic.
///
/// The decision flow is:
/// 1. If the server is in the allowlist → `Allow`
/// 2. If a callback is set → delegate to the callback
/// 3. Otherwise → `Deny`
type PermissionCallback = Box<dyn Fn(&ChannelMessage) -> ChannelPermissionDecision + Send + Sync>;

pub struct ChannelPermissionManager {
    allowlist: ChannelAllowlist,
    decision_callback: Option<PermissionCallback>,
}

impl std::fmt::Debug for ChannelPermissionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelPermissionManager")
            .field("allowlist", &self.allowlist)
            .field("has_callback", &self.decision_callback.is_some())
            .finish()
    }
}

impl ChannelPermissionManager {
    /// Create a new permission manager with an empty allowlist and no callback.
    #[must_use]
    pub fn new() -> Self {
        Self {
            allowlist: ChannelAllowlist::new(),
            decision_callback: None,
        }
    }

    /// Create a permission manager with a custom decision callback.
    #[must_use]
    pub fn with_callback(callback: PermissionCallback) -> Self {
        Self {
            allowlist: ChannelAllowlist::new(),
            decision_callback: Some(callback),
        }
    }

    /// Check the permission for a channel message.
    pub fn check_permission(&self, message: &ChannelMessage) -> ChannelPermissionDecision {
        // Step 1: Check allowlist
        if self.allowlist.is_allowed(&message.server_name) {
            return ChannelPermissionDecision::Allow;
        }

        // Step 2: Delegate to callback if present
        if let Some(ref callback) = self.decision_callback {
            return callback(message);
        }

        // Step 3: Default deny
        ChannelPermissionDecision::Deny {
            reason: format!("server '{}' not in allowlist", message.server_name),
        }
    }

    /// Add a server to the allowlist.
    pub fn add_to_allowlist(&mut self, server_name: String) {
        self.allowlist.add(server_name);
    }

    /// Remove a server from the allowlist.
    pub fn remove_from_allowlist(&mut self, server_name: &str) {
        self.allowlist.remove(server_name);
    }

    /// Check if a server is in the allowlist.
    #[must_use]
    pub fn is_allowed(&self, server_name: &str) -> bool {
        self.allowlist.is_allowed(server_name)
    }

    /// Get the number of servers in the allowlist.
    #[must_use]
    pub fn allowlist_len(&self) -> usize {
        self.allowlist.len()
    }
}

impl Default for ChannelPermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Get current epoch seconds.
fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_new_is_empty() {
        let al = ChannelAllowlist::new();
        assert!(al.is_empty());
        assert_eq!(al.len(), 0);
    }

    #[test]
    fn allowlist_add_and_check() {
        let mut al = ChannelAllowlist::new();
        al.add("server-a".to_owned());
        al.add("server-b".to_owned());
        assert!(al.is_allowed("server-a"));
        assert!(al.is_allowed("server-b"));
        assert!(!al.is_allowed("server-c"));
        assert_eq!(al.len(), 2);
    }

    #[test]
    fn allowlist_remove() {
        let mut al = ChannelAllowlist::new();
        al.add("server-a".to_owned());
        assert!(al.is_allowed("server-a"));
        al.remove("server-a");
        assert!(!al.is_allowed("server-a"));
        assert!(al.is_empty());
    }

    #[test]
    fn channel_message_new_has_timestamp() {
        let msg = ChannelMessage::new(
            "test-server".to_owned(),
            "ch-1".to_owned(),
            "hello".to_owned(),
        );
        assert_eq!(msg.server_name, "test-server");
        assert_eq!(msg.channel, "ch-1");
        assert_eq!(msg.content, "hello");
        assert!(msg.timestamp > 0);
    }

    #[test]
    fn channel_message_serde_roundtrip() {
        let msg = ChannelMessage::with_timestamp(
            "srv".to_owned(),
            "ch".to_owned(),
            "content".to_owned(),
            1234567890,
        );
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: ChannelMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, msg);
    }

    #[test]
    fn permission_manager_allowlist_allows() {
        let mut mgr = ChannelPermissionManager::new();
        mgr.add_to_allowlist("trusted-server".to_owned());

        let msg = ChannelMessage::with_timestamp(
            "trusted-server".to_owned(),
            "ch".to_owned(),
            "data".to_owned(),
            100,
        );
        let decision = mgr.check_permission(&msg);
        assert_eq!(decision, ChannelPermissionDecision::Allow);
    }

    #[test]
    fn permission_manager_default_denies() {
        let mgr = ChannelPermissionManager::new();
        let msg = ChannelMessage::with_timestamp(
            "unknown-server".to_owned(),
            "ch".to_owned(),
            "data".to_owned(),
            100,
        );
        let decision = mgr.check_permission(&msg);
        assert!(matches!(decision, ChannelPermissionDecision::Deny { .. }));
    }

    #[test]
    fn permission_manager_callback_overrides() {
        let mgr = ChannelPermissionManager::with_callback(Box::new(|_msg| {
            ChannelPermissionDecision::AskUser
        }));

        let msg = ChannelMessage::with_timestamp(
            "any-server".to_owned(),
            "ch".to_owned(),
            "data".to_owned(),
            100,
        );
        let decision = mgr.check_permission(&msg);
        assert_eq!(decision, ChannelPermissionDecision::AskUser);
    }

    #[test]
    fn permission_manager_allowlist_takes_precedence_over_callback() {
        let mut mgr = ChannelPermissionManager::with_callback(Box::new(|_msg| {
            ChannelPermissionDecision::Deny {
                reason: "blocked by callback".to_owned(),
            }
        }));
        mgr.add_to_allowlist("special-server".to_owned());

        // Allowlisted server should be allowed even though callback denies
        let msg = ChannelMessage::with_timestamp(
            "special-server".to_owned(),
            "ch".to_owned(),
            "data".to_owned(),
            100,
        );
        let decision = mgr.check_permission(&msg);
        assert_eq!(decision, ChannelPermissionDecision::Allow);

        // Non-allowlisted server should be denied by callback
        let msg2 = ChannelMessage::with_timestamp(
            "other-server".to_owned(),
            "ch".to_owned(),
            "data".to_owned(),
            100,
        );
        let decision2 = mgr.check_permission(&msg2);
        assert!(matches!(decision2, ChannelPermissionDecision::Deny { .. }));
    }

    #[test]
    fn permission_manager_remove_from_allowlist() {
        let mut mgr = ChannelPermissionManager::new();
        mgr.add_to_allowlist("srv".to_owned());
        assert!(mgr.is_allowed("srv"));
        mgr.remove_from_allowlist("srv");
        assert!(!mgr.is_allowed("srv"));
    }

    #[test]
    fn allowlist_iter() {
        let mut al = ChannelAllowlist::new();
        al.add("a".to_owned());
        al.add("b".to_owned());
        let names: Vec<&str> = al.iter().collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }
}
