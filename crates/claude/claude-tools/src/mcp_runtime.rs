use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use claude_config::{RuntimeConfig, SettingSource};
use claude_mcp::{McpClientInfo, McpServerInspection, inspect_server};
use claude_ui_bridge::{
    UiRuntimeMcpInventorySummary, UiRuntimeMcpOriginCounts, UiRuntimeMcpServerStatus,
    UiRuntimeMcpStatusCounts,
};

use crate::{RuntimeMcpServerPolicyEntry, ToolRuntimePolicy};

#[derive(Debug, Clone)]
pub struct RuntimeMcpServerEntry {
    pub origin_kind: &'static str,
    pub origin_name: String,
    pub config_path: PathBuf,
    pub server: claude_mcp::McpServerConfig,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeMcpDiscovery {
    pub servers: Vec<RuntimeMcpServerEntry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeMcpResolvedEntry {
    pub entry: RuntimeMcpServerEntry,
    pub shadowed_entries: Vec<RuntimeMcpServerEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeMcpResolvedDiscovery {
    pub servers: Vec<RuntimeMcpResolvedEntry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeMcpServerObservation {
    pub entry: RuntimeMcpServerEntry,
    pub status: UiRuntimeMcpServerStatus,
    pub inspection: Option<McpServerInspection>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeMcpObservation {
    pub servers: Vec<RuntimeMcpServerObservation>,
    pub warnings: Vec<String>,
}

impl RuntimeMcpDiscovery {
    pub fn enabled_server_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|entry| entry.server.enabled)
            .map(|entry| entry.server.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn disabled_server_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|entry| !entry.server.enabled)
            .map(|entry| entry.server.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn into_policy_entries(self) -> Vec<RuntimeMcpServerPolicyEntry> {
        self.servers
            .into_iter()
            .map(|entry| RuntimeMcpServerPolicyEntry {
                origin_kind: entry.origin_kind.to_owned(),
                origin_name: entry.origin_name,
                config_path: entry.config_path,
                server: entry.server,
            })
            .collect()
    }

    #[must_use]
    pub fn inventory_summary(&self) -> UiRuntimeMcpInventorySummary {
        summarize_runtime_mcp(&self.servers, &self.warnings, |entry| {
            base_runtime_mcp_status(&entry.server)
        })
    }
}

impl RuntimeMcpObservation {
    #[must_use]
    pub fn inventory_summary(&self) -> UiRuntimeMcpInventorySummary {
        let unique_server_names = self
            .servers
            .iter()
            .map(|server| server.entry.server.name.as_str())
            .collect::<BTreeSet<_>>();
        let ambiguous_server_names = unique_server_names
            .iter()
            .filter(|name| {
                self.servers
                    .iter()
                    .filter(|server| server.entry.server.name == **name)
                    .nth(1)
                    .is_some()
            })
            .count();
        let mut origins = UiRuntimeMcpOriginCounts::default();
        let mut status_counts = UiRuntimeMcpStatusCounts::default();

        for server in &self.servers {
            match server.entry.origin_kind {
                "cwd" => origins.cwd += 1,
                "profile" => origins.profile += 1,
                "explicit" => origins.explicit += 1,
                "plugin" => origins.plugin += 1,
                _ => {}
            }
            accumulate_runtime_mcp_status_count(&mut status_counts, server.status);
        }

        UiRuntimeMcpInventorySummary {
            total_servers: self.servers.len(),
            enabled_servers: self
                .servers
                .iter()
                .filter(|server| server.entry.server.enabled)
                .count(),
            disabled_servers: self
                .servers
                .iter()
                .filter(|server| !server.entry.server.enabled)
                .count(),
            unique_server_names: unique_server_names.len(),
            ambiguous_server_names,
            warning_count: self.warnings.len(),
            origins,
            status_counts,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeMcpResolution {
    pub entry: RuntimeMcpServerEntry,
    pub shadowed_entries: Vec<RuntimeMcpServerEntry>,
    pub warnings: Vec<String>,
}

pub fn runtime_mcp_policy_entries(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
) -> Vec<RuntimeMcpServerPolicyEntry> {
    discover_runtime_mcp_servers(config, extra_config_paths).into_policy_entries()
}

#[must_use]
pub fn runtime_mcp_inventory_summary(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
) -> UiRuntimeMcpInventorySummary {
    discover_runtime_mcp_servers(config, extra_config_paths).inventory_summary()
}

pub async fn observe_runtime_mcp_servers(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
    connect: bool,
    client_info: &McpClientInfo,
) -> RuntimeMcpObservation {
    let discovery = discover_runtime_mcp_servers(config, extra_config_paths);
    let mut servers = Vec::with_capacity(discovery.servers.len());

    for entry in discovery.servers {
        let mut observation = RuntimeMcpServerObservation {
            status: base_runtime_mcp_status(&entry.server),
            entry,
            inspection: None,
            error: None,
        };

        if connect && observation.entry.server.enabled {
            match inspect_server(&observation.entry.server, client_info).await {
                Ok(inspection) => {
                    observation.status = UiRuntimeMcpServerStatus::Connected;
                    observation.inspection = Some(inspection);
                }
                Err(error) => {
                    observation.status = UiRuntimeMcpServerStatus::Failed;
                    observation.error = Some(error.to_string());
                }
            }
        }

        servers.push(observation);
    }

    RuntimeMcpObservation {
        servers,
        warnings: discovery.warnings,
    }
}

pub fn resolve_runtime_policy_mcp_server(
    policy: &ToolRuntimePolicy,
    server_name: &str,
) -> Result<RuntimeMcpServerPolicyEntry> {
    if policy.mcp_servers.is_empty() {
        return Err(anyhow!(
            "MCP runtime inventory is not configured for the current process"
        ));
    }

    let matches = policy
        .mcp_servers
        .iter()
        .filter(|entry| entry.server.name == server_name)
        .cloned()
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Err(anyhow!(
            "MCP server '{server_name}' is not available in the current runtime inventory"
        )),
        1 => {
            let entry = matches.into_iter().next().expect("single match");
            if !entry.server.enabled {
                return Err(anyhow!(
                    "MCP server '{server_name}' is disabled by the current runtime inventory"
                ));
            }
            Ok(entry)
        }
        _ => {
            let candidates = matches
                .into_iter()
                .map(|entry| {
                    format!(
                        "{}:{} ({})",
                        entry.origin_kind,
                        entry.origin_name,
                        entry.config_path.display()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(anyhow!(
                "MCP server '{server_name}' is ambiguous across: {candidates}"
            ))
        }
    }
}

pub fn resolve_runtime_mcp_server(
    config: &RuntimeConfig,
    server_name: &str,
    extra_config_paths: &[PathBuf],
) -> Result<RuntimeMcpResolution> {
    let discovery = discover_runtime_mcp_resolution(config, extra_config_paths);
    let match_entry = discovery
        .servers
        .iter()
        .find(|entry| entry.entry.server.name == server_name)
        .cloned();

    match match_entry {
        Some(entry) => Ok(RuntimeMcpResolution {
            entry: entry.entry,
            shadowed_entries: entry.shadowed_entries,
            warnings: discovery.warnings,
        }),
        None => Err(anyhow!("No MCP server named `{server_name}` was found")),
    }
}

pub fn discover_runtime_mcp_resolution(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
) -> RuntimeMcpResolvedDiscovery {
    let candidates = collect_runtime_mcp_candidates(config, extra_config_paths);
    RuntimeMcpResolvedDiscovery {
        servers: resolve_runtime_mcp_candidates(candidates.servers),
        warnings: candidates.warnings,
    }
}

pub fn discover_runtime_mcp_servers(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
) -> RuntimeMcpDiscovery {
    let resolved = discover_runtime_mcp_resolution(config, extra_config_paths);
    RuntimeMcpDiscovery {
        servers: resolved
            .servers
            .into_iter()
            .map(|entry| entry.entry)
            .collect(),
        warnings: resolved.warnings,
    }
}

fn collect_runtime_mcp_candidates(
    config: &RuntimeConfig,
    extra_config_paths: &[PathBuf],
) -> RuntimeMcpDiscovery {
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let mut loaded_paths = BTreeSet::new();
    let load_only_explicit = config.strict_mcp_config;

    if !load_only_explicit
        && setting_source_enabled(config, SettingSource::User)
        && config.paths.plugins_dir.exists()
    {
        match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
            Ok(plugins) => {
                for plugin in plugins {
                    if let Some(path) = plugin.mcp_config_path() {
                        if !loaded_paths.insert(path.clone()) {
                            continue;
                        }
                        match claude_mcp::McpConfig::load(&path) {
                            Ok(config) => append_runtime_mcp_servers(
                                &mut servers,
                                "plugin",
                                &plugin.manifest.name,
                                &path,
                                config,
                            ),
                            Err(error) => warnings.push(format!(
                                "Failed to load plugin MCP config for {}: {error}",
                                plugin.manifest.name
                            )),
                        }
                    }
                }
            }
            Err(error) => warnings.push(format!(
                "Failed to discover plugins for MCP inspection: {error}"
            )),
        }
    }

    if !load_only_explicit && setting_source_enabled(config, SettingSource::User) {
        load_runtime_mcp_candidates_in_dir(
            &mut servers,
            &mut warnings,
            &mut loaded_paths,
            "profile",
            &config.paths.profile_dir.display().to_string(),
            &config.paths.profile_dir,
        );
    }
    if !load_only_explicit && setting_source_enabled(config, SettingSource::Project) {
        load_runtime_project_mcp_hierarchy(
            &mut servers,
            &mut warnings,
            &mut loaded_paths,
            &config.cwd,
        );
    }
    for path in extra_config_paths {
        if path.is_dir() {
            load_runtime_mcp_candidates_in_dir(
                &mut servers,
                &mut warnings,
                &mut loaded_paths,
                "explicit",
                &path.display().to_string(),
                path,
            );
        } else {
            load_runtime_mcp_file(
                &mut servers,
                &mut warnings,
                &mut loaded_paths,
                "explicit",
                &path.display().to_string(),
                path,
            );
        }
    }

    RuntimeMcpDiscovery { servers, warnings }
}

fn resolve_runtime_mcp_candidates(
    candidates: Vec<RuntimeMcpServerEntry>,
) -> Vec<RuntimeMcpResolvedEntry> {
    let mut resolved = BTreeMap::<String, RuntimeMcpResolvedEntry>::new();

    for candidate in candidates {
        let server_name = candidate.server.name.clone();
        if let Some(existing) = resolved.get_mut(&server_name) {
            existing.shadowed_entries.push(existing.entry.clone());
            existing.entry = candidate;
            continue;
        }
        resolved.insert(
            server_name,
            RuntimeMcpResolvedEntry {
                entry: candidate,
                shadowed_entries: Vec::new(),
            },
        );
    }

    let mut resolved = resolved.into_values().collect::<Vec<_>>();
    resolved.sort_by(|left, right| {
        left.entry
            .server
            .name
            .cmp(&right.entry.server.name)
            .then_with(|| left.entry.origin_kind.cmp(right.entry.origin_kind))
            .then_with(|| left.entry.origin_name.cmp(&right.entry.origin_name))
    });
    resolved
}

fn project_directory_chain(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = cwd.to_path_buf();

    while current.parent().is_some() {
        dirs.push(current.clone());
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        }
    }

    dirs.reverse();
    dirs
}

fn load_runtime_project_mcp_hierarchy(
    servers: &mut Vec<RuntimeMcpServerEntry>,
    warnings: &mut Vec<String>,
    loaded_paths: &mut BTreeSet<PathBuf>,
    cwd: &Path,
) {
    let cwd_string = cwd.display().to_string();

    for dir in project_directory_chain(cwd) {
        if dir == cwd {
            load_runtime_mcp_file(
                servers,
                warnings,
                loaded_paths,
                "cwd",
                &cwd_string,
                &dir.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            );
        }
        load_runtime_mcp_file(
            servers,
            warnings,
            loaded_paths,
            "cwd",
            &cwd_string,
            &dir.join(claude_mcp::DEFAULT_PROJECT_MCP_CONFIG_FILE),
        );
    }
}

fn load_runtime_mcp_candidates_in_dir(
    servers: &mut Vec<RuntimeMcpServerEntry>,
    warnings: &mut Vec<String>,
    loaded_paths: &mut BTreeSet<PathBuf>,
    origin_kind: &'static str,
    origin_name: &str,
    dir: &Path,
) {
    let candidates = [
        dir.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
        dir.join(claude_mcp::DEFAULT_PROJECT_MCP_CONFIG_FILE),
    ];
    let existing = candidates
        .iter()
        .filter(|candidate| candidate.exists())
        .cloned()
        .collect::<Vec<_>>();

    if existing.is_empty() {
        if origin_kind == "explicit" {
            warnings.push(format!(
                "Explicit MCP config directory {} did not contain {} or {}",
                dir.display(),
                claude_mcp::DEFAULT_MCP_CONFIG_FILE,
                claude_mcp::DEFAULT_PROJECT_MCP_CONFIG_FILE,
            ));
        }
        return;
    }

    for candidate in existing {
        load_runtime_mcp_file(
            servers,
            warnings,
            loaded_paths,
            origin_kind,
            origin_name,
            &candidate,
        );
    }
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn base_runtime_mcp_status(server: &claude_mcp::McpServerConfig) -> UiRuntimeMcpServerStatus {
    if server.enabled {
        UiRuntimeMcpServerStatus::Pending
    } else {
        UiRuntimeMcpServerStatus::Disabled
    }
}

fn accumulate_runtime_mcp_status_count(
    counts: &mut UiRuntimeMcpStatusCounts,
    status: UiRuntimeMcpServerStatus,
) {
    match status {
        UiRuntimeMcpServerStatus::Connected => counts.connected += 1,
        UiRuntimeMcpServerStatus::Failed => counts.failed += 1,
        UiRuntimeMcpServerStatus::NeedsAuth => counts.needs_auth += 1,
        UiRuntimeMcpServerStatus::Pending => counts.pending += 1,
        UiRuntimeMcpServerStatus::Disabled => counts.disabled += 1,
    }
}

fn summarize_runtime_mcp(
    servers: &[RuntimeMcpServerEntry],
    warnings: &[String],
    status_for_server: impl Fn(&RuntimeMcpServerEntry) -> UiRuntimeMcpServerStatus,
) -> UiRuntimeMcpInventorySummary {
    let unique_server_names = servers
        .iter()
        .map(|entry| entry.server.name.as_str())
        .collect::<BTreeSet<_>>();
    let ambiguous_server_names = unique_server_names
        .iter()
        .filter(|name| {
            servers
                .iter()
                .filter(|entry| entry.server.name == **name)
                .nth(1)
                .is_some()
        })
        .count();
    let mut origins = UiRuntimeMcpOriginCounts::default();
    let mut status_counts = UiRuntimeMcpStatusCounts::default();

    for entry in servers {
        match entry.origin_kind {
            "cwd" => origins.cwd += 1,
            "profile" => origins.profile += 1,
            "explicit" => origins.explicit += 1,
            "plugin" => origins.plugin += 1,
            _ => {}
        }
        accumulate_runtime_mcp_status_count(&mut status_counts, status_for_server(entry));
    }

    UiRuntimeMcpInventorySummary {
        total_servers: servers.len(),
        enabled_servers: servers.iter().filter(|entry| entry.server.enabled).count(),
        disabled_servers: servers.iter().filter(|entry| !entry.server.enabled).count(),
        unique_server_names: unique_server_names.len(),
        ambiguous_server_names,
        warning_count: warnings.len(),
        origins,
        status_counts,
    }
}

fn load_runtime_mcp_file(
    servers: &mut Vec<RuntimeMcpServerEntry>,
    warnings: &mut Vec<String>,
    loaded_paths: &mut BTreeSet<PathBuf>,
    origin_kind: &'static str,
    origin_name: &str,
    path: &Path,
) {
    if !path.exists() {
        if origin_kind == "explicit" {
            warnings.push(format!(
                "Explicit MCP config {} was not found",
                path.display()
            ));
        }
        return;
    }
    if !loaded_paths.insert(path.to_path_buf()) {
        return;
    }
    match claude_mcp::McpConfig::load(path) {
        Ok(config) => append_runtime_mcp_servers(servers, origin_kind, origin_name, path, config),
        Err(error) => warnings.push(format!(
            "Failed to load MCP config {}: {error}",
            path.display()
        )),
    }
}

fn append_runtime_mcp_servers(
    servers: &mut Vec<RuntimeMcpServerEntry>,
    origin_kind: &'static str,
    origin_name: &str,
    config_path: &Path,
    config: claude_mcp::McpConfig,
) {
    for server in config.servers.into_values() {
        servers.push(RuntimeMcpServerEntry {
            origin_kind,
            origin_name: origin_name.to_string(),
            config_path: config_path.to_path_buf(),
            server,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        discover_runtime_mcp_resolution, observe_runtime_mcp_servers, runtime_mcp_inventory_summary,
    };
    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use claude_mcp::McpClientInfo;
    use claude_ui_bridge::UiRuntimeMcpServerStatus;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn runtime_mcp_inventory_summary_resolves_higher_precedence_duplicates() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plugin_root = profile.join("plugins").join("sample");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"sample","version":"0.1.0","mcp":"./mcp.toml"}"#,
        )
        .expect("plugin manifest");

        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            concat!(
                "[mcp_servers.shared]\ncommand = \"python\"\n",
                "[mcp_servers.disabled]\ncommand = \"python\"\nenabled = false\n"
            ),
        )
        .expect("cwd mcp");
        fs::write(
            profile.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.shared]\ncommand = \"python\"\n",
        )
        .expect("profile mcp");
        fs::write(
            plugin_root.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.plugin]\ncommand = \"python\"\n",
        )
        .expect("plugin mcp");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");

        let summary = runtime_mcp_inventory_summary(&config, &[]);
        assert_eq!(summary.total_servers, 3);
        assert_eq!(summary.enabled_servers, 2);
        assert_eq!(summary.disabled_servers, 1);
        assert_eq!(summary.unique_server_names, 3);
        assert_eq!(summary.ambiguous_server_names, 0);
        assert_eq!(summary.origins.cwd, 2);
        assert_eq!(summary.origins.profile, 0);
        assert_eq!(summary.origins.plugin, 1);
        assert_eq!(summary.status_counts.pending, 2);
        assert_eq!(summary.status_counts.disabled, 1);
    }

    #[test]
    fn runtime_mcp_resolution_tracks_shadowed_entries_for_same_server_name() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            r#"[mcp_servers.shared]
command = "python"
args = ["managed.py"]"#,
        )
        .expect("write managed mcp");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_PROJECT_MCP_CONFIG_FILE),
            r#"{
  "mcpServers": {
    "shared": {
      "command": "python",
      "args": ["project.py"]
    }
  }
}"#,
        )
        .expect("write project mcp");

        let config = load_runtime_config(
            Some(cwd.clone()),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");

        let discovery = discover_runtime_mcp_resolution(&config, &[]);
        let resolved = discovery
            .servers
            .iter()
            .find(|entry| entry.entry.server.name == "shared")
            .expect("shared resolution");

        assert_eq!(
            resolved.entry.config_path,
            cwd.join(claude_mcp::DEFAULT_PROJECT_MCP_CONFIG_FILE)
        );
        assert_eq!(resolved.shadowed_entries.len(), 1);
        assert_eq!(
            resolved.shadowed_entries[0].config_path,
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE)
        );
    }

