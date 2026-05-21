//! Multi-server connection pool for MCP.
//!
//! Manages multiple MCP server connections with parallel connection
//! establishment, health checking, and batch disconnect/reconnect
//! operations. Works with [`crate::lifecycle::McpConnectionLifecycle`]
//! for state tracking.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::connection::McpServerConnection;
use crate::lifecycle::{DisconnectReason, McpConnectionLifecycle};
use crate::resources::ServerResource;
use crate::types::McpToolDescriptor;

// ── Pool connection entry ─────────────────────────────────────────────────────

/// A single server's entry in the connection pool.
#[derive(Debug, Clone)]
pub struct PoolEntry {
    /// Server name.
    pub name: String,
    /// Current connection state.
    pub connection: McpServerConnection,
    /// Discovered tools (if any).
    pub tools: Vec<McpToolDescriptor>,
    /// Discovered resources (if any).
    pub resources: Vec<ServerResource>,
    /// Last health check time.
    pub last_health_check: Option<Instant>,
    /// Whether the server is healthy (based on last health check).
    pub healthy: bool,
}

impl PoolEntry {
    /// Create a new pool entry.
    #[must_use]
    pub fn new(name: String, connection: McpServerConnection) -> Self {
        Self {
            name,
            connection,
            tools: Vec::new(),
            resources: Vec::new(),
            last_health_check: None,
            healthy: false,
        }
    }

    /// Returns `true` if the underlying connection is in the Connected state.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.is_connected()
    }

    /// Update the tools for this entry.
    pub fn set_tools(&mut self, tools: Vec<McpToolDescriptor>) {
        self.tools = tools;
    }

    /// Update the resources for this entry.
    pub fn set_resources(&mut self, resources: Vec<ServerResource>) {
        self.resources = resources;
    }

    /// Mark this entry as healthy with a timestamp.
    pub fn mark_healthy(&mut self) {
        self.healthy = true;
        self.last_health_check = Some(Instant::now());
    }

    /// Mark this entry as unhealthy.
    pub fn mark_unhealthy(&mut self) {
        self.healthy = false;
        self.last_health_check = Some(Instant::now());
    }
}

// ── Health check result ───────────────────────────────────────────────────────

/// Result of a health check on a single server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheckResult {
    /// Server is healthy and responsive.
    Healthy,
    /// Server is unhealthy or unresponsive.
    Unhealthy {
        /// Reason for the unhealthy status.
        reason: String,
    },
    /// Server is not connected (cannot health-check).
    NotConnected,
}

// ── Pool operation result ─────────────────────────────────────────────────────

/// Result of a batch pool operation.
#[derive(Debug, Clone)]
pub struct BatchOperationResult {
    /// Server names that succeeded.
    pub succeeded: Vec<String>,
    /// Server names that failed, with error messages.
    pub failed: Vec<(String, String)>,
}

impl BatchOperationResult {
    /// Create a new empty result.
    #[must_use]
    pub fn new() -> Self {
        Self {
            succeeded: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Returns `true` if all operations succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.failed.is_empty()
    }

    /// Returns the total number of operations.
    #[must_use]
    pub fn total(&self) -> usize {
        self.succeeded.len() + self.failed.len()
    }
}

impl Default for BatchOperationResult {
    fn default() -> Self {
        Self::new()
    }
}

// ── Connection pool configuration ─────────────────────────────────────────────

/// Configuration for the connection pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum number of parallel connection attempts.
    #[serde(default = "default_max_parallel")]
    pub max_parallel_connections: usize,
    /// Health check interval.
    #[serde(
        default = "default_health_check_interval_secs",
        with = "serde_duration_secs"
    )]
    pub health_check_interval: Duration,
    /// Connection timeout for each server.
    #[serde(default = "default_connect_timeout_secs", with = "serde_duration_secs")]
    pub connect_timeout: Duration,
    /// Whether to automatically reconnect failed servers.
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
}

fn default_max_parallel() -> usize {
    8
}
fn default_health_check_interval_secs() -> Duration {
    Duration::from_secs(30)
}
fn default_connect_timeout_secs() -> Duration {
    Duration::from_secs(30)
}
fn default_true() -> bool {
    true
}

mod serde_duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_parallel_connections: default_max_parallel(),
            health_check_interval: default_health_check_interval_secs(),
            connect_timeout: default_connect_timeout_secs(),
            auto_reconnect: default_true(),
        }
    }
}

// ── Connection pool ───────────────────────────────────────────────────────────

