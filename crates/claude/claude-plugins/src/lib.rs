//! Plugin system with JSON-RPC runtime and skill discovery.
//!
//! Loads plugin manifests, discovers bundled skills, inspects runtime
//! capabilities, and invokes plugin actions over JSON-RPC. Supports both
//! stdio and HTTP transports.

pub mod autoupdate;
pub mod blocklist;
pub mod dependency;
pub mod directories;
pub mod flagging;
pub mod git_availability;
pub mod hint_recommendation;
pub mod identifier;
pub mod installation;
pub mod load_agents;
pub mod load_commands;
pub mod load_hooks;
pub mod load_output_styles;
pub mod loader;
pub mod lsp_integration;
pub mod managed;
pub mod markdown_walker;
pub mod marketplace;
pub mod mcp_integration;
pub mod mcpb_handler;
pub mod options_storage;
pub mod orphan_filter;
pub mod policy;
pub mod reconciler;
pub mod refresh;
pub mod schemas;
pub mod startup_check;
pub mod telemetry;
pub mod validate;
pub mod versioning;
pub mod zip_cache;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use claude_mcp::McpConfig;
use claude_skills::{SkillDocument, SkillError};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Duration, timeout},
};
use walkdir::WalkDir;

pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
/// Directory name for plugin manifests.
pub const PLUGIN_MANIFEST_DIR: &str = ".codex-plugin";
/// Marker file used to opt a plugin out of discovery without deleting it.
pub const PLUGIN_DISABLED_MARKER: &str = ".remote-code-disabled";
/// Default protocol version for plugin runtime communication.
pub const DEFAULT_PLUGIN_RUNTIME_PROTOCOL_VERSION: &str = "2026-04-07";
/// Default timeout for the runtime handshake in seconds.
pub const DEFAULT_PLUGIN_HANDSHAKE_TIMEOUT_SECS: u64 = 10;
/// Default timeout for individual runtime requests in seconds.
pub const DEFAULT_PLUGIN_REQUEST_TIMEOUT_SECS: u64 = 15;

/// Plugin manifest loaded from `.codex-plugin/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional author information.
    #[serde(default)]
    pub author: Option<PluginAuthor>,
    /// Optional homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Optional repository URL.
    #[serde(default)]
    pub repository: Option<String>,
    /// Optional license identifier.
    #[serde(default)]
    pub license: Option<String>,
    /// Keywords for discovery.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Relative path to the skills directory.
    #[serde(default)]
    pub skills: Option<String>,
    /// Relative path to the hooks configuration.
    #[serde(default)]
    pub hooks: Option<String>,
    /// Relative path to the apps directory.
    #[serde(default)]
    pub apps: Option<String>,
    /// Relative path to the MCP configuration.
    #[serde(default, alias = "mcpServers")]
    pub mcp: Option<String>,
    /// Optional interface metadata.
    #[serde(default)]
    pub interface: Option<PluginInterface>,
    /// Optional runtime configuration.
    #[serde(default)]
    pub runtime: Option<PluginRuntimeConfig>,
    /// Additional output style files or directories relative to the plugin root.
    #[serde(default, rename = "outputStyles", alias = "output_styles")]
    pub output_styles: Option<Value>,
}

/// Author information for a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAuthor {
    /// Author name.
    pub name: String,
    /// Author email.
    #[serde(default)]
    pub email: Option<String>,
    /// Author URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// Interface metadata for display in plugin registries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInterface {
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "shortDescription")]
    pub short_description: String,
    #[serde(rename = "longDescription")]
    pub long_description: Option<String>,
    #[serde(rename = "developerName")]
    pub developer_name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,
    #[serde(rename = "websiteURL")]
    pub website_url: Option<String>,
    #[serde(rename = "privacyPolicyURL")]
    pub privacy_policy_url: Option<String>,
    #[serde(rename = "termsOfServiceURL")]
    pub terms_of_service_url: Option<String>,
    #[serde(rename = "defaultPrompt", default)]
    pub default_prompt: Vec<String>,
    #[serde(rename = "composerIcon")]
    pub composer_icon: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(rename = "brandColor")]
    pub brand_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRuntimeConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub handshake_timeout_secs: Option<u64>,
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPluginRuntimeConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub handshake_timeout_secs: u64,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCapability {
    Read,
    Write,
    Interactive,
    Background,
    Network,
    Unknown(String),
}

