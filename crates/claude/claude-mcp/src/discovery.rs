//! MCP tool/resource discovery.
//!
//! Handles discovering tools, resources, and server instructions from MCP
//! servers after a successful connection. Results are cached by server name
//! and can be queried later.

use std::collections::HashMap;

use crate::config::McpServerConfig;
use crate::error::McpRuntimeError;
use crate::lifecycle::McpListChangedSurface;
use crate::resources::ServerResource;
use crate::types::{McpClientInfo, McpPromptDescriptor, McpToolDescriptor};

/// Result of discovering tools and resources from a server.
#[derive(Debug, Clone)]
pub struct McpDiscoveryResult {
    /// Discovered tools.
    pub tools: Vec<McpToolDescriptor>,
    /// Discovered resources.
    pub resources: Vec<ServerResource>,
    /// Discovered prompts.
    pub prompts: Vec<McpPromptDescriptor>,
    /// Server instructions (if any).
    pub instructions: Option<String>,
}

/// MCP tool/resource discovery cache.
///
/// Caches the results of `tools/list` and `resources/list` calls per server,
/// so they can be queried without re-inspecting the server.
#[derive(Debug, Default)]
pub struct McpDiscovery {
    /// Server name → discovered tools.
    tools: HashMap<String, Vec<McpToolDescriptor>>,
    /// Server name → discovered resources.
    resources: HashMap<String, Vec<ServerResource>>,
    /// Server name → discovered prompts.
    prompts: HashMap<String, Vec<McpPromptDescriptor>>,
    /// Server name → server instructions.
    instructions: HashMap<String, Option<String>>,
}

impl McpDiscovery {
    /// Create a new empty discovery cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Discover tools and resources for a server by calling [`inspect_server`].
    ///
    /// This spawns a new connection, performs the MCP handshake, lists tools
    /// and resources, then disconnects. The results are cached.
    ///
    /// Tools are filtered through the server's [`McpToolPolicy`] before
    /// being cached, so the model never sees denied tools.
    pub async fn discover_for_server(
        &mut self,
        name: &str,
        config: &McpServerConfig,
        client_info: &McpClientInfo,
    ) -> Result<McpDiscoveryResult, McpRuntimeError> {
        let inspection = crate::session::inspect_server(config, client_info).await?;

        // Apply tool policy filtering before caching.
        let tools = config.tool_policy.filter_tools(&inspection.tools);
        let resources = inspection.resources.clone();
        let prompts = inspection.prompts.clone();
        let instructions = inspection.instructions.clone();

        self.tools.insert(name.to_owned(), tools.clone());
        self.resources.insert(name.to_owned(), resources.clone());
        self.prompts.insert(name.to_owned(), prompts.clone());
        self.instructions
            .insert(name.to_owned(), instructions.clone());

        Ok(McpDiscoveryResult {
            tools,
            resources,
            prompts,
            instructions,
        })
    }

    /// Store discovery results directly (e.g. from an already-connected session).
    pub fn store(
        &mut self,
        name: &str,
        tools: Vec<McpToolDescriptor>,
        resources: Vec<ServerResource>,
        prompts: Vec<McpPromptDescriptor>,
        instructions: Option<String>,
    ) {
        self.tools.insert(name.to_owned(), tools);
        self.resources.insert(name.to_owned(), resources);
        self.prompts.insert(name.to_owned(), prompts);
        self.instructions.insert(name.to_owned(), instructions);
    }

    /// Standalone discovery function that can be called from a spawned task.
    ///
    /// This is a free-function version of [`Self::discover_for_server`] that
    /// does not require `&mut self`, making it suitable for concurrent use
    /// via `tokio::task::JoinSet`. The caller is responsible for caching the
    /// results via [`Self::store`].
    pub async fn discover_for_server_standalone(
        _name: &str,
        config: &McpServerConfig,
        client_info: &McpClientInfo,
    ) -> Result<McpDiscoveryResult, McpRuntimeError> {
        let inspection = crate::session::inspect_server(config, client_info).await?;

        // Apply tool policy filtering.
        let tools = config.tool_policy.filter_tools(&inspection.tools);
        let resources = inspection.resources;
        let prompts = inspection.prompts;
        let instructions = inspection.instructions;

        Ok(McpDiscoveryResult {
            tools,
            resources,
            prompts,
            instructions,
        })
    }