/// Multi-server MCP connection pool.
///
/// Manages multiple MCP server connections, providing:
/// - Parallel connection establishment
/// - Connection health checking
/// - Batch disconnect/reconnect operations
/// - Tool and resource aggregation across servers
#[derive(Debug)]
pub struct McpConnectionPool {
    entries: HashMap<String, PoolEntry>,
    lifecycle: McpConnectionLifecycle,
    config: PoolConfig,
}

impl McpConnectionPool {
    /// Create a new connection pool with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            lifecycle: McpConnectionLifecycle::new(),
            config: PoolConfig::default(),
        }
    }

    /// Create a new connection pool with custom configuration.
    #[must_use]
    pub fn with_config(config: PoolConfig) -> Self {
        let lifecycle = McpConnectionLifecycle::with_settings(
            5,
            Duration::from_secs(1),
            Duration::from_secs(30),
            config.connect_timeout,
        );
        Self {
            entries: HashMap::new(),
            lifecycle,
            config,
        }
    }

    /// Add a server to the pool.
    ///
    /// The server starts in a Pending connection state.
    pub fn add_server(&mut self, connection: McpServerConnection) {
        let name = connection.name().to_owned();
        self.lifecycle.register_server(&name);
        self.entries
            .insert(name.clone(), PoolEntry::new(name, connection));
    }

    /// Remove a server from the pool.
    pub fn remove_server(&mut self, name: &str) -> Option<PoolEntry> {
        self.lifecycle.unregister_server(name);
        self.entries.remove(name)
    }

    /// Get a pool entry by server name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PoolEntry> {
        self.entries.get(name)
    }

    /// Get a mutable pool entry by server name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut PoolEntry> {
        self.entries.get_mut(name)
    }

    /// Returns `true` if the pool contains a server with the given name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Return the number of servers in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return all server names in the pool.
    #[must_use]
    pub fn server_names(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    /// Return all connected server names.
    #[must_use]
    pub fn connected_servers(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, e)| e.is_connected())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Return all healthy server names.
    #[must_use]
    pub fn healthy_servers(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, e)| e.healthy && e.is_connected())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Return all unhealthy server names (connected but not healthy).
    #[must_use]
    pub fn unhealthy_servers(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, e)| e.is_connected() && !e.healthy)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Update a server's connection state in the pool.
    ///
    /// Returns the previous connection if the server existed.
    pub fn update_connection(
        &mut self,
        name: &str,
        connection: McpServerConnection,
    ) -> Option<McpServerConnection> {
        match self.entries.get_mut(name) {
            Some(entry) => {
                let old = std::mem::replace(&mut entry.connection, connection);
                // Update health based on connection state
                if entry.is_connected() {
                    entry.mark_healthy();
                } else {
                    entry.healthy = false;
                }
                Some(old)
            }
            None => None,
        }
    }

    /// Update tools for a server.
    pub fn update_tools(&mut self, name: &str, tools: Vec<McpToolDescriptor>) -> bool {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.set_tools(tools);
            true
        } else {
            false
        }
    }

    /// Update resources for a server.
    pub fn update_resources(&mut self, name: &str, resources: Vec<ServerResource>) -> bool {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.set_resources(resources);
            true
        } else {
            false
        }
    }

    /// Aggregate all tools from all connected servers.
    #[must_use]
    pub fn all_tools(&self) -> Vec<(&str, &McpToolDescriptor)> {
        let mut result = Vec::new();
        for (name, entry) in &self.entries {
            if entry.is_connected() {
                for tool in &entry.tools {
                    result.push((name.as_str(), tool));
                }
            }
        }
        result
    }

    /// Aggregate all resources from all connected servers.
    #[must_use]
    pub fn all_resources(&self) -> Vec<(&str, &ServerResource)> {
        let mut result = Vec::new();
        for (name, entry) in &self.entries {
            if entry.is_connected() {
                for resource in &entry.resources {
                    result.push((name.as_str(), resource));
                }
            }
        }
        result
    }

    /// Disconnect a specific server.
    pub fn disconnect_server(&mut self, name: &str, reason: DisconnectReason) -> bool {
        if let Some(entry) = self.entries.get_mut(name)
            && entry.is_connected()
        {
            entry.healthy = false;
            entry.last_health_check = None;
            let _ = self.lifecycle.disconnect(name, reason);
            return true;
        }
        false
    }

    /// Disconnect all servers.
    ///
    /// Returns a batch result indicating which servers were disconnected.
    pub fn disconnect_all(&mut self, reason: DisconnectReason) -> BatchOperationResult {
        let mut result = BatchOperationResult::new();
        let names: Vec<String> = self.entries.keys().cloned().collect();

        for name in names {
            if self.disconnect_server(&name, reason.clone()) {
                result.succeeded.push(name);
            }
            // Not connected is not a failure
        }
        result
    }

    /// Perform a health check on all connected servers.
    ///
    /// The `check_fn` closure is called for each connected server and
    /// returns whether the server is healthy.
    pub fn health_check_all<F>(&mut self, mut check_fn: F)
    where
        F: FnMut(&str, &PoolEntry) -> HealthCheckResult,
    {
        let names: Vec<String> = self.entries.keys().cloned().collect();
        for name in names {
            if let Some(entry) = self.entries.get_mut(&name) {
                if !entry.is_connected() {
                    continue;
                }
                // We need to get the result first, then update
                let result = {
                    let entry_ref = self.entries.get(&name).expect("entry exists");
                    check_fn(&name, entry_ref)
                };
                match result {
                    HealthCheckResult::Healthy => {
                        if let Some(entry) = self.entries.get_mut(&name) {
                            entry.mark_healthy();
                        }
                    }
                    HealthCheckResult::Unhealthy { .. } => {
                        if let Some(entry) = self.entries.get_mut(&name) {
                            entry.mark_unhealthy();
                        }
                    }
                    HealthCheckResult::NotConnected => {}
                }
            }
        }
    }

    /// Get the pool configuration.
    #[must_use]
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Get the underlying lifecycle manager.
    #[must_use]
    pub fn lifecycle(&self) -> &McpConnectionLifecycle {
        &self.lifecycle
    }

    /// Get a mutable reference to the underlying lifecycle manager.
    pub fn lifecycle_mut(&mut self) -> &mut McpConnectionLifecycle {
        &mut self.lifecycle
    }

    /// Return the number of servers in each connection state.
    #[must_use]
    pub fn connection_stats(&self) -> PoolConnectionStats {
        let mut stats = PoolConnectionStats::default();
        for entry in self.entries.values() {
            match &entry.connection {
                McpServerConnection::Connected(_) => stats.connected += 1,
                McpServerConnection::Failed(_) => stats.failed += 1,
                McpServerConnection::NeedsAuth(_) => stats.needs_auth += 1,
                McpServerConnection::Pending(_) => stats.pending += 1,
                McpServerConnection::Disabled(_) => stats.disabled += 1,
            }
        }
        stats
    }
}

