//! Agent Types parameter for API requests.
//!
//! Provides types and utilities for specifying which agent types are
//! allowed in a given API request, enabling the API to control agent
//! spawning behavior.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// AgentType enum
// ---------------------------------------------------------------------------

/// Types of agents that can be spawned by the API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// A sub-agent spawned to handle a subtask.
    SubAgent,
    /// A forked agent running in parallel.
    Fork,
    /// A coordinator agent managing multiple sub-agents.
    Coordinator,
    /// An exploration agent for codebase navigation.
    Explore,
    /// A planning agent for task decomposition.
    Plan,
    /// A verification agent for result checking.
    Verify,
}

impl AgentType {
    /// Return the wire representation for the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubAgent => "sub_agent",
            Self::Fork => "fork",
            Self::Coordinator => "coordinator",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Verify => "verify",
        }
    }

    /// All known agent type values.
    #[must_use]
    pub fn all_values() -> &'static [AgentType] {
        &[
            AgentType::SubAgent,
            AgentType::Fork,
            AgentType::Coordinator,
            AgentType::Explore,
            AgentType::Plan,
            AgentType::Verify,
        ]
    }

    /// Parse an agent type from its wire representation.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "sub_agent" => Some(Self::SubAgent),
            "fork" => Some(Self::Fork),
            "coordinator" => Some(Self::Coordinator),
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            "verify" => Some(Self::Verify),
            _ => None,
        }
    }
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// AgentTypeConfig
// ---------------------------------------------------------------------------

/// Configuration for allowed agent types in an API request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTypeConfig {
    /// The set of allowed agent types.
    pub allowed_types: Vec<AgentType>,
    /// Whether to allow spawning new agents at all.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AgentTypeConfig {
    fn default() -> Self {
        Self {
            allowed_types: AgentType::all_values().to_vec(),
            enabled: true,
        }
    }
}

impl AgentTypeConfig {
    /// Create a new config with only the specified agent types.
    #[must_use]
    pub fn new(allowed_types: Vec<AgentType>) -> Self {
        Self {
            allowed_types,
            enabled: true,
        }
    }

    /// Create a config that disables all agent spawning.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            allowed_types: vec![],
            enabled: false,
        }
    }

    /// Check if a specific agent type is allowed.
    #[must_use]
    pub fn is_allowed(&self, agent_type: AgentType) -> bool {
        self.enabled && self.allowed_types.contains(&agent_type)
    }
}

// ---------------------------------------------------------------------------
// API parameter generation
// ---------------------------------------------------------------------------

/// Generate the `allowedAgentTypes` API parameter.
///
/// # Arguments
///
/// * `config` — The agent type configuration.
///
/// # Returns
///
/// A JSON value suitable for inclusion in the API request body.
#[must_use]
pub fn allowed_agent_types(config: &AgentTypeConfig) -> Value {
    if !config.enabled {
        return json!({
            "allowedAgentTypes": [],
            "agentsEnabled": false,
        });
    }

    let types: Vec<&str> = config.allowed_types.iter().map(|t| t.as_str()).collect();
    json!({
        "allowedAgentTypes": types,
        "agentsEnabled": true,
    })
}

