use crate::permissions::PermissionMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub features: FeaturesConfig,
    #[serde(default)]
    pub model: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            permissions: PermissionsConfig::default(),
            mcp_servers: HashMap::new(),
            plugins: PluginsConfig::default(),
            features: FeaturesConfig::default(),
            model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub openai_compat_api_key: Option<String>,
    #[serde(default)]
    pub openai_compat_base_url: Option<String>,
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub timeout_secs: u64,
}

fn default_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            anthropic_api_key: None,
            openai_compat_api_key: None,
            openai_compat_base_url: None,
            max_tokens: 8192,
            temperature: None,
            timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub mode: PermissionMode,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginsConfig {
    #[serde(default)]
    pub directories: Vec<PathBuf>,
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeaturesConfig {
    #[serde(default)]
    pub mcp: bool,
    #[serde(default)]
    pub web_search: bool,
    #[serde(default)]
    pub web_browser: bool,
    #[serde(default)]
    pub voice: bool,
    #[serde(default)]
    pub proactive: bool,
    #[serde(default)]
    pub worktrees: bool,
}

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn global_config_path() -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        Some(home.join(".config").join("claude").join("config.json"))
    }

    pub fn local_config_path(cwd: &std::path::Path) -> Option<PathBuf> {
        let path = cwd.join(".claude").join("config.json");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    pub fn claude_md_path(cwd: &std::path::Path) -> PathBuf {
        cwd.join("CLAUDE.md")
    }

    pub fn load(cwd: &std::path::Path) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();

        if let Some(global_path) = Self::global_config_path() {
            if global_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&global_path) {
                    if let Ok(global) = serde_json::from_str::<RuntimeConfig>(&content) {
                        config.merge(global);
                    }
                }
            }
        }

        if let Some(local_path) = Self::local_config_path(cwd) {
            if let Ok(content) = std::fs::read_to_string(&local_path) {
                if let Ok(local) = serde_json::from_str::<RuntimeConfig>(&content) {
                    config.merge(local);
                }
            }
        }

        config
    }

    pub fn load_from_env(config: &mut RuntimeConfig) {
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            config.api.anthropic_api_key = Some(key);
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            config.api.openai_compat_api_key = Some(key);
        }
        if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
            config.api.openai_compat_base_url = Some(url);
        }
        if let Ok(model) = std::env::var("CLAUDE_MODEL") {
            config.model = model;
        }
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            config.api.base_url = url;
        }
    }
}

impl RuntimeConfig {
    pub fn merge(&mut self, other: RuntimeConfig) {
        if other.model != "claude-sonnet-4-20250514" {
            self.model = other.model;
        }
        if other.api.base_url != default_base_url() {
            self.api.base_url = other.api.base_url;
        }
        if other.api.anthropic_api_key.is_some() {
            self.api.anthropic_api_key = other.api.anthropic_api_key;
        }
        if other.api.max_tokens != 8192 {
            self.api.max_tokens = other.api.max_tokens;
        }
        if other.api.temperature.is_some() {
            self.api.temperature = other.api.temperature;
        }
        if other.permissions.mode != PermissionMode::Default {
            self.permissions.mode = other.permissions.mode;
        }
        self.permissions.allow.extend(other.permissions.allow);
        self.permissions.deny.extend(other.permissions.deny);
        for (name, server) in other.mcp_servers {
            self.mcp_servers.insert(name, server);
        }
        if other.features.mcp {
            self.features.mcp = true;
        }
        if other.features.web_search {
            self.features.web_search = true;
        }
        if other.features.voice {
            self.features.voice = true;
        }
    }
}