impl Default for McpConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}

// ── Connection statistics ─────────────────────────────────────────────────────

/// Statistics about the pool's connection states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolConnectionStats {
    /// Number of connected servers.
    pub connected: usize,
    /// Number of failed servers.
    pub failed: usize,
    /// Number of servers needing authentication.
    pub needs_auth: usize,
    /// Number of pending servers.
    pub pending: usize,
    /// Number of disabled servers.
    pub disabled: usize,
}

impl PoolConnectionStats {
    /// Total number of servers.
    #[must_use]
    pub fn total(&self) -> usize {
        self.connected + self.failed + self.needs_auth + self.pending + self.disabled
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpCapabilityMatrix, McpServerConfig};
    use crate::connection::{DisabledServer, PendingServer};
    use crate::scope::{ConfigScope, ScopedMcpServerConfig};
    use crate::transport::McpTransportConfig;
    use std::collections::BTreeMap;

    fn test_scoped_config(name: &str) -> ScopedMcpServerConfig {
        ScopedMcpServerConfig::new(
            McpServerConfig {
                name: name.to_owned(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "echo".to_owned(),
                    args: vec![],
                    cwd: None,
                    env: BTreeMap::new(),
                },
                capabilities: McpCapabilityMatrix::default(),
                startup_timeout_secs: None,
                request_timeout_secs: None,
                metadata: BTreeMap::new(),
                oauth: None,
                tool_policy: crate::tool_policy::McpToolPolicy::default(),
            },
            ConfigScope::Local,
        )
    }

    fn pending_connection(name: &str) -> McpServerConnection {
        McpServerConnection::Pending(PendingServer {
            name: name.to_owned(),
            config: test_scoped_config(name),
            reconnect_attempt: None,
            max_reconnect_attempts: None,
        })
    }

