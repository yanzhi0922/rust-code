use std::path::PathBuf;

use anyhow::Result;
use claude_config::{RuntimeConfig, SettingSource, runtime_version, validate_provider_config};
use claude_mcp::McpClientInfo;
use claude_permissions::{load_layered_rules, rules::summarize_rule_sources};
use claude_tools::mcp_runtime::observe_runtime_mcp_servers;
use claude_ui_bridge::{UiRuntimeMcpInventorySummary, UiRuntimeMcpServerStatus};
use serde::Serialize;

use super::install::{InstallSource, detect_install_source, release_repository_slug};
use super::network::{ProbeResult, ProbeSpec, run_probe};
use super::providers::{
    EnvProviderSummary, env_provider_summaries, provider_endpoint_url, provider_probe_spec,
};
use crate::cli::DoctorArgs;
use crate::conversation::discover_runtime_extensions;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuleSourceCount {
    pub source: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeSection {
    pub version: String,
    pub cwd: PathBuf,
    pub profile_dir: PathBuf,
    pub session_id: String,
    pub session_name: Option<String>,
    pub permission_mode: String,
    pub input_format: String,
    pub output_format: String,
    pub setting_sources: Vec<String>,
    pub allowed_setting_sources: Vec<String>,
    pub settings_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct InstallSection {
    pub source: String,
    pub update_supported: bool,
    pub executable: PathBuf,
    pub repository_url: String,
    pub repository_slug: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProviderSection {
    pub name: String,
    pub protocol: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key_present: bool,
    pub auth_source: Option<String>,
    pub effort: Option<String>,
    pub fallback_model: Option<String>,
    pub context_window_tokens: u64,
    pub output_reserve_tokens: u64,
    pub multimodal: bool,
    pub reasoning: bool,
    pub validation_ok: bool,
    pub validation_issues: Vec<String>,
    pub probe: Option<ProbeResult>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ToolsSection {
    pub builtin_tools: usize,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PermissionsSection {
    pub layered_rules: usize,
    pub rule_sources: Vec<RuleSourceCount>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExtensionsSection {
    pub skills: usize,
    pub plugins: usize,
    pub disabled_plugins: usize,
    pub plugin_runtimes: usize,
    pub mcp_servers: usize,
    pub disabled_mcp_servers: usize,
    pub hooks: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct McpRuntimeServerSection {
    pub name: String,
    pub status: UiRuntimeMcpServerStatus,
    pub enabled: bool,
    pub origin_kind: String,
    pub origin_name: String,
    pub config_path: PathBuf,
    pub tool_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct McpRuntimeSection {
    pub probed: bool,
    pub summary: UiRuntimeMcpInventorySummary,
    pub servers: Vec<McpRuntimeServerSection>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorReport {
    pub ok: bool,
    pub runtime: RuntimeSection,
    pub install: InstallSection,
    pub provider: ProviderSection,
    pub tools: ToolsSection,
    pub permissions: PermissionsSection,
    pub extensions: ExtensionsSection,
    pub mcp_runtime: McpRuntimeSection,
    pub network: Option<Vec<ProbeResult>>,
    pub env_providers: Option<Vec<EnvProviderSummary>>,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
}

pub(crate) async fn collect_report(
    config: &RuntimeConfig,
    args: &DoctorArgs,
) -> Result<DoctorReport> {
    let validation = validate_provider_config(&config.provider);
    let discovery = discover_runtime_extensions(config);
    let hooks = crate::runtime_hooks::HookRuntime::discover(config);
    let layered_rules = load_layered_rules(
        &config.cwd,
        &config.paths.profile_dir,
        &config.settings_files,
        &config.cli_settings_files,
    );
    let model_info = claude_provider::model_info::get_model_info(
        config.provider.model.as_deref().unwrap_or("unknown"),
    );
    let install_source = detect_install_source();
    let mcp_runtime = observe_runtime_mcp_servers(
        config,
        &[],
        args.probe_mcp,
        &McpClientInfo::new("remote-code-rust", runtime_version()),
    )
    .await;

    let mut issues = validation.issues.clone();
    let mut warnings = Vec::new();
    extend_unique_strings(&mut warnings, discovery.warnings.clone());
    extend_unique_strings(&mut warnings, hooks.warnings().to_vec());
    extend_unique_strings(&mut warnings, mcp_runtime.warnings.clone());
    let disabled_plugins = if config
        .allowed_setting_sources
        .contains(&SettingSource::User)
        && config.paths.plugins_dir.exists()
    {
        match claude_plugins::discover_plugins_including_disabled(&config.paths.plugins_dir) {
            Ok(plugins) => plugins.iter().filter(|plugin| plugin.is_disabled()).count(),
            Err(error) => {
                warnings.push(format!("Failed to inspect disabled plugins: {error}"));
                0
            }
        }
    } else {
        0
    };

    let provider_probe = if args.probe_provider {
        if let Some(spec) = provider_probe_spec(&config.provider) {
            let probe = run_probe(spec).await;
            if probe.is_issue() {
                issues.push(format!("Provider probe failed: {}", probe.detail));
            } else if probe.is_warning() {
                warnings.push(format!("Provider probe warning: {}", probe.detail));
            }
            Some(probe)
        } else {
            warnings.push(
                "Provider probe skipped: no probeable endpoint for the active protocol.".to_owned(),
            );
            None
        }
    } else {
        None
    };

    let network = if args.probe_network {
        let mut probes = Vec::new();
        if let Some(repository_slug) = release_repository_slug() {
            probes.push(
                ProbeSpec::new(
                    "github:releases",
                    format!("https://api.github.com/repos/{repository_slug}/releases/latest"),
                )
                .with_header("accept", "application/vnd.github+json"),
            );
        }
        if !args.probe_provider
            && let Some(provider_url) = provider_endpoint_url(&config.provider)
        {
            probes.push(ProbeSpec::new("provider:network", provider_url));
        }

        let mut results = Vec::new();
        for probe in probes {
            let result = run_probe(probe).await;
            if result.is_issue() || result.is_warning() {
                warnings.push(format!("Network probe warning: {}", result.detail));
            }
            results.push(result);
        }
        Some(results)
    } else {
        None
    };

    let runtime = RuntimeSection {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        cwd: config.cwd.clone(),
        profile_dir: config.paths.profile_dir.clone(),
        session_id: config.session_id.to_string(),
        session_name: config.session_name.clone(),
        permission_mode: format!("{:?}", config.permission_mode),
        input_format: format!("{:?}", config.input_format),
        output_format: format!("{:?}", config.output_format),
        setting_sources: config.setting_sources.clone(),
        allowed_setting_sources: config
            .allowed_setting_sources
            .iter()
            .map(|source| source.as_str().to_owned())
            .collect(),
        settings_files: config.settings_files.clone(),
    };
    let install = build_install_section(&install_source);
    let provider = ProviderSection {
        name: config.provider.name.clone(),
        protocol: config.provider.protocol.as_str().to_owned(),
        base_url: config.provider.base_url.clone(),
        model: config.provider.model.clone(),
        api_key_present: config.provider.api_key.is_some(),
        auth_source: config.auth_source.clone(),
        effort: config.effort.clone(),
        fallback_model: config.fallback_model.clone(),
        context_window_tokens: model_info.max_context,
        output_reserve_tokens: model_info.max_output,
        multimodal: model_info.multimodal,
        reasoning: model_info
            .capabilities
            .contains(&claude_provider::model_info::ModelCapability::Reasoning),
        validation_ok: validation.ok,
        validation_issues: validation.issues,
        probe: provider_probe,
    };
    let tools = ToolsSection {
        builtin_tools: claude_tools::runtime_builtin_tool_specs().len(),
        allowed_tools: config.allowed_tools.clone(),
        disallowed_tools: config.disallowed_tools.clone(),
    };
    let permissions = PermissionsSection {
        layered_rules: layered_rules.len(),
        rule_sources: summarize_rule_sources(&layered_rules)
            .into_iter()
            .map(|(source, count)| RuleSourceCount {
                source: source.as_str().to_owned(),
                count,
            })
            .collect(),
    };
    let extensions = ExtensionsSection {
        skills: discovery.skills.len(),
        plugins: discovery.plugins.len(),
        disabled_plugins,
        plugin_runtimes: discovery.plugin_runtimes.len(),
        mcp_servers: discovery.mcp_servers.len(),
        disabled_mcp_servers: discovery.disabled_mcp_servers.len(),
        hooks: hooks.list(None).len(),
    };
    let mcp_runtime = McpRuntimeSection {
        probed: args.probe_mcp,
        summary: mcp_runtime.inventory_summary(),
        servers: mcp_runtime
            .servers
            .into_iter()
            .map(|server| McpRuntimeServerSection {
                name: server.entry.server.name,
                status: server.status,
                enabled: server.entry.server.enabled,
                origin_kind: server.entry.origin_kind.to_owned(),
                origin_name: server.entry.origin_name,
                config_path: server.entry.config_path,
                tool_count: server
                    .inspection
                    .as_ref()
                    .map_or(0, |inspection| inspection.tools.len()),
                error: server.error,
            })
            .collect(),
    };
    let env_providers = args.include_env_providers.then(env_provider_summaries);

    Ok(DoctorReport {
        ok: issues.is_empty(),
        runtime,
        install,
        provider,
        tools,
        permissions,
        extensions,
        mcp_runtime,
        network,
        env_providers,
        issues,
        warnings,
    })
}

fn extend_unique_strings(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

fn build_install_section(install_source: &InstallSource) -> InstallSection {
    InstallSection {
        source: install_source.label().to_owned(),
        update_supported: install_source.supports_in_place_update(),
        executable: install_source.executable.clone(),
        repository_url: install_source.repository_url.clone(),
        repository_slug: install_source.repository_slug.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::tempdir;

    use super::collect_report;
    use crate::cli::DoctorArgs;

    fn test_config() -> (tempfile::TempDir, claude_config::RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        (
            tempdir,
            load_runtime_config(
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
            .expect("config"),
        )
    }

    #[tokio::test]
    async fn doctor_report_includes_runtime_mcp_summary_without_probe() {
        let (_tempdir, config) = test_config();
        fs::write(
            config.cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            concat!(
                "[mcp_servers.pending]\ncommand = \"python\"\n",
                "[mcp_servers.disabled]\ncommand = \"python\"\nenabled = false\n"
            ),
        )
        .expect("write mcp config");

        let report = collect_report(&config, &DoctorArgs::default())
            .await
            .expect("doctor report");
        assert!(!report.mcp_runtime.probed);
        assert_eq!(report.mcp_runtime.summary.total_servers, 2);
        assert_eq!(report.mcp_runtime.summary.status_counts.pending, 1);
        assert_eq!(report.mcp_runtime.summary.status_counts.disabled, 1);
        assert_eq!(report.mcp_runtime.summary.status_counts.failed, 0);
    }

    #[tokio::test]
    async fn doctor_report_probe_mcp_marks_failed_servers() {
        let (_tempdir, config) = test_config();
        fs::write(
            config.cwd.join(claude_mcp::DEFAULT_MCP_CONFIG_FILE),
            "[mcp_servers.failing]\ncommand = \"command-that-does-not-exist-remote-code\"\n",
        )
        .expect("write mcp config");

        let report = collect_report(
            &config,
            &DoctorArgs {
                probe_mcp: true,
                ..DoctorArgs::default()
            },
        )
        .await
        .expect("doctor report");
        assert!(report.mcp_runtime.probed);
        assert_eq!(report.mcp_runtime.summary.status_counts.failed, 1);
        assert_eq!(report.mcp_runtime.summary.status_counts.pending, 0);
        assert!(report.mcp_runtime.servers.iter().any(|server| server.status
            == claude_ui_bridge::UiRuntimeMcpServerStatus::Failed
            && server.error.as_deref().is_some()));
    }
}
