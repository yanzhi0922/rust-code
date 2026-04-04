use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub kind: PluginKind,
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
    #[serde(default)]
    pub tools: Vec<ToolManifestEntry>,
    #[serde(default)]
    pub commands: Vec<CommandManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    #[default]
    Builtin,
    Bundled,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: HookEvent,
    pub handler: String,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PreCommand,
    PostCommand,
    SessionStart,
    SessionEnd,
    QueryStart,
    QueryEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifestEntry {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandManifestEntry {
    pub name: String,
    pub description: String,
    pub handler: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub root_dir: PathBuf,
}

impl LoadedPlugin {
    pub fn new(manifest: PluginManifest, root_dir: PathBuf) -> Self {
        Self { manifest, root_dir }
    }
}

pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    pub fn load(&mut self, plugin: LoadedPlugin) {
        self.plugins.push(plugin);
    }

    pub fn unload(&mut self, name: &str) {
        self.plugins.retain(|p| p.manifest.name != name);
    }

    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|p| p.manifest.name == name)
    }

    pub fn list(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub fn load_manifest(path: &Path) -> anyhow::Result<PluginManifest> {
        let content = std::fs::read_to_string(path)?;
        let manifest: PluginManifest = serde_json::from_str(&content)?;
        Ok(manifest)
    }

    pub fn collect_tools(&self) -> Vec<&ToolManifestEntry> {
        self.plugins
            .iter()
            .flat_map(|p| p.manifest.tools.iter())
            .collect()
    }

    pub fn collect_commands(&self) -> Vec<&CommandManifestEntry> {
        self.plugins
            .iter()
            .flat_map(|p| p.manifest.commands.iter())
            .collect()
    }
}

pub struct HookRunner {
    hooks_by_event: HashMap<HookEvent, Vec<(HookConfig, PathBuf)>>,
}

impl HookRunner {
    pub fn new() -> Self {
        Self {
            hooks_by_event: HashMap::new(),
        }
    }

    pub fn register(&mut self, event: HookEvent, config: HookConfig, plugin_dir: PathBuf) {
        self.hooks_by_event
            .entry(event)
            .or_default()
            .push((config, plugin_dir));
    }

    pub async fn run_hooks(
        &self,
        event: &HookEvent,
        context: &HookContext,
    ) -> HookResult {
        let Some(hooks) = self.hooks_by_event.get(event) else {
            return HookResult::Continue;
        };
        let mut sorted: Vec<_> = hooks.iter().collect();
        sorted.sort_by_key(|(c, _)| c.priority);

        for (config, _plugin_dir) in sorted {
            tracing::debug!(hook = %config.handler, event = ?event, "running hook");
            let _ = &config;
            let _ = &context;
        }

        HookResult::Continue
    }
}

impl Default for HookRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct HookContext {
    pub event: HookEvent,
    pub data: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResult {
    Continue,
    Abort { reason: String },
}