    fn disabled_connection(name: &str) -> McpServerConnection {
        McpServerConnection::Disabled(DisabledServer {
            name: name.to_owned(),
            config: test_scoped_config(name),
        })
    }

    fn connected_connection(name: &str) -> McpServerConnection {
        McpServerConnection::Connected(crate::connection::ConnectedServer {
            name: name.to_owned(),
            capabilities: McpCapabilityMatrix::default(),
            server_info: None,
            instructions: None,
            config: test_scoped_config(name),
        })
    }

    fn failed_connection(name: &str, error: &str) -> McpServerConnection {
        McpServerConnection::Failed(crate::connection::FailedServer {
            name: name.to_owned(),
            config: test_scoped_config(name),
            error: Some(error.to_owned()),
        })
    }

    // ── PoolEntry tests ───────────────────────────────────────────────────

    #[test]
    fn pool_entry_new() {
        let entry = PoolEntry::new("test".to_owned(), pending_connection("test"));
        assert_eq!(entry.name, "test");
        assert!(!entry.is_connected());
        assert!(entry.tools.is_empty());
        assert!(entry.resources.is_empty());
        assert!(!entry.healthy);
    }

    #[test]
    fn pool_entry_connected() {
        let entry = PoolEntry::new("test".to_owned(), connected_connection("test"));
        assert!(entry.is_connected());
    }