    #[test]
    fn runtime_mcp_inventory_summary_obeys_allowed_setting_sources() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plugin_root = profile.join("plugins").join("sample");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"sample","version":"0.1.0","mcp":"./mcp.toml"}"#,
        )
        .expect("plugin manifest");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.project]\ncommand = \"python\"\n",
        )
        .expect("cwd mcp");
        fs::write(
            profile.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.profile]\ncommand = \"python\"\n",
        )
        .expect("profile mcp");
        fs::write(
            plugin_root.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.plugin]\ncommand = \"python\"\n",
        )
        .expect("plugin mcp");

        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        config.allowed_setting_sources = vec![SettingSource::Project];

        let summary = runtime_mcp_inventory_summary(&config, &[]);
        assert_eq!(summary.total_servers, 1);
        assert_eq!(summary.enabled_servers, 1);
        assert_eq!(summary.disabled_servers, 0);
        assert_eq!(summary.origins.cwd, 1);
        assert_eq!(summary.origins.profile, 0);
        assert_eq!(summary.origins.plugin, 0);
        assert_eq!(summary.status_counts.pending, 1);
        assert_eq!(summary.status_counts.disabled, 0);
    }

    #[test]
    fn runtime_mcp_strict_config_uses_only_explicit_paths() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let explicit = tempdir.path().join("explicit.json");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.project]\ncommand = \"python\"\n",
        )
        .expect("cwd mcp");
        fs::write(
            profile.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.profile]\ncommand = \"python\"\n",
        )
        .expect("profile mcp");
        fs::write(
            &explicit,
            r#"{"mcpServers":{"explicit":{"command":"python"}}}"#,
        )
        .expect("explicit mcp");

        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        config.strict_mcp_config = true;

        let discovery =
            super::discover_runtime_mcp_servers(&config, std::slice::from_ref(&explicit));
        let names = discovery
            .servers
            .iter()
            .map(|entry| entry.server.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["explicit"]);
        assert_eq!(discovery.servers[0].origin_kind, "explicit");
    }

    #[tokio::test]
    async fn observe_runtime_mcp_servers_reports_failed_status_after_connect_attempt() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::write(
            cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            concat!(
                "[mcp_servers.pending]\ncommand = \"command-that-does-not-exist-remote-code\"\n",
                "[mcp_servers.disabled]\ncommand = \"command-that-does-not-exist-remote-code\"\nenabled = false\n"
            ),
        )
        .expect("cwd mcp");

        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");

        let pending_only = observe_runtime_mcp_servers(
            &config,
            &[],
            false,
            &McpClientInfo::new("remote-code-rust", "test"),
        )
        .await;
        let pending_summary = pending_only.inventory_summary();
        assert_eq!(pending_summary.status_counts.pending, 1);
        assert_eq!(pending_summary.status_counts.disabled, 1);
        assert_eq!(pending_summary.status_counts.failed, 0);

        let connected = observe_runtime_mcp_servers(
            &config,
            &[],
            true,
            &McpClientInfo::new("remote-code-rust", "test"),
        )
        .await;
        let connected_summary = connected.inventory_summary();
        assert_eq!(connected_summary.status_counts.connected, 0);
        assert_eq!(connected_summary.status_counts.failed, 1);
        assert_eq!(connected_summary.status_counts.disabled, 1);
        assert_eq!(connected_summary.status_counts.pending, 0);
        assert!(
            connected
                .servers
                .iter()
                .any(|server| server.status == UiRuntimeMcpServerStatus::Failed
                    && server.error.as_deref().is_some())
        );
    }
}
