use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use claude_config::{RuntimeConfig, SettingSource, runtime_version};

use crate::cli::{
    PluginsCommand, PluginsInspectArgs, PluginsInstallArgs, PluginsInvokeArgs, PluginsListArgs,
    PluginsRemoveArgs, PluginsToggleArgs, PluginsUpdateArgs, PluginsValidateArgs,
};
use crate::mcp_cli::parse_named_json_object_args;

pub(crate) async fn run_plugins(config: &RuntimeConfig, command: PluginsCommand) -> Result<()> {
    match command {
        PluginsCommand::List(args) => run_plugins_list(config, args).await,
        PluginsCommand::Inspect(args) => run_plugins_inspect(config, args).await,
        PluginsCommand::Invoke(args) => run_plugins_invoke(config, args).await,
        PluginsCommand::Validate(args) => run_plugins_validate(config, args).await,
        PluginsCommand::Install(args) => run_plugins_install(config, args),
        PluginsCommand::Remove(args) => run_plugins_remove(config, args),
        PluginsCommand::Enable(args) => run_plugins_toggle(config, args, true),
        PluginsCommand::Disable(args) => run_plugins_toggle(config, args, false),
        PluginsCommand::Update(args) => run_plugins_update(config, args),
    }
}

async fn run_plugins_list(config: &RuntimeConfig, args: PluginsListArgs) -> Result<()> {
    let output = build_plugins_list_output(config, &args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if output.plugins.is_empty() {
        println!("No plugins found.");
        for warning in output.warnings {
            println!("  - {warning}");
        }
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    for plugin in &output.plugins {
        println!(
            "{}  {}  runtime={}  skills={}  mcp={}  disabled={}  {}",
            plugin.name,
            plugin.version,
            if plugin.has_runtime { "yes" } else { "no" },
            if plugin.has_skills { "yes" } else { "no" },
            if plugin.has_mcp { "yes" } else { "no" },
            if plugin.disabled { "yes" } else { "no" },
            format_plugin_source(plugin)
        );
        if let Some(live) = &plugin.live {
            match live.status.as_str() {
                "ok" => {
                    let peer = live.plugin_info.as_ref().map_or_else(
                        || "unknown-plugin".to_owned(),
                        |info| match &info.version {
                            Some(version) => format!("{} {}", info.name, version),
                            None => info.name.clone(),
                        },
                    );
                    println!(
                        "  connect: ok  protocol={}  actions={}  peer={peer}",
                        live.protocol_version.as_deref().unwrap_or("unknown"),
                        live.action_count
                    );
                    for action in &live.actions {
                        match &action.description {
                            Some(description) => println!("    - {}: {description}", action.name),
                            None => println!("    - {}", action.name),
                        }
                    }
                }
                "skipped" => {
                    println!(
                        "  connect: skipped  {}",
                        live.error.as_deref().unwrap_or("inspection not attempted")
                    );
                }
                _ => {
                    println!(
                        "  connect: error  {}",
                        live.error
                            .as_deref()
                            .unwrap_or("inspection failed without details")
                    );
                }
            }
        }
    }
    Ok(())
}

async fn run_plugins_inspect(config: &RuntimeConfig, args: PluginsInspectArgs) -> Result<()> {
    let output = build_plugins_inspect_output(config, &args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    println!(
        "plugin: {} {}  {}",
        output.plugin.name,
        output.plugin.version,
        format_plugin_source(&output.plugin)
    );
    println!(
        "features: runtime={}  skills={}  mcp={}  disabled={}",
        if output.plugin.has_runtime {
            "yes"
        } else {
            "no"
        },
        if output.plugin.has_skills {
            "yes"
        } else {
            "no"
        },
        if output.plugin.has_mcp { "yes" } else { "no" },
        if output.plugin.disabled { "yes" } else { "no" }
    );
    match &output.plugin.live {
        Some(live) if live.status == "ok" => {
            println!(
                "runtime: ok  protocol={}  actions={}",
                live.protocol_version.as_deref().unwrap_or("unknown"),
                live.action_count
            );
            if let Some(info) = &live.plugin_info {
                match &info.version {
                    Some(version) => println!("peer: {} {}", info.name, version),
                    None => println!("peer: {}", info.name),
                }
            }
            for action in &live.actions {
                match &action.description {
                    Some(description) => println!("  - {}: {description}", action.name),
                    None => println!("  - {}", action.name),
                }
            }
        }
        Some(live) => {
            println!(
                "runtime: {}  {}",
                live.status,
                live.error.as_deref().unwrap_or("inspection failed")
            );
        }
        None => {
            println!("runtime: not inspected");
        }
    }
    Ok(())
}

async fn run_plugins_invoke(config: &RuntimeConfig, args: PluginsInvokeArgs) -> Result<()> {
    let output = build_plugins_invoke_output(config, &args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    println!(
        "plugin: {} {}  {}",
        output.plugin.name,
        output.plugin.version,
        format_plugin_source(&output.plugin)
    );
    println!("action: {}", output.response.action);
    println!(
        "status: {}",
        if output.response.result.is_error {
            "error"
        } else {
            "ok"
        }
    );
    println!("protocol: {}", output.response.protocol_version);
    if let Some(info) = &output.response.plugin_info {
        match &info.version {
            Some(version) => println!("peer: {} {}", info.name, version),
            None => println!("peer: {}", info.name),
        }
    }
    println!("output:");
    println!(
        "{}",
        serde_json::to_string_pretty(&output.response.result.output)?
    );
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginValidateOutput {
    warnings: Vec<String>,
    reports: Vec<claude_plugins::PluginValidationReport>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginInstallOutput {
    status: String,
    plugin: String,
    destination: PathBuf,
    validation: claude_plugins::PluginValidationReport,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginToggleOutput {
    status: String,
    plugin: String,
    enabled: bool,
    destination: PathBuf,
    marker_path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginUpdateOutput {
    status: String,
    plugin: String,
    destination: PathBuf,
    preserved_disabled: bool,
    validation: claude_plugins::PluginValidationReport,
}

async fn run_plugins_validate(config: &RuntimeConfig, args: PluginsValidateArgs) -> Result<()> {
    let output = build_plugins_validate_output(config, &args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    if output.reports.is_empty() {
        println!("No plugins matched the validation request.");
        return Ok(());
    }
    for report in &output.reports {
        println!(
            "{}  errors={} warnings={} skills={} runtime={} mcp={}",
            report.plugin_name,
            report.errors.len(),
            report.warnings.len(),
            report.bundled_skills,
            report.has_runtime,
            report.has_mcp
        );
        for error in &report.errors {
            println!("  error: {error}");
        }
        for warning in &report.warnings {
            println!("  warning: {warning}");
        }
    }
    Ok(())
}

fn run_plugins_install(config: &RuntimeConfig, args: PluginsInstallArgs) -> Result<()> {
    let plugin = claude_plugins::load_plugin_from_root(&args.path)?;
    let validation = claude_plugins::validate_plugin_bundle(&plugin);
    if !validation.errors.is_empty() {
        return Err(anyhow!(
            "Plugin validation failed: {}",
            validation.errors.join("; ")
        ));
    }

    let destination = config.paths.plugins_dir.join(&plugin.manifest.name);
    if destination.exists() {
        if !args.force {
            return Err(anyhow!(
                "Plugin destination {} already exists; pass --force to replace it",
                destination.display()
            ));
        }
        if !destination.starts_with(&config.paths.plugins_dir) {
            return Err(anyhow!(
                "Refusing to replace plugin outside {}",
                config.paths.plugins_dir.display()
            ));
        }
        fs::remove_dir_all(&destination)?;
    }

    copy_dir_recursive(&plugin.root, &destination)?;
    let output = PluginInstallOutput {
        status: if args.force {
            "reinstalled"
        } else {
            "installed"
        }
        .to_owned(),
        plugin: plugin.manifest.name,
        destination,
        validation,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Plugin {} at {}.",
            output.status,
            output.destination.display()
        );
    }
    Ok(())
}

fn run_plugins_remove(config: &RuntimeConfig, args: PluginsRemoveArgs) -> Result<()> {
    let destination = config.paths.plugins_dir.join(&args.plugin);
    if !destination.exists() {
        if args.if_exists {
            println!("Plugin {} already absent.", args.plugin);
            return Ok(());
        }
        return Err(anyhow!(
            "Plugin {} is not installed in {}",
            args.plugin,
            config.paths.plugins_dir.display()
        ));
    }
    if !destination.starts_with(&config.paths.plugins_dir) {
        return Err(anyhow!(
            "Refusing to remove plugin outside {}",
            config.paths.plugins_dir.display()
        ));
    }
    fs::remove_dir_all(&destination)?;
    let output = serde_json::json!({
        "status": "removed",
        "plugin": args.plugin,
        "destination": destination,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Plugin removed from {}.",
            output["destination"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

fn run_plugins_toggle(
    config: &RuntimeConfig,
    args: PluginsToggleArgs,
    enabled: bool,
) -> Result<()> {
    let destination = config.paths.plugins_dir.join(&args.plugin);
    if !destination.exists() {
        if args.if_exists {
            let output = PluginToggleOutput {
                status: "noop".to_owned(),
                plugin: args.plugin,
                enabled,
                marker_path: destination.join(claude_plugins::PLUGIN_DISABLED_MARKER),
                destination,
            };
            if args.json {
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Plugin {} already absent.", output.plugin);
            }
            return Ok(());
        }
        return Err(anyhow!(
            "Plugin {} is not installed in {}",
            args.plugin,
            config.paths.plugins_dir.display()
        ));
    }
    if !destination.starts_with(&config.paths.plugins_dir) {
        return Err(anyhow!(
            "Refusing to mutate plugin outside {}",
            config.paths.plugins_dir.display()
        ));
    }

    let marker_path = destination.join(claude_plugins::PLUGIN_DISABLED_MARKER);
    let status = if enabled {
        if marker_path.exists() {
            fs::remove_file(&marker_path)?;
            "enabled"
        } else {
            "noop"
        }
    } else if marker_path.exists() {
        "noop"
    } else {
        fs::write(&marker_path, b"disabled\n")?;
        "disabled"
    };

    let output = PluginToggleOutput {
        status: status.to_owned(),
        plugin: args.plugin,
        enabled,
        destination,
        marker_path,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if output.status == "noop" {
        println!(
            "Plugin {} already {}.",
            output.plugin,
            if enabled { "enabled" } else { "disabled" }
        );
    } else {
        println!(
            "Plugin {} {} at {}.",
            output.plugin,
            output.status,
            output.destination.display()
        );
    }
    Ok(())
}

fn run_plugins_update(config: &RuntimeConfig, args: PluginsUpdateArgs) -> Result<()> {
    let plugin = claude_plugins::load_plugin_from_root(&args.path)?;
    let validation = claude_plugins::validate_plugin_bundle(&plugin);
    if !validation.errors.is_empty() {
        return Err(anyhow!(
            "Plugin validation failed: {}",
            validation.errors.join("; ")
        ));
    }

    let destination = config.paths.plugins_dir.join(&plugin.manifest.name);
    if !destination.exists() {
        return Err(anyhow!(
            "Plugin {} is not installed in {}; use `plugins install` first",
            plugin.manifest.name,
            config.paths.plugins_dir.display()
        ));
    }
    if !destination.starts_with(&config.paths.plugins_dir) {
        return Err(anyhow!(
            "Refusing to update plugin outside {}",
            config.paths.plugins_dir.display()
        ));
    }

    let marker_path = destination.join(claude_plugins::PLUGIN_DISABLED_MARKER);
    let preserved_disabled = marker_path.exists();
    fs::remove_dir_all(&destination)?;
    copy_dir_recursive(&plugin.root, &destination)?;
    if preserved_disabled {
        fs::write(&marker_path, b"disabled\n")?;
    } else if marker_path.exists() {
        fs::remove_file(&marker_path)?;
    }

    let output = PluginUpdateOutput {
        status: "updated".to_owned(),
        plugin: plugin.manifest.name,
        destination,
        preserved_disabled,
        validation,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Plugin updated at {}{}.",
            output.destination.display(),
            if output.preserved_disabled {
                " (disabled state preserved)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimePluginEntry {
    pub(crate) origin_kind: &'static str,
    pub(crate) origin_name: String,
    pub(crate) bundle: claude_plugins::PluginBundle,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimePluginDiscovery {
    pub(crate) plugins: Vec<RuntimePluginEntry>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct RuntimePluginResolution {
    entry: RuntimePluginEntry,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginsListOutput {
    warnings: Vec<String>,
    plugins: Vec<PluginRecord>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PluginRecord {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) has_runtime: bool,
    pub(crate) has_skills: bool,
    pub(crate) has_mcp: bool,
    pub(crate) disabled: bool,
    pub(crate) origin_kind: String,
    pub(crate) origin_name: String,
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) live: Option<PluginLiveRecord>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PluginLiveRecord {
    status: String,
    protocol_version: Option<String>,
    plugin_info: Option<claude_plugins::PluginPeerInfo>,
    action_count: usize,
    actions: Vec<claude_plugins::PluginRuntimeActionDescriptor>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginInspectOutput {
    warnings: Vec<String>,
    plugin: PluginRecord,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PluginInvokeOutput {
    warnings: Vec<String>,
    plugin: PluginRecord,
    input: serde_json::Value,
    response: claude_plugins::PluginInvokeResponse,
}

impl PluginLiveRecord {
    fn from_inspection(inspection: claude_plugins::PluginRuntimeInspection) -> Self {
        Self {
            status: "ok".to_owned(),
            protocol_version: Some(inspection.protocol_version),
            plugin_info: inspection.plugin_info,
            action_count: inspection.actions.len(),
            actions: inspection.actions,
            error: None,
        }
    }

    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            status: "skipped".to_owned(),
            protocol_version: None,
            plugin_info: None,
            action_count: 0,
            actions: Vec::new(),
            error: Some(reason.into()),
        }
    }

    fn failed(error: &impl ToString) -> Self {
        Self {
            status: "error".to_owned(),
            protocol_version: None,
            plugin_info: None,
            action_count: 0,
            actions: Vec::new(),
            error: Some(error.to_string()),
        }
    }
}

async fn build_plugins_list_output(
    config: &RuntimeConfig,
    args: &PluginsListArgs,
) -> Result<PluginsListOutput> {
    let discovery = discover_runtime_plugins(config, &args.plugin_roots);
    let filters = args.plugins.iter().cloned().collect::<BTreeSet<_>>();
    let mut plugins = Vec::new();

    for entry in discovery.plugins {
        if !filters.is_empty() && !filters.contains(&entry.bundle.manifest.name) {
            continue;
        }
        let has_runtime = entry.bundle.runtime_config().is_some();
        let live = if args.connect {
            if entry.bundle.is_disabled() {
                Some(PluginLiveRecord::skipped(
                    "plugin is currently disabled by marker file",
                ))
            } else if has_runtime {
                Some(
                    match claude_plugins::inspect_runtime(
                        &entry.bundle,
                        &claude_plugins::PluginHostInfo::new("remote-code-rust", runtime_version()),
                    )
                    .await
                    {
                        Ok(inspection) => PluginLiveRecord::from_inspection(inspection),
                        Err(error) => PluginLiveRecord::failed(&error),
                    },
                )
            } else {
                Some(PluginLiveRecord::skipped(
                    "plugin does not define a runtime adapter",
                ))
            }
        } else {
            None
        };
        plugins.push(plugin_record_from_entry(&entry, has_runtime, live));
    }

    if !filters.is_empty() && plugins.is_empty() {
        return Err(anyhow!(
            "No matching plugins found for: {}",
            filters.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    Ok(PluginsListOutput {
        warnings: discovery.warnings,
        plugins,
    })
}

async fn build_plugins_inspect_output(
    config: &RuntimeConfig,
    args: &PluginsInspectArgs,
) -> Result<PluginInspectOutput> {
    let resolution = resolve_runtime_plugin(config, &args.plugin, &args.plugin_roots)?;
    let has_runtime = resolution.entry.bundle.runtime_config().is_some();
    let live = if resolution.entry.bundle.is_disabled() {
        Some(PluginLiveRecord::skipped(
            "plugin is currently disabled by marker file",
        ))
    } else if has_runtime {
        Some(
            match claude_plugins::inspect_runtime(
                &resolution.entry.bundle,
                &claude_plugins::PluginHostInfo::new("remote-code-rust", runtime_version()),
            )
            .await
            {
                Ok(inspection) => PluginLiveRecord::from_inspection(inspection),
                Err(error) => PluginLiveRecord::failed(&error),
            },
        )
    } else {
        Some(PluginLiveRecord::skipped(
            "plugin does not define a runtime adapter",
        ))
    };

    Ok(PluginInspectOutput {
        warnings: resolution.warnings,
        plugin: plugin_record_from_entry(&resolution.entry, has_runtime, live),
    })
}

async fn build_plugins_invoke_output(
    config: &RuntimeConfig,
    args: &PluginsInvokeArgs,
) -> Result<PluginInvokeOutput> {
    let resolution = resolve_runtime_plugin(config, &args.plugin, &args.plugin_roots)?;
    if resolution.entry.bundle.is_disabled() {
        return Err(anyhow!("Plugin `{}` is currently disabled", args.plugin));
    }
    let has_runtime = resolution.entry.bundle.runtime_config().is_some();
    if !has_runtime {
        return Err(anyhow!(
            "Plugin `{}` does not define a runtime adapter",
            args.plugin
        ));
    }
    let input = parse_plugin_invoke_input(args)?;
    let response = claude_plugins::invoke_runtime(
        &resolution.entry.bundle,
        &claude_plugins::PluginHostInfo::new("remote-code-rust", runtime_version()),
        &args.action,
        input.clone(),
    )
    .await?;

    Ok(PluginInvokeOutput {
        warnings: resolution.warnings,
        plugin: plugin_record_from_entry(&resolution.entry, true, None),
        input,
        response,
    })
}

fn parse_plugin_invoke_input(args: &PluginsInvokeArgs) -> Result<serde_json::Value> {
    parse_named_json_object_args("--input-json", args.input_json.as_ref(), &args.args)
}

pub(crate) fn discover_runtime_plugins(
    config: &RuntimeConfig,
    extra_plugin_roots: &[PathBuf],
) -> RuntimePluginDiscovery {
    let mut discovery = RuntimePluginDiscovery::default();
    let mut seen_manifest_paths = BTreeSet::new();
    if setting_source_enabled(config, SettingSource::User) {
        load_runtime_plugins_root(
            &mut discovery,
            &mut seen_manifest_paths,
            "profile",
            &config.paths.plugins_dir.display().to_string(),
            &config.paths.plugins_dir,
        );
    }
    for root in extra_plugin_roots {
        load_runtime_plugins_root(
            &mut discovery,
            &mut seen_manifest_paths,
            "explicit",
            &root.display().to_string(),
            root,
        );
    }

    discovery.plugins.sort_by(|left, right| {
        left.bundle
            .manifest
            .name
            .cmp(&right.bundle.manifest.name)
            .then_with(|| left.origin_kind.cmp(right.origin_kind))
            .then_with(|| left.origin_name.cmp(&right.origin_name))
    });
    discovery
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn load_runtime_plugins_root(
    discovery: &mut RuntimePluginDiscovery,
    seen_manifest_paths: &mut BTreeSet<PathBuf>,
    origin_kind: &'static str,
    origin_name: &str,
    root: &Path,
) {
    if !root.exists() {
        if origin_kind == "explicit" {
            discovery.warnings.push(format!(
                "Explicit plugin root {} was not found",
                root.display()
            ));
        }
        return;
    }
    match claude_plugins::discover_plugins_including_disabled(root) {
        Ok(plugins) => {
            for plugin in plugins {
                if !seen_manifest_paths.insert(plugin.manifest_path.clone()) {
                    continue;
                }
                discovery.plugins.push(RuntimePluginEntry {
                    origin_kind,
                    origin_name: origin_name.to_string(),
                    bundle: plugin,
                });
            }
        }
        Err(error) => discovery.warnings.push(format!(
            "Failed to discover plugins in {}: {error}",
            root.display()
        )),
    }
}

fn resolve_runtime_plugin(
    config: &RuntimeConfig,
    plugin_name: &str,
    extra_plugin_roots: &[PathBuf],
) -> Result<RuntimePluginResolution> {
    let mut discovery = discover_runtime_plugins(config, extra_plugin_roots);
    let mut matches = discovery
        .plugins
        .iter()
        .filter(|entry| entry.bundle.manifest.name == plugin_name)
        .cloned()
        .collect::<Vec<_>>();

    match matches.len() {
        0 => Err(anyhow!("No plugin named `{plugin_name}` was found")),
        1 => Ok(RuntimePluginResolution {
            entry: matches.pop().expect("single plugin match must exist"),
            warnings: discovery.warnings,
        }),
        _ => {
            let candidates = matches
                .into_iter()
                .map(|entry| {
                    format!(
                        "{}:{} ({})",
                        entry.origin_kind,
                        entry.origin_name,
                        entry.bundle.manifest_path.display()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            discovery.warnings.push(format!(
                "Multiple plugins named `{plugin_name}` were discovered; use a unique plugin layout"
            ));
            Err(anyhow!(
                "Plugin `{plugin_name}` is ambiguous across: {candidates}"
            ))
        }
    }
}

fn plugin_record_from_entry(
    entry: &RuntimePluginEntry,
    has_runtime: bool,
    live: Option<PluginLiveRecord>,
) -> PluginRecord {
    PluginRecord {
        name: entry.bundle.manifest.name.clone(),
        version: entry.bundle.manifest.version.clone(),
        has_runtime,
        has_skills: entry.bundle.skills_root().is_some(),
        has_mcp: entry.bundle.mcp_config_path().is_some(),
        disabled: entry.bundle.is_disabled(),
        origin_kind: entry.origin_kind.to_owned(),
        origin_name: entry.origin_name.clone(),
        root: entry.bundle.root.clone(),
        manifest_path: entry.bundle.manifest_path.clone(),
        live,
    }
}

pub(crate) fn format_plugin_source(plugin: &PluginRecord) -> String {
    match plugin.origin_kind.as_str() {
        "explicit" => format!(
            "explicit:{} ({})",
            plugin.origin_name,
            plugin.manifest_path.display()
        ),
        _ => format!(
            "{} ({})",
            plugin.origin_kind,
            plugin.manifest_path.display()
        ),
    }
}

fn build_plugins_validate_output(
    config: &RuntimeConfig,
    args: &PluginsValidateArgs,
) -> Result<PluginValidateOutput> {
    let mut warnings = Vec::new();
    let mut reports = Vec::new();

    if let Some(path) = &args.path {
        let plugin = claude_plugins::load_plugin_from_root(path)?;
        reports.push(claude_plugins::validate_plugin_bundle(&plugin));
        return Ok(PluginValidateOutput { warnings, reports });
    }

    if let Some(plugin_name) = &args.plugin {
        let resolution = resolve_runtime_plugin(config, plugin_name, &args.plugin_roots)?;
        warnings.extend(resolution.warnings);
        reports.push(claude_plugins::validate_plugin_bundle(
            &resolution.entry.bundle,
        ));
        return Ok(PluginValidateOutput { warnings, reports });
    }

    let discovery = discover_runtime_plugins(config, &args.plugin_roots);
    warnings.extend(discovery.warnings.clone());
    reports.extend(
        discovery
            .plugins
            .iter()
            .map(|entry| claude_plugins::validate_plugin_bundle(&entry.bundle)),
    );
    Ok(PluginValidateOutput { warnings, reports })
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use tempfile::tempdir;

    use super::{
        build_plugins_inspect_output, build_plugins_invoke_output, build_plugins_list_output,
        discover_runtime_plugins, resolve_runtime_plugin, run_plugins_toggle, run_plugins_update,
    };
    use crate::cli::{
        PluginsInspectArgs, PluginsInvokeArgs, PluginsListArgs, PluginsToggleArgs,
        PluginsUpdateArgs,
    };

    fn test_config() -> (tempfile::TempDir, claude_config::RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(profile.join("plugins")).expect("plugins");
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        (tempdir, config)
    }

    fn write_plugin(root: &Path, version: &str, extra_files: &[(&str, &str)]) {
        fs::create_dir_all(root.join(claude_plugins::PLUGIN_MANIFEST_DIR)).expect("manifest dir");
        fs::write(
            root.join(claude_plugins::PLUGIN_MANIFEST_DIR)
                .join(claude_plugins::PLUGIN_MANIFEST_FILE),
            format!(r#"{{"name":"demo","version":"{version}"}}"#),
        )
        .expect("manifest");
        for (relative, content) in extra_files {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent");
            }
            fs::write(path, content).expect("file");
        }
    }

    fn write_plugin_manifest(root: &Path, manifest: &str, extra_files: &[(&str, &str)]) {
        fs::create_dir_all(root.join(claude_plugins::PLUGIN_MANIFEST_DIR)).expect("manifest dir");
        fs::write(
            root.join(claude_plugins::PLUGIN_MANIFEST_DIR)
                .join(claude_plugins::PLUGIN_MANIFEST_FILE),
            manifest,
        )
        .expect("manifest");
        for (relative, content) in extra_files {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent");
            }
            fs::write(path, content).expect("file");
        }
    }

    #[test]
    fn plugin_toggle_writes_and_clears_disabled_marker() {
        let (_tempdir, config) = test_config();
        let plugin_root = config.paths.plugins_dir.join("demo");
        write_plugin(&plugin_root, "0.1.0", &[]);

        run_plugins_toggle(
            &config,
            PluginsToggleArgs {
                plugin: "demo".to_owned(),
                json: false,
                if_exists: false,
            },
            false,
        )
        .expect("disable plugin");
        assert!(
            plugin_root
                .join(claude_plugins::PLUGIN_DISABLED_MARKER)
                .exists()
        );

        run_plugins_toggle(
            &config,
            PluginsToggleArgs {
                plugin: "demo".to_owned(),
                json: false,
                if_exists: false,
            },
            true,
        )
        .expect("enable plugin");
        assert!(
            !plugin_root
                .join(claude_plugins::PLUGIN_DISABLED_MARKER)
                .exists()
        );
    }

    #[test]
    fn plugin_update_preserves_disabled_state() {
        let (_tempdir, config) = test_config();
        let installed_root = config.paths.plugins_dir.join("demo");
        write_plugin(&installed_root, "0.1.0", &[("data.txt", "old")]);
        fs::write(
            installed_root.join(claude_plugins::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("disabled marker");

        let source_root = config.cwd.join("source-demo");
        write_plugin(&source_root, "0.2.0", &[("data.txt", "new")]);

        run_plugins_update(
            &config,
            PluginsUpdateArgs {
                path: source_root,
                json: false,
            },
        )
        .expect("update plugin");

        assert_eq!(
            fs::read_to_string(installed_root.join("data.txt")).expect("updated data"),
            "new"
        );
        assert!(
            installed_root
                .join(claude_plugins::PLUGIN_DISABLED_MARKER)
                .exists()
        );
    }

    #[tokio::test]
    async fn plugin_list_output_reports_disabled_state() {
        let (_tempdir, config) = test_config();
        let plugin_root = config.paths.plugins_dir.join("demo");
        write_plugin(&plugin_root, "0.1.0", &[]);
        fs::write(
            plugin_root.join(claude_plugins::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("disabled marker");

        let output = build_plugins_list_output(
            &config,
            &PluginsListArgs {
                connect: false,
                json: false,
                plugins: Vec::new(),
                plugin_roots: Vec::new(),
            },
        )
        .await
        .expect("list output");

        assert_eq!(output.plugins.len(), 1);
        assert!(output.plugins[0].disabled);
    }

    #[tokio::test]
    async fn plugin_list_connect_skips_disabled_runtime_inspection() {
        let (_tempdir, config) = test_config();
        let plugin_root = config.paths.plugins_dir.join("demo");
        write_plugin_manifest(
            &plugin_root,
            r#"{
                "name":"demo",
                "version":"0.1.0",
                "runtime":{"command":"definitely-not-a-real-plugin-runtime"}
            }"#,
            &[],
        );
        fs::write(
            plugin_root.join(claude_plugins::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("disabled marker");

        let output = build_plugins_list_output(
            &config,
            &PluginsListArgs {
                connect: true,
                json: false,
                plugins: Vec::new(),
                plugin_roots: Vec::new(),
            },
        )
        .await
        .expect("list output");

        let live = output.plugins[0].live.as_ref().expect("live record");
        assert_eq!(live.status, "skipped");
        assert_eq!(
            live.error.as_deref(),
            Some("plugin is currently disabled by marker file")
        );
    }

    #[tokio::test]
    async fn plugin_inspect_skips_runtime_for_disabled_plugin() {
        let (_tempdir, config) = test_config();
        let plugin_root = config.paths.plugins_dir.join("demo");
        write_plugin_manifest(
            &plugin_root,
            r#"{
                "name":"demo",
                "version":"0.1.0",
                "runtime":{"command":"definitely-not-a-real-plugin-runtime"}
            }"#,
            &[],
        );
        fs::write(
            plugin_root.join(claude_plugins::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("disabled marker");

        let output = build_plugins_inspect_output(
            &config,
            &PluginsInspectArgs {
                plugin: "demo".to_owned(),
                json: false,
                plugin_roots: Vec::new(),
            },
        )
        .await
        .expect("inspect output");

        let live = output.plugin.live.as_ref().expect("live record");
        assert!(output.plugin.disabled);
        assert_eq!(live.status, "skipped");
        assert_eq!(
            live.error.as_deref(),
            Some("plugin is currently disabled by marker file")
        );
    }

    #[tokio::test]
    async fn plugin_invoke_refuses_disabled_plugin() {
        let (_tempdir, config) = test_config();
        let plugin_root = config.paths.plugins_dir.join("demo");
        write_plugin_manifest(
            &plugin_root,
            r#"{
                "name":"demo",
                "version":"0.1.0",
                "runtime":{"command":"definitely-not-a-real-plugin-runtime"}
            }"#,
            &[],
        );
        fs::write(
            plugin_root.join(claude_plugins::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("disabled marker");

        let error = build_plugins_invoke_output(
            &config,
            &PluginsInvokeArgs {
                plugin: "demo".to_owned(),
                action: "ping".to_owned(),
                json: false,
                args: Vec::new(),
                input_json: None,
                plugin_roots: Vec::new(),
            },
        )
        .await
        .expect_err("disabled plugin should not invoke");

        assert!(
            error
                .to_string()
                .contains("Plugin `demo` is currently disabled")
        );
    }

    #[tokio::test]
    async fn plugin_discovery_respects_user_setting_sources_but_keeps_explicit_roots() {
        let (_tempdir, mut config) = test_config();
        let profile_plugin = config.paths.plugins_dir.join("demo");
        write_plugin(&profile_plugin, "0.1.0", &[]);

        let explicit_root = config.cwd.join("extra-plugins");
        let explicit_plugin = explicit_root.join("explicit");
        write_plugin(&explicit_plugin, "0.2.0", &[]);

        let output = build_plugins_list_output(
            &config,
            &PluginsListArgs {
                connect: false,
                json: false,
                plugins: Vec::new(),
                plugin_roots: vec![explicit_root.clone()],
            },
        )
        .await
        .expect("list output");
        assert_eq!(output.plugins.len(), 2);

        config.allowed_setting_sources = vec![SettingSource::Local];
        let discovery = discover_runtime_plugins(&config, std::slice::from_ref(&explicit_root));
        assert_eq!(discovery.plugins.len(), 1);
        assert_eq!(discovery.plugins[0].origin_kind, "explicit");
        assert_eq!(discovery.plugins[0].bundle.manifest.version, "0.2.0");

        let output = build_plugins_list_output(
            &config,
            &PluginsListArgs {
                connect: false,
                json: false,
                plugins: Vec::new(),
                plugin_roots: vec![explicit_root.clone()],
            },
        )
        .await
        .expect("filtered list output");
        assert_eq!(output.plugins.len(), 1);
        assert_eq!(output.plugins[0].origin_kind, "explicit");
        assert_eq!(output.plugins[0].version, "0.2.0");
    }

    #[test]
    fn runtime_plugin_resolution_uses_setting_source_filtering() {
        let (_tempdir, mut config) = test_config();
        let profile_plugin = config.paths.plugins_dir.join("demo");
        write_plugin(&profile_plugin, "0.1.0", &[]);

        config.allowed_setting_sources = vec![SettingSource::Local];
        assert!(resolve_runtime_plugin(&config, "demo", &[]).is_err());

        let explicit_root = config.cwd.join("extra-plugins");
        let explicit_plugin = explicit_root.join("demo");
        write_plugin(&explicit_plugin, "0.2.0", &[]);
        let resolution =
            resolve_runtime_plugin(&config, "demo", &[explicit_root]).expect("explicit resolve");
        assert_eq!(resolution.entry.origin_kind, "explicit");
        assert_eq!(resolution.entry.bundle.manifest.version, "0.2.0");
    }
}