    #[test]
    fn pool_entry_set_tools() {
        let mut entry = PoolEntry::new("test".to_owned(), connected_connection("test"));
        let tools = vec![McpToolDescriptor {
            name: "tool1".to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        entry.set_tools(tools);
        assert_eq!(entry.tools.len(), 1);
        assert_eq!(entry.tools[0].name, "tool1");
    }

    #[test]
    fn pool_entry_mark_healthy() {
        let mut entry = PoolEntry::new("test".to_owned(), connected_connection("test"));
        assert!(!entry.healthy);
        entry.mark_healthy();
        assert!(entry.healthy);
        assert!(entry.last_health_check.is_some());
    }

    #[test]
    fn pool_entry_mark_unhealthy() {
        let mut entry = PoolEntry::new("test".to_owned(), connected_connection("test"));
        entry.mark_healthy();
        entry.mark_unhealthy();
        assert!(!entry.healthy);
    }

    // ── McpConnectionPool tests ───────────────────────────────────────────

    #[test]
    fn new_pool_is_empty() {
        let pool = McpConnectionPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn add_server() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("srv1"));
        assert_eq!(pool.len(), 1);
        assert!(pool.contains("srv1"));
    }

    #[test]
    fn remove_server() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("srv1"));
        let removed = pool.remove_server("srv1");
        assert!(removed.is_some());
        assert!(pool.is_empty());
        assert!(!pool.contains("srv1"));
    }

    #[test]
    fn get_server() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("srv1"));
        let entry = pool.get("srv1");
        assert!(entry.is_some());
        assert_eq!(entry.expect("entry").name, "srv1");
    }

    #[test]
    fn server_names() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("a"));
        pool.add_server(pending_connection("b"));
        pool.add_server(pending_connection("c"));
        let mut names = pool.server_names();
        names.sort();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn connected_servers_filters() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("connected"));
        pool.add_server(pending_connection("pending"));
        pool.add_server(failed_connection("failed", "err"));

        let connected = pool.connected_servers();
        assert_eq!(connected.len(), 1);
        assert!(connected.contains(&"connected"));
    }

    #[test]
    fn update_connection() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("srv"));
        let old = pool.update_connection("srv", connected_connection("srv"));
        assert!(old.is_some());
        let entry = pool.get("srv").expect("exists");
        assert!(entry.is_connected());
        assert!(entry.healthy); // connected → healthy
    }

    #[test]
    fn update_tools() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("srv"));
        let tools = vec![McpToolDescriptor {
            name: "tool1".to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        assert!(pool.update_tools("srv", tools));
        assert_eq!(pool.get("srv").expect("exists").tools.len(), 1);
    }

    #[test]
    fn update_tools_nonexistent() {
        let mut pool = McpConnectionPool::new();
        assert!(!pool.update_tools("nope", vec![]));
    }

    #[test]
    fn all_tools_aggregates() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("srv1"));
        pool.add_server(connected_connection("srv2"));
        pool.add_server(pending_connection("srv3"));

        pool.update_tools(
            "srv1",
            vec![McpToolDescriptor {
                name: "tool-a".to_owned(),
                title: None,
                description: None,
                input_schema: serde_json::json!({}),
                annotations: serde_json::json!({}),
            }],
        );
        pool.update_tools(
            "srv2",
            vec![
                McpToolDescriptor {
                    name: "tool-b".to_owned(),
                    title: None,
                    description: None,
                    input_schema: serde_json::json!({}),
                    annotations: serde_json::json!({}),
                },
                McpToolDescriptor {
                    name: "tool-c".to_owned(),
                    title: None,
                    description: None,
                    input_schema: serde_json::json!({}),
                    annotations: serde_json::json!({}),
                },
            ],
        );

        let all_tools = pool.all_tools();
        assert_eq!(all_tools.len(), 3); // 1 from srv1 + 2 from srv2
    }

    #[test]
    fn disconnect_server() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("srv"));
        assert!(pool.disconnect_server("srv", DisconnectReason::Manual));
        let entry = pool.get("srv").expect("exists");
        assert!(!entry.healthy);
    }

    #[test]
    fn disconnect_non_connected_is_noop() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(pending_connection("srv"));
        assert!(!pool.disconnect_server("srv", DisconnectReason::Manual));
    }

    #[test]
    fn disconnect_all() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("a"));
        pool.add_server(connected_connection("b"));
        pool.add_server(pending_connection("c"));

        let result = pool.disconnect_all(DisconnectReason::Manual);
        assert_eq!(result.succeeded.len(), 2);
        assert!(result.all_succeeded());
    }

    #[test]
    fn health_check_all() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("healthy-srv"));
        pool.add_server(connected_connection("sick-srv"));
        pool.add_server(pending_connection("pending-srv"));

        pool.health_check_all(|name, _entry| match name {
            "healthy-srv" => HealthCheckResult::Healthy,
            "sick-srv" => HealthCheckResult::Unhealthy {
                reason: "slow response".to_owned(),
            },
            _ => HealthCheckResult::NotConnected,
        });

        let healthy = pool.healthy_servers();
        assert!(healthy.contains(&"healthy-srv"));
        assert!(!healthy.contains(&"sick-srv"));

        let unhealthy = pool.unhealthy_servers();
        assert!(unhealthy.contains(&"sick-srv"));
    }

    #[test]
    fn connection_stats() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("a"));
        pool.add_server(connected_connection("b"));
        pool.add_server(failed_connection("c", "err"));
        pool.add_server(pending_connection("d"));
        pool.add_server(disabled_connection("e"));

        let stats = pool.connection_stats();
        assert_eq!(stats.connected, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.disabled, 1);
        assert_eq!(stats.total(), 5);
    }

    #[test]
    fn batch_operation_result() {
        let mut result = BatchOperationResult::new();
        assert!(result.all_succeeded());
        assert_eq!(result.total(), 0);

        result.succeeded.push("a".to_owned());
        result.succeeded.push("b".to_owned());
        result.failed.push(("c".to_owned(), "error".to_owned()));
        assert!(!result.all_succeeded());
        assert_eq!(result.total(), 3);
    }

    #[test]
    fn pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_parallel_connections, 8);
        assert_eq!(config.health_check_interval, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, Duration::from_secs(30));
        assert!(config.auto_reconnect);
    }

    #[test]
    fn pool_config_serde_roundtrip() {
        let config = PoolConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let back: PoolConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.max_parallel_connections,
            config.max_parallel_connections
        );
        assert_eq!(back.health_check_interval, config.health_check_interval);
        assert_eq!(back.auto_reconnect, config.auto_reconnect);
    }

    #[test]
    fn pool_with_custom_config() {
        let config = PoolConfig {
            max_parallel_connections: 4,
            health_check_interval: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
            auto_reconnect: false,
        };
        let pool = McpConnectionPool::with_config(config);
        assert_eq!(pool.config().max_parallel_connections, 4);
        assert!(!pool.config().auto_reconnect);
    }

    #[test]
    fn all_resources_aggregates() {
        let mut pool = McpConnectionPool::new();
        pool.add_server(connected_connection("srv1"));
        pool.update_resources(
            "srv1",
            vec![
                ServerResource::new("file:///a", "srv1"),
                ServerResource::new("file:///b", "srv1"),
            ],
        );

        pool.add_server(pending_connection("srv2"));
        pool.update_resources("srv2", vec![ServerResource::new("file:///c", "srv2")]);

        let all_resources = pool.all_resources();
        assert_eq!(all_resources.len(), 2); // Only srv1 is connected
    }
}