    /// Get the tools for a specific server.
    #[must_use]
    pub fn tools(&self, server_name: &str) -> Option<&[McpToolDescriptor]> {
        self.tools.get(server_name).map(Vec::as_slice)
    }

    /// Get all tools across all servers.
    ///
    /// Returns a list of `(server_name, tools_slice)` tuples.
    pub fn all_tools(&self) -> Vec<(&str, &[McpToolDescriptor])> {
        self.tools
            .iter()
            .map(|(name, tools)| (name.as_str(), tools.as_slice()))
            .collect()
    }

    /// Get the resources for a specific server.
    #[must_use]
    pub fn resources(&self, server_name: &str) -> Option<&[ServerResource]> {
        self.resources.get(server_name).map(Vec::as_slice)
    }

    /// Get the prompts for a specific server.
    #[must_use]
    pub fn prompts(&self, server_name: &str) -> Option<&[McpPromptDescriptor]> {
        self.prompts.get(server_name).map(Vec::as_slice)
    }

    /// Get the instructions for a specific server.
    #[must_use]
    pub fn instructions(&self, server_name: &str) -> Option<&Option<String>> {
        self.instructions.get(server_name)
    }

    /// Clear cached discovery data for a specific server.
    pub fn clear_server(&mut self, server_name: &str) {
        self.tools.remove(server_name);
        self.resources.remove(server_name);
        self.prompts.remove(server_name);
        self.instructions.remove(server_name);
    }

    /// Clear cached discovery data for one surface of a specific server.
    pub fn clear_server_surface(&mut self, server_name: &str, surface: McpListChangedSurface) {
        match surface {
            McpListChangedSurface::Tools => {
                self.tools.remove(server_name);
                self.instructions.remove(server_name);
            }
            McpListChangedSurface::Prompts => {
                self.prompts.remove(server_name);
                self.instructions.remove(server_name);
            }
            McpListChangedSurface::Resources => {
                self.resources.remove(server_name);
            }
        }
    }

    /// Clear all cached discovery data.
    pub fn clear_all(&mut self) {
        self.tools.clear();
        self.resources.clear();
        self.prompts.clear();
        self.instructions.clear();
    }

    /// Return the total number of tools across all servers.
    #[must_use]
    pub fn total_tool_count(&self) -> usize {
        self.tools.values().map(Vec::len).sum()
    }

    /// Return the total number of resources across all servers.
    #[must_use]
    pub fn total_resource_count(&self) -> usize {
        self.resources.values().map(Vec::len).sum()
    }

    /// Return the total number of prompts across all servers.
    #[must_use]
    pub fn total_prompt_count(&self) -> usize {
        self.prompts.values().map(Vec::len).sum()
    }