impl Serialize for PluginCapability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Interactive => "Interactive",
            Self::Background => "Background",
            Self::Network => "Network",
            Self::Unknown(value) => value,
        })
    }
}

impl<'de> Deserialize<'de> for PluginCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "Read" => Self::Read,
            "Write" => Self::Write,
            "Interactive" => Self::Interactive,
            "Background" => Self::Background,
            "Network" => Self::Network,
            _ => Self::Unknown(value),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginBundle {
    pub manifest: PluginManifest,
    pub manifest_path: PathBuf,
    pub root: PathBuf,
}

impl PluginBundle {
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.root.join(PLUGIN_DISABLED_MARKER).exists()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginValidationReport {
    pub plugin_name: String,
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    pub bundled_skills: usize,
    pub has_runtime: bool,
    pub has_mcp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostInfo {
    pub name: String,
    pub version: String,
}

impl PluginHostInfo {
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

impl Default for PluginHostInfo {
    fn default() -> Self {
        Self::new("remote-code-rust", env!("CARGO_PKG_VERSION"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPeerInfo {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeActionDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeInspection {
    pub plugin_name: String,
    pub protocol_version: String,
    #[serde(default)]
    pub plugin_info: Option<PluginPeerInfo>,
    #[serde(default)]
    pub actions: Vec<PluginRuntimeActionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInvokeResult {
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInvokeResponse {
    pub plugin_name: String,
    pub action: String,
    pub protocol_version: String,
    #[serde(default)]
    pub plugin_info: Option<PluginPeerInfo>,
    pub result: PluginInvokeResult,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("failed to read plugin manifest `{path}`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse plugin manifest `{path}`")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Error)]
pub enum PluginRuntimeError {
    #[error("plugin `{plugin}` does not define a runtime adapter configuration")]
    MissingRuntimeConfig { plugin: String },
    #[error("failed to spawn plugin runtime for `{plugin}` using `{command}`")]
    Spawn {
        plugin: String,
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("plugin runtime for `{plugin}` did not expose {pipe}")]
    MissingPipe { plugin: String, pipe: &'static str },
    #[error("failed to serialize JSON-RPC payload for plugin `{plugin}` during {phase}")]
    Serialize {
        plugin: String,
        phase: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to write to plugin `{plugin}` during {phase}")]
    Write {
        plugin: String,
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read from plugin `{plugin}` during {phase}")]
    Read {
        plugin: String,
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for plugin `{plugin}` during {phase} after {timeout_secs}s")]
    Timeout {
        plugin: String,
        phase: &'static str,
        timeout_secs: u64,
    },
    #[error("plugin `{plugin}` closed stdout while waiting for {phase}")]
    Closed { plugin: String, phase: &'static str },
    #[error("failed to decode JSON from plugin `{plugin}` during {phase}")]
    Decode {
        plugin: String,
        phase: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("plugin `{plugin}` returned an invalid response during {phase}: {message}")]
    Protocol {
        plugin: String,
        phase: &'static str,
        message: String,
    },
    #[error("plugin `{plugin}` returned JSON-RPC error {code}: {message}")]
    Rpc {
        plugin: String,
        code: i64,
        message: String,
    },
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification<T> {
    jsonrpc: &'static str,
    method: &'static str,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcEnvelope {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcErrorPayload {
    code: i64,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginInitializeParams<'a> {
    protocol_version: &'a str,
    host_info: &'a PluginHostInfo,
    plugin: PluginIdentity<'a>,
}

#[derive(Debug, Serialize)]
struct PluginIdentity<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginInitializeResult {
    #[serde(default)]
    protocol_version: String,
    #[serde(default)]
    plugin_info: Option<PluginPeerInfo>,
    #[serde(default)]
    actions: Vec<PluginRuntimeActionDescriptor>,
}

#[derive(Debug, Serialize)]
struct PluginInvokeParams<'a> {
    action: &'a str,
    input: Value,
}

struct PluginRuntimeSession {
    plugin_name: String,
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    initialized: PluginInitializeResult,
    request_timeout_secs: u64,
}

pub fn discover_plugins(root: &Path) -> Result<Vec<PluginBundle>, PluginError> {
    discover_plugins_with_mode(root, false)
}

pub fn discover_plugins_including_disabled(root: &Path) -> Result<Vec<PluginBundle>, PluginError> {
    discover_plugins_with_mode(root, true)
}

fn discover_plugins_with_mode(
    root: &Path,
    include_disabled: bool,
) -> Result<Vec<PluginBundle>, PluginError> {
    let mut plugins = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.file_name() == PLUGIN_MANIFEST_FILE)
        .map(|entry| load_plugin(entry.path()))
        .collect::<Result<Vec<_>, _>>()?;

    if !include_disabled {
        plugins.retain(|plugin| !plugin.is_disabled());
    }

    plugins.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
    Ok(plugins)
}

pub fn load_plugin(path: impl AsRef<Path>) -> Result<PluginBundle, PluginError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| PluginError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest = serde_json::from_str(&content).map_err(|source| PluginError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(PluginBundle {
        manifest,
        manifest_path: path.to_path_buf(),
        root: resolve_plugin_root(path),
    })
}

pub fn load_plugin_from_root(root: impl AsRef<Path>) -> Result<PluginBundle, PluginError> {
    let root = root.as_ref();
    load_plugin(root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE))
}

pub fn validate_plugin_bundle(plugin: &PluginBundle) -> PluginValidationReport {
    let mut report = PluginValidationReport {
        plugin_name: plugin.manifest.name.clone(),
        manifest_path: plugin.manifest_path.clone(),
        has_runtime: plugin.runtime_config().is_some(),
        has_mcp: plugin.mcp_config_path().is_some(),
        ..PluginValidationReport::default()
    };

    if plugin.manifest.name.trim().is_empty() {
        report
            .errors
            .push("plugin name must not be empty".to_owned());
    }
    if plugin.manifest.version.trim().is_empty() {
        report
            .errors
            .push("plugin version must not be empty".to_owned());
    }
    if plugin.is_disabled() {
        report
            .warnings
            .push("plugin is currently disabled by marker file".to_owned());
    }

    if let Some(skills_root) = plugin.skills_root() {
        if !skills_root.exists() {
            report.errors.push(format!(
                "skills directory {} does not exist",
                skills_root.display()
            ));
        } else {
            match plugin.discover_bundled_skills() {
                Ok(skills) => {
                    report.bundled_skills = skills.len();
                    if skills.is_empty() {
                        report
                            .warnings
                            .push("skills path exists but no SKILL.md files were found".to_owned());
                    }
                }
                Err(error) => report
                    .errors
                    .push(format!("failed to discover bundled skills: {error}")),
            }
        }
    }

    if let Some(hooks_path) = plugin.hooks_config_path()
        && !hooks_path.exists()
    {
        report.errors.push(format!(
            "hooks config {} does not exist",
            hooks_path.display()
        ));
    }
    if let Some(app_path) = plugin.app_manifest_path()
        && !app_path.exists()
    {
        report.errors.push(format!(
            "app manifest {} does not exist",
            app_path.display()
        ));
    }
    if let Some(mcp_path) = plugin.mcp_config_path() {
        if !mcp_path.exists() {
            report
                .errors
                .push(format!("MCP config {} does not exist", mcp_path.display()));
        } else if let Err(error) = plugin.load_mcp_config() {
            report
                .errors
                .push(format!("failed to parse MCP config: {error}"));
        }
    }

    if let Some(runtime) = plugin.runtime_config() {
        if runtime.command.trim().is_empty() {
            report
                .errors
                .push("runtime command must not be empty".to_owned());
        }
        if !runtime.cwd.exists() {
            report.errors.push(format!(
                "runtime cwd {} does not exist",
                runtime.cwd.display()
            ));
        } else if !runtime.cwd.is_dir() {
            report.errors.push(format!(
                "runtime cwd {} is not a directory",
                runtime.cwd.display()
            ));
        }
    }

    if plugin.skills_root().is_none()
        && plugin.hooks_config_path().is_none()
        && plugin.app_manifest_path().is_none()
        && plugin.mcp_config_path().is_none()
        && plugin.runtime_config().is_none()
    {
        report.warnings.push(
            "plugin does not expose runtime, skills, hooks, apps, or MCP surfaces".to_owned(),
        );
    }

    report
}

pub async fn inspect_runtime(
    plugin: &PluginBundle,
    host_info: &PluginHostInfo,
) -> Result<PluginRuntimeInspection, PluginRuntimeError> {
    let runtime =
        plugin
            .runtime_config()
            .ok_or_else(|| PluginRuntimeError::MissingRuntimeConfig {
                plugin: plugin.manifest.name.clone(),
            })?;
    let mut session = PluginRuntimeSession::connect(plugin, &runtime, host_info).await?;
    let inspection = session.inspect();
    session.shutdown().await;
    Ok(inspection)
}

pub async fn invoke_runtime(
    plugin: &PluginBundle,
    host_info: &PluginHostInfo,
    action: &str,
    input: Value,
) -> Result<PluginInvokeResponse, PluginRuntimeError> {
    let runtime =
        plugin
            .runtime_config()
            .ok_or_else(|| PluginRuntimeError::MissingRuntimeConfig {
                plugin: plugin.manifest.name.clone(),
            })?;
    let mut session = PluginRuntimeSession::connect(plugin, &runtime, host_info).await?;
    let response = session.invoke(action, input).await;
    session.shutdown().await;
    response
}

pub async fn inspect_plugin_runtime(
    plugin: &PluginBundle,
    host_info: &PluginHostInfo,
) -> Result<PluginRuntimeInspection, PluginRuntimeError> {
    inspect_runtime(plugin, host_info).await
}

pub async fn invoke_plugin_action(
    plugin: &PluginBundle,
    host_info: &PluginHostInfo,
    action: &str,
    input: Value,
) -> Result<PluginInvokeResponse, PluginRuntimeError> {
    invoke_runtime(plugin, host_info, action, input).await
}

impl PluginBundle {
    #[must_use]
    pub fn skills_root(&self) -> Option<PathBuf> {
        self.manifest
            .skills
            .as_deref()
            .map(|relative| self.resolve_relative(relative))
    }

    #[must_use]
    pub fn app_manifest_path(&self) -> Option<PathBuf> {
        self.manifest
            .apps
            .as_deref()
            .map(|relative| self.resolve_relative(relative))
    }

    #[must_use]
    pub fn hooks_config_path(&self) -> Option<PathBuf> {
        self.manifest
            .hooks
            .as_deref()
            .map(|relative| self.resolve_relative(relative))
    }

    #[must_use]
    pub fn mcp_config_path(&self) -> Option<PathBuf> {
        self.manifest
            .mcp
            .as_deref()
            .map(|relative| self.resolve_relative(relative))
    }

    #[must_use]
    pub fn default_output_styles_path(&self) -> Option<PathBuf> {
        let path = self.root.join("output-styles");
        path.exists().then_some(path)
    }

    #[must_use]
    pub fn output_styles_paths(&self) -> Vec<PathBuf> {
        let Some(raw) = &self.manifest.output_styles else {
            return Vec::new();
        };
        match raw {
            Value::String(relative) => vec![self.resolve_relative(relative)],
            Value::Array(values) => values
                .iter()
                .filter_map(Value::as_str)
                .map(|relative| self.resolve_relative(relative))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn runtime_config(&self) -> Option<ResolvedPluginRuntimeConfig> {
        self.manifest.runtime.as_ref().map(|runtime| {
            let cwd = runtime
                .cwd
                .as_ref()
                .map(|cwd| {
                    if cwd.is_absolute() {
                        cwd.clone()
                    } else {
                        self.root.join(cwd)
                    }
                })
                .unwrap_or_else(|| self.root.clone());
            ResolvedPluginRuntimeConfig {
                command: runtime.command.clone(),
                args: runtime.args.clone(),
                cwd,
                env: runtime.env.clone(),
                handshake_timeout_secs: runtime
                    .handshake_timeout_secs
                    .unwrap_or(DEFAULT_PLUGIN_HANDSHAKE_TIMEOUT_SECS)
                    .max(1),
                request_timeout_secs: runtime
                    .request_timeout_secs
                    .unwrap_or(DEFAULT_PLUGIN_REQUEST_TIMEOUT_SECS)
                    .max(1),
            }
        })
    }

    pub fn discover_bundled_skills(&self) -> Result<Vec<SkillDocument>, SkillError> {
        match self.skills_root() {
            Some(root) => claude_skills::discover_skills(&root),
            None => Ok(Vec::new()),
        }
    }

    pub fn load_mcp_config(&self) -> Result<Option<McpConfig>, claude_mcp::McpConfigError> {
        match self.mcp_config_path() {
            Some(path) => McpConfig::load(path).map(Some),
            None => Ok(None),
        }
    }

    fn resolve_relative(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl PluginRuntimeSession {
    async fn connect(
        plugin: &PluginBundle,
        runtime: &ResolvedPluginRuntimeConfig,
        host_info: &PluginHostInfo,
    ) -> Result<Self, PluginRuntimeError> {
        let mut process = Command::new(&runtime.command);
        process
            .args(&runtime.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(&runtime.cwd)
            .kill_on_drop(true);
        if !runtime.env.is_empty() {
            process.envs(&runtime.env);
        }

        let mut child = process
            .spawn()
            .map_err(|source| PluginRuntimeError::Spawn {
                plugin: plugin.manifest.name.clone(),
                command: runtime.command.clone(),
                source,
            })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginRuntimeError::MissingPipe {
                plugin: plugin.manifest.name.clone(),
                pipe: "stdin",
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginRuntimeError::MissingPipe {
                plugin: plugin.manifest.name.clone(),
                pipe: "stdout",
            })?;
        let mut lines = BufReader::new(stdout).lines();

        let initialize = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: PluginInitializeParams {
                protocol_version: DEFAULT_PLUGIN_RUNTIME_PROTOCOL_VERSION,
                host_info,
                plugin: PluginIdentity {
                    name: &plugin.manifest.name,
                    version: &plugin.manifest.version,
                },
            },
        };
        write_message(
            &mut stdin,
            &plugin.manifest.name,
            "initialize request",
            &initialize,
        )
        .await?;
        let initialized: PluginInitializeResult = wait_for_response(
            &mut lines,
            &plugin.manifest.name,
            1,
            "initialize response",
            runtime.handshake_timeout_secs,
        )
        .await?;
        if initialized.protocol_version.trim().is_empty() {
            return Err(PluginRuntimeError::Protocol {
                plugin: plugin.manifest.name.clone(),
                phase: "initialize response",
                message: "protocolVersion was empty".to_owned(),
            });
        }

        let ready = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: serde_json::json!({}),
        };
        write_message(
            &mut stdin,
            &plugin.manifest.name,
            "initialized notification",
            &ready,
        )
        .await?;

        Ok(Self {
            plugin_name: plugin.manifest.name.clone(),
            child,
            stdin,
            lines,
            initialized,
            request_timeout_secs: runtime.request_timeout_secs,
        })
    }

    fn inspect(&self) -> PluginRuntimeInspection {
        PluginRuntimeInspection {
            plugin_name: self.plugin_name.clone(),
            protocol_version: self.initialized.protocol_version.clone(),
            plugin_info: self.initialized.plugin_info.clone(),
            actions: self.initialized.actions.clone(),
        }
    }

    async fn invoke(
        &mut self,
        action: &str,
        input: Value,
    ) -> Result<PluginInvokeResponse, PluginRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "plugin/invoke",
            params: PluginInvokeParams { action, input },
        };
        write_message(
            &mut self.stdin,
            &self.plugin_name,
            "plugin/invoke request",
            &request,
        )
        .await?;
        let result: PluginInvokeResult = wait_for_response(
            &mut self.lines,
            &self.plugin_name,
            2,
            "plugin/invoke response",
            self.request_timeout_secs,
        )
        .await?;

        Ok(PluginInvokeResponse {
            plugin_name: self.plugin_name.clone(),
            action: action.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            plugin_info: self.initialized.plugin_info.clone(),
            result,
        })
    }

    async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

async fn write_message<T: Serialize>(
    stdin: &mut ChildStdin,
    plugin: &str,
    phase: &'static str,
    payload: &T,
) -> Result<(), PluginRuntimeError> {
    let mut body = serde_json::to_vec(payload).map_err(|source| PluginRuntimeError::Serialize {
        plugin: plugin.to_owned(),
        phase,
        source,
    })?;
    body.push(b'\n');
    stdin
        .write_all(&body)
        .await
        .map_err(|source| PluginRuntimeError::Write {
            plugin: plugin.to_owned(),
            phase,
            source,
        })?;
    stdin
        .flush()
        .await
        .map_err(|source| PluginRuntimeError::Write {
            plugin: plugin.to_owned(),
            phase,
            source,
        })
}

async fn wait_for_response<T: DeserializeOwned>(
    lines: &mut Lines<BufReader<ChildStdout>>,
    plugin: &str,
    request_id: u64,
    phase: &'static str,
    timeout_secs: u64,
) -> Result<T, PluginRuntimeError> {
    timeout(Duration::from_secs(timeout_secs), async {
        loop {
            let line = lines
                .next_line()
                .await
                .map_err(|source| PluginRuntimeError::Read {
                    plugin: plugin.to_owned(),
                    phase,
                    source,
                })?;
            let Some(line) = line else {
                return Err(PluginRuntimeError::Closed {
                    plugin: plugin.to_owned(),
                    phase,
                });
            };
            if line.trim().is_empty() {
                continue;
            }
            let envelope: JsonRpcEnvelope =
                serde_json::from_str(&line).map_err(|source| PluginRuntimeError::Decode {
                    plugin: plugin.to_owned(),
                    phase,
                    source,
                })?;
            let Some(id) = envelope.id.as_ref() else {
                continue;
            };
            if !rpc_id_matches(id, request_id) {
                continue;
            }
            if let Some(error) = envelope.error {
                return Err(PluginRuntimeError::Rpc {
                    plugin: plugin.to_owned(),
                    code: error.code,
                    message: error.message,
                });
            }
            let result = envelope
                .result
                .ok_or_else(|| PluginRuntimeError::Protocol {
                    plugin: plugin.to_owned(),
                    phase,
                    message: "response did not include a result payload".to_owned(),
                })?;
            return serde_json::from_value(result).map_err(|source| PluginRuntimeError::Decode {
                plugin: plugin.to_owned(),
                phase,
                source,
            });
        }
    })
    .await
    .map_err(|_| PluginRuntimeError::Timeout {
        plugin: plugin.to_owned(),
        phase,
        timeout_secs,
    })?
}

fn rpc_id_matches(id: &Value, request_id: u64) -> bool {
    id.as_u64() == Some(request_id)
        || id.as_i64() == Some(request_id as i64)
        || id
            .as_str()
            .is_some_and(|value| value == request_id.to_string())
}

fn resolve_plugin_root(manifest_path: &Path) -> PathBuf {
    let Some(parent) = manifest_path.parent() else {
        return PathBuf::from(".");
    };
    if parent
        .file_name()
        .is_some_and(|name| name == PLUGIN_MANIFEST_DIR)
    {
        return parent
            .parent()
            .map_or_else(|| parent.to_path_buf(), Path::to_path_buf);
    }
    parent.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn loads_plugin_manifest_and_resolves_paths() {
        let temp = ok(tempdir());
        let root = temp.path().join("github-plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{
                "name": "github",
                "version": "0.1.0",
                "skills": "./skills",
                "hooks": "./hooks.json",
                "apps": "./.app.json",
                "mcpServers": "./mcp.toml",
                "runtime": {
                    "command": "python",
                    "args": ["adapter.py"],
                    "cwd": "./adapter"
                },
                "interface": {
                    "displayName": "GitHub",
                    "shortDescription": "Triage GitHub work",
                    "capabilities": ["Interactive", "Write", "ExperimentalCapability"],
                    "defaultPrompt": ["Help with GitHub"]
                }
            }"#,
        ));

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));

        assert_eq!(plugin.root, root);
        assert_eq!(plugin.manifest.name, "github");
        assert_eq!(plugin.skills_root(), Some(plugin.root.join("./skills")));
        assert_eq!(plugin.manifest.hooks.as_deref(), Some("./hooks.json"));
        assert_eq!(
            plugin.app_manifest_path(),
            Some(plugin.root.join("./.app.json"))
        );
        assert_eq!(
            plugin.hooks_config_path(),
            Some(plugin.root.join("./hooks.json"))
        );
        assert_eq!(
            plugin.mcp_config_path(),
            Some(plugin.root.join("./mcp.toml"))
        );
        let runtime = plugin
            .runtime_config()
            .unwrap_or_else(|| panic!("missing runtime config"));
        assert_eq!(runtime.command, "python");
        assert_eq!(runtime.args, vec!["adapter.py"]);
        assert_eq!(runtime.cwd, plugin.root.join("./adapter"));
        let interface = match plugin.manifest.interface {
            Some(interface) => interface,
            None => panic!("missing interface"),
        };
        assert_eq!(
            interface.capabilities,
            vec![
                PluginCapability::Interactive,
                PluginCapability::Write,
                PluginCapability::Unknown("ExperimentalCapability".to_owned())
            ]
        );
    }

    #[test]
    fn discovers_plugins_sorted_by_name() {
        let temp = ok(tempdir());
        let alpha = temp.path().join("alpha");
        let zeta = temp.path().join("zeta");
        ok(fs::create_dir_all(alpha.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::create_dir_all(zeta.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::write(
            alpha.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"alpha","version":"0.1.0"}"#,
        ));
        ok(fs::write(
            zeta.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"zeta","version":"0.1.0"}"#,
        ));

        let plugins = ok(discover_plugins(temp.path()));

        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].manifest.name, "alpha");
        assert_eq!(plugins[1].manifest.name, "zeta");
    }

    #[test]
    fn discover_plugins_skips_disabled_unless_requested() {
        let temp = ok(tempdir());
        let alpha = temp.path().join("alpha");
        let disabled = temp.path().join("disabled");
        ok(fs::create_dir_all(alpha.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::create_dir_all(disabled.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::write(
            alpha.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"alpha","version":"0.1.0"}"#,
        ));
        ok(fs::write(
            disabled
                .join(PLUGIN_MANIFEST_DIR)
                .join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"disabled","version":"0.1.0"}"#,
        ));
        ok(fs::write(
            disabled.join(PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        ));

        let enabled_only = ok(discover_plugins(temp.path()));
        assert_eq!(enabled_only.len(), 1);
        assert_eq!(enabled_only[0].manifest.name, "alpha");

        let with_disabled = ok(discover_plugins_including_disabled(temp.path()));
        assert_eq!(with_disabled.len(), 2);
        assert!(with_disabled.iter().any(|plugin| plugin.is_disabled()));
    }

    #[test]
    fn discovers_bundled_skills() {
        let temp = ok(tempdir());
        let root = temp.path().join("bundle");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::create_dir_all(root.join("skills").join("demo")));
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"bundle","version":"0.1.0","skills":"./skills"}"#,
        ));
        ok(fs::write(
            root.join("skills").join("demo").join("SKILL.md"),
            "# Demo\n\nDemo summary.\n",
        ));

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        let skills = ok(plugin.discover_bundled_skills());

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].metadata.slug, "demo");
    }

    #[test]
    fn loads_plugin_mcp_config() {
        let temp = ok(tempdir());
        let root = temp.path().join("mcp-plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"mcp-plugin","version":"0.1.0","mcp":"./mcp.toml"}"#,
        ));
        ok(fs::write(
            root.join("mcp.toml"),
            "[mcp_servers.demo]\ncommand = \"uvx\"\n",
        ));

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        let config = ok(plugin.load_mcp_config());
        let config = match config {
            Some(config) => config,
            None => panic!("missing MCP config"),
        };

        assert!(config.servers.contains_key("demo"));
    }

    #[test]
    fn runtime_config_is_optional() {
        let temp = ok(tempdir());
        let root = temp.path().join("plain-plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{"name":"plain","version":"0.1.0"}"#,
        ));

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        assert!(plugin.runtime_config().is_none());
    }

    #[test]
    fn loads_plugin_from_root_and_validates_bundle() {
        let temp = ok(tempdir());
        let root = temp.path().join("bundle");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        ok(fs::create_dir_all(root.join("skills").join("demo")));
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            r#"{
                "name":"bundle",
                "version":"0.1.0",
                "skills":"./skills",
                "runtime":{"command":"python","cwd":"."}
            }"#,
        ));
        ok(fs::write(
            root.join("skills").join("demo").join("SKILL.md"),
            "# Demo\n\nHello.\n",
        ));

        let plugin = ok(load_plugin_from_root(&root));
        let report = validate_plugin_bundle(&plugin);
        assert_eq!(plugin.manifest.name, "bundle");
        assert!(
            report.errors.is_empty(),
            "validation errors: {:?}",
            report.errors
        );
        assert_eq!(report.bundled_skills, 1);
        assert!(report.has_runtime);
    }

    #[tokio::test]
    async fn inspects_runtime_and_invokes_action() {
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping plugin runtime test because Python is unavailable.");
            return;
        };

        let temp = ok(tempdir());
        let root = temp.path().join("plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        let script = root.join("adapter.py");
        ok(fs::write(&script, mock_plugin_runtime_script()));
        prefix_args.push("adapter.py".to_owned());
        prefix_args.push("success".to_owned());

        write_runtime_manifest(&root, &python, &prefix_args);

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        let host = PluginHostInfo::new("remote-code-rust", "test");

        let inspection = inspect_runtime(&plugin, &host)
            .await
            .unwrap_or_else(|error| panic!("inspection failed: {error}"));
        assert_eq!(inspection.plugin_name, "demo-plugin");
        assert_eq!(
            inspection.protocol_version,
            DEFAULT_PLUGIN_RUNTIME_PROTOCOL_VERSION
        );
        assert_eq!(inspection.actions.len(), 1);
        assert_eq!(inspection.actions[0].name, "echo");

        let response = invoke_runtime(&plugin, &host, "echo", serde_json::json!({"text": "hello"}))
            .await
            .unwrap_or_else(|error| panic!("invoke failed: {error}"));
        assert_eq!(response.action, "echo");
        assert_eq!(response.plugin_name, "demo-plugin");
        assert!(!response.result.is_error);
        assert_eq!(
            response.result.output,
            serde_json::json!({"echoed": "hello"})
        );
    }

    #[tokio::test]
    async fn surfaces_runtime_protocol_errors() {
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping plugin runtime protocol test because Python is unavailable.");
            return;
        };

        let temp = ok(tempdir());
        let root = temp.path().join("plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        let script = root.join("adapter.py");
        ok(fs::write(&script, mock_plugin_runtime_script()));
        prefix_args.push("adapter.py".to_owned());
        prefix_args.push("protocol_error".to_owned());

        write_runtime_manifest(&root, &python, &prefix_args);

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        let error = inspect_runtime(&plugin, &PluginHostInfo::default())
            .await
            .expect_err("protocol error should surface");
        assert!(matches!(
            error,
            PluginRuntimeError::Protocol { phase, .. } if phase == "initialize response"
        ));
    }

    #[tokio::test]
    async fn surfaces_runtime_rpc_errors() {
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping plugin runtime RPC test because Python is unavailable.");
            return;
        };

        let temp = ok(tempdir());
        let root = temp.path().join("plugin");
        ok(fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)));
        let script = root.join("adapter.py");
        ok(fs::write(&script, mock_plugin_runtime_script()));
        prefix_args.push("adapter.py".to_owned());
        prefix_args.push("rpc_error".to_owned());

        write_runtime_manifest(&root, &python, &prefix_args);

        let plugin = ok(load_plugin(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
        ));
        let error = invoke_runtime(
            &plugin,
            &PluginHostInfo::default(),
            "echo",
            serde_json::json!({"text": "boom"}),
        )
        .await
        .expect_err("RPC error should surface");
        assert!(matches!(
            error,
            PluginRuntimeError::Rpc {
                code: -32001,
                ref message,
                ..
            } if message == "invoke failed"
        ));
    }

    fn write_runtime_manifest(root: &Path, command: &str, args: &[String]) {
        ok(fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE),
            format!(
                r#"{{
                    "name": "demo-plugin",
                    "version": "0.1.0",
                    "runtime": {{
                        "command": "{command}",
                        "args": [{args}],
                        "cwd": "."
                    }}
                }}"#,
                command = command,
                args = args
                    .iter()
                    .map(|arg| format!(r#""{}""#, arg.replace('\\', "\\\\").replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    fn python_command() -> Option<(String, Vec<String>)> {
        let probe = |cmd: &str, args: &[&str]| -> bool {
            let mut cmd = ProcessCommand::new(cmd);
            cmd.args(args).args(["-c", "import json"]);
            cmd.output().is_ok_and(|output| output.status.success())
        };

        if let Ok(path) = std::env::var("PYTHON")
            && probe(&path, &[])
        {
            return Some((path, Vec::new()));
        }

        for candidate in ["python", "python3"] {
            if probe(candidate, &[]) {
                return Some((candidate.to_owned(), Vec::new()));
            }
        }

        if cfg!(windows) && probe("py", &["-3"]) {
            return Some(("py".to_owned(), vec!["-3".to_owned()]));
        }

        None
    }

    fn mock_plugin_runtime_script() -> &'static str {
        r#"
import json
import sys

mode = sys.argv[1] if len(sys.argv) > 1 else "success"

while True:
    raw = sys.stdin.readline()
    if not raw:
        break
    raw = raw.strip()
    if not raw:
        continue
    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        if mode == "protocol_error":
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "pluginInfo": {"name": "demo-adapter", "version": "0.1.0"},
                    "actions": [{"name": "echo"}]
                }
            }), flush=True)
        else:
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "protocolVersion": "2026-04-07",
                    "pluginInfo": {
                        "name": "demo-adapter",
                        "title": "Demo Adapter",
                        "version": "0.1.0"
                    },
                    "actions": [{
                        "name": "echo",
                        "description": "Echo a text payload",
                        "inputSchema": {"type": "object"}
                    }]
                }
            }), flush=True)
    elif method == "notifications/initialized":
        continue
    elif method == "plugin/invoke":
        if mode == "rpc_error":
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "error": {"code": -32001, "message": "invoke failed"}
            }), flush=True)
        else:
            text = message["params"]["input"]["text"]
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "output": {"echoed": text},
                    "isError": False
                }
            }), flush=True)
        break
"#
    }
}