/// Merge agent type configuration into an API request body.
///
/// # Arguments
///
/// * `body` — The mutable API request body.
/// * `config` — The agent type configuration.
pub fn merge_agent_types_into_body(body: &mut Value, config: &AgentTypeConfig) {
    let params = allowed_agent_types(config);
    if let Value::Object(map) = params {
        for (key, value) in map {
            body[key] = value;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- AgentType ---

    #[test]
    fn agent_type_as_str() {
        assert_eq!(AgentType::SubAgent.as_str(), "sub_agent");
        assert_eq!(AgentType::Fork.as_str(), "fork");
        assert_eq!(AgentType::Coordinator.as_str(), "coordinator");
        assert_eq!(AgentType::Explore.as_str(), "explore");
        assert_eq!(AgentType::Plan.as_str(), "plan");
        assert_eq!(AgentType::Verify.as_str(), "verify");
    }

    #[test]
    fn agent_type_display() {
        assert_eq!(AgentType::SubAgent.to_string(), "sub_agent");
    }

    #[test]
    fn agent_type_all_values() {
        assert_eq!(AgentType::all_values().len(), 6);
    }

    #[test]
    fn agent_type_from_str_opt() {
        assert_eq!(
            AgentType::from_str_opt("sub_agent"),
            Some(AgentType::SubAgent)
        );
        assert_eq!(AgentType::from_str_opt("fork"), Some(AgentType::Fork));
        assert_eq!(AgentType::from_str_opt("unknown"), None);
    }

    #[test]
    fn agent_type_serialization_roundtrip() {
        for at in AgentType::all_values() {
            let json = serde_json::to_string(at).expect("serialize");
            let deserialized: AgentType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*at, deserialized);
        }
    }

    // --- AgentTypeConfig ---

    #[test]
    fn agent_type_config_default() {
        let config = AgentTypeConfig::default();
        assert!(config.enabled);
        assert_eq!(config.allowed_types.len(), 6);
    }

    #[test]
    fn agent_type_config_new() {
        let config = AgentTypeConfig::new(vec![AgentType::SubAgent, AgentType::Fork]);
        assert!(config.enabled);
        assert_eq!(config.allowed_types.len(), 2);
    }

    #[test]
    fn agent_type_config_disabled() {
        let config = AgentTypeConfig::disabled();
        assert!(!config.enabled);
        assert!(config.allowed_types.is_empty());
    }

    #[test]
    fn agent_type_config_is_allowed() {
        let config = AgentTypeConfig::new(vec![AgentType::SubAgent]);
        assert!(config.is_allowed(AgentType::SubAgent));
        assert!(!config.is_allowed(AgentType::Fork));
    }

    #[test]
    fn agent_type_config_is_allowed_disabled() {
        let config = AgentTypeConfig::disabled();
        assert!(!config.is_allowed(AgentType::SubAgent));
    }

    #[test]
    fn agent_type_config_serialization_roundtrip() {
        let config = AgentTypeConfig::new(vec![AgentType::Explore, AgentType::Plan]);
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: AgentTypeConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    // --- allowed_agent_types ---

    #[test]
    fn allowed_agent_types_enabled() {
        let config = AgentTypeConfig::new(vec![AgentType::SubAgent, AgentType::Fork]);
        let val = allowed_agent_types(&config);
        assert_eq!(val["agentsEnabled"], true);
        let types = val["allowedAgentTypes"].as_array().expect("array");
        assert_eq!(types.len(), 2);
        assert_eq!(types[0], "sub_agent");
        assert_eq!(types[1], "fork");
    }

    #[test]
    fn allowed_agent_types_disabled() {
        let config = AgentTypeConfig::disabled();
        let val = allowed_agent_types(&config);
        assert_eq!(val["agentsEnabled"], false);
        let types = val["allowedAgentTypes"].as_array().expect("array");
        assert!(types.is_empty());
    }

    #[test]
    fn allowed_agent_types_all() {
        let config = AgentTypeConfig::default();
        let val = allowed_agent_types(&config);
        let types = val["allowedAgentTypes"].as_array().expect("array");
        assert_eq!(types.len(), 6);
    }

    // --- merge_agent_types_into_body ---

    #[test]
    fn merge_into_body() {
        let mut body = json!({"model": "claude-3"});
        let config = AgentTypeConfig::new(vec![AgentType::SubAgent]);
        merge_agent_types_into_body(&mut body, &config);
        assert_eq!(body["model"], "claude-3");
        assert_eq!(body["agentsEnabled"], true);
        assert_eq!(body["allowedAgentTypes"][0], "sub_agent");
    }

    #[test]
    fn merge_into_body_disabled() {
        let mut body = json!({});
        let config = AgentTypeConfig::disabled();
        merge_agent_types_into_body(&mut body, &config);
        assert_eq!(body["agentsEnabled"], false);
    }
}