    /// Return the number of servers with cached discovery data.
    #[must_use]
    pub fn server_count(&self) -> usize {
        self.tools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_discovery_is_empty() {
        let discovery = McpDiscovery::new();
        assert_eq!(discovery.server_count(), 0);
        assert_eq!(discovery.total_tool_count(), 0);
        assert_eq!(discovery.total_resource_count(), 0);
        assert_eq!(discovery.total_prompt_count(), 0);
    }

    #[test]
    fn store_and_retrieve_tools() {
        let mut discovery = McpDiscovery::new();
        let tools = vec![McpToolDescriptor {
            name: "search".to_owned(),
            title: None,
            description: Some("Search things".to_owned()),
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        discovery.store("srv", tools.clone(), vec![], vec![], None);
        assert_eq!(discovery.tools("srv"), Some(tools.as_slice()));
        assert_eq!(discovery.total_tool_count(), 1);
    }

    #[test]
    fn store_and_retrieve_resources() {
        let mut discovery = McpDiscovery::new();
        let resources = vec![
            ServerResource::new("file:///data", "srv")
                .with_name("Data")
                .with_mime_type("text/csv"),
        ];
        discovery.store("srv", vec![], resources.clone(), vec![], None);
        assert_eq!(discovery.resources("srv"), Some(resources.as_slice()));
        assert_eq!(discovery.total_resource_count(), 1);
    }

    #[test]
    fn store_and_retrieve_prompts() {
        let mut discovery = McpDiscovery::new();
        let prompts = vec![McpPromptDescriptor {
            name: "plan".to_owned(),
            title: None,
            description: Some("Plan work".to_owned()),
            arguments: vec![],
        }];
        discovery.store("srv", vec![], vec![], prompts.clone(), None);
        assert_eq!(discovery.prompts("srv"), Some(prompts.as_slice()));
        assert_eq!(discovery.total_prompt_count(), 1);
    }

    #[test]
    fn store_and_retrieve_instructions() {
        let mut discovery = McpDiscovery::new();
        discovery.store("srv", vec![], vec![], vec![], Some("Be careful".to_owned()));
        assert_eq!(
            discovery.instructions("srv"),
            Some(&Some("Be careful".to_owned()))
        );
    }

    #[test]
    fn all_tools_across_servers() {
        let mut discovery = McpDiscovery::new();
        let tools_a = vec![McpToolDescriptor {
            name: "a1".to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        let tools_b = vec![McpToolDescriptor {
            name: "b1".to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        discovery.store("a", tools_a, vec![], vec![], None);
        discovery.store("b", tools_b, vec![], vec![], None);
        let all = discovery.all_tools();
        assert_eq!(all.len(), 2);
        assert_eq!(discovery.total_tool_count(), 2);
    }

    #[test]
    fn clear_server_removes_data() {
        let mut discovery = McpDiscovery::new();
        discovery.store("srv", vec![], vec![], vec![], None);
        assert_eq!(discovery.server_count(), 1);
        discovery.clear_server("srv");
        assert_eq!(discovery.server_count(), 0);
        assert!(discovery.tools("srv").is_none());
    }

    #[test]
    fn clear_server_surface_invalidates_only_requested_surface() {
        let mut discovery = McpDiscovery::new();
        let tools = vec![McpToolDescriptor {
            name: "search".to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }];
        let resources = vec![ServerResource::new("file:///data", "srv")];
        let prompts = vec![McpPromptDescriptor {
            name: "plan".to_owned(),
            title: None,
            description: None,
            arguments: vec![],
        }];
        discovery.store(
            "srv",
            tools.clone(),
            resources.clone(),
            prompts.clone(),
            Some("instructions".to_owned()),
        );

        discovery.clear_server_surface("srv", McpListChangedSurface::Resources);
        assert_eq!(discovery.tools("srv"), Some(tools.as_slice()));
        assert!(discovery.resources("srv").is_none());
        assert_eq!(discovery.prompts("srv"), Some(prompts.as_slice()));
        assert_eq!(
            discovery.instructions("srv"),
            Some(&Some("instructions".to_owned()))
        );

        discovery.clear_server_surface("srv", McpListChangedSurface::Prompts);
        assert!(discovery.prompts("srv").is_none());
        assert!(discovery.instructions("srv").is_none());

        discovery.store(
            "srv",
            tools.clone(),
            Vec::new(),
            prompts,
            Some("instructions".to_owned()),
        );
        discovery.clear_server_surface("srv", McpListChangedSurface::Tools);
        assert!(discovery.tools("srv").is_none());
        assert!(discovery.instructions("srv").is_none());
    }

    #[test]
    fn clear_all_removes_everything() {
        let mut discovery = McpDiscovery::new();
        discovery.store("a", vec![], vec![], vec![], None);
        discovery.store("b", vec![], vec![], vec![], None);
        discovery.clear_all();
        assert_eq!(discovery.server_count(), 0);
    }

    #[test]
    fn missing_server_returns_none() {
        let discovery = McpDiscovery::new();
        assert!(discovery.tools("nonexistent").is_none());
        assert!(discovery.resources("nonexistent").is_none());
        assert!(discovery.prompts("nonexistent").is_none());
        assert!(discovery.instructions("nonexistent").is_none());
    }
}
