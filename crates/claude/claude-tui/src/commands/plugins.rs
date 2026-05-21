use std::path::Path;

use claude_config::{RuntimeConfig, SettingSource};

pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/plugins")
        .unwrap_or_default()
        .trim();
    if remainder.is_empty() || remainder == "list" {
        render(config);
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "show" | "inspect" => {
            let Some(name) = parts.next() else {
                println!(
                    "Usage: /plugins [list|show <plugin>|validate [plugin]|enable <plugin>|disable <plugin>]"
                );
                return;
            };
            render_plugin(config, name);
        }
        "validate" => {
            if let Some(name) = parts.next() {
                validate_plugin(config, Some(name));
            } else {
                validate_plugin(config, None);
            }
        }
        "enable" | "disable" => {
            let Some(name) = parts.next() else {
                println!(
                    "Usage: /plugins {} <plugin>",
                    remainder.split_whitespace().next().unwrap_or("enable")
                );
                return;
            };
            set_plugin_enabled(config, name, remainder.starts_with("enable"));
        }
        other => {
            println!("Unknown /plugins subcommand '{other}'.");
            println!(
                "Usage: /plugins [list|show <plugin>|validate [plugin]|enable <plugin>|disable <plugin>]"
            );
        }
    }
}

pub fn render(config: &RuntimeConfig) {
    match discover_visible_plugins(config) {
        Ok(mut plugins) => {
            if plugins.is_empty() {
                println!(
                    "Plugins: none discovered in {}.",
                    config.paths.plugins_dir.display()
                );
                return;
            }
            plugins.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
            println!("Plugins ({}):", plugins.len());
            for plugin in plugins {
                let disabled = plugin
                    .root
                    .join(claude_plugins::PLUGIN_DISABLED_MARKER)
                    .exists();
                println!(
                    "  {}  {}  runtime={}  skills={}  mcp={}  disabled={}  {}",
                    plugin.manifest.name,
                    plugin.manifest.version,
                    yes_no(plugin.runtime_config().is_some()),
                    yes_no(plugin.skills_root().is_some()),
                    yes_no(plugin.mcp_config_path().is_some()),
                    yes_no(disabled),
                    plugin.manifest_path.display()
                );
            }
            println!("Tip: /plugins show <plugin> or /plugins validate");
        }
        Err(error) => eprintln!(
            "Failed to discover plugins in {}: {error}",
            config.paths.plugins_dir.display()
        ),
    }
}

pub(crate) fn discovered_plugin_counts(config: &RuntimeConfig) -> (usize, usize) {
    match discover_visible_plugins(config) {
        Ok(plugins) => {
            let disabled = plugins.iter().filter(|plugin| plugin.is_disabled()).count();
            (plugins.len().saturating_sub(disabled), disabled)
        }
        Err(_) => (0, 0),
    }
}

fn render_plugin(config: &RuntimeConfig, name: &str) {
    match resolve_plugin(config, name) {
        Ok(plugin) => {
            let report = claude_plugins::validate_plugin_bundle(&plugin);
            let disabled = plugin
                .root
                .join(claude_plugins::PLUGIN_DISABLED_MARKER)
                .exists();
            println!(
                "Plugin: {} {}",
                plugin.manifest.name, plugin.manifest.version
            );
            println!("  root: {}", plugin.root.display());
            println!("  manifest: {}", plugin.manifest_path.display());
            println!("  disabled: {}", yes_no(disabled));
            println!(
                "  surfaces: runtime={}  skills={}  hooks={}  mcp={}  apps={}",
                yes_no(plugin.runtime_config().is_some()),
                yes_no(plugin.skills_root().is_some()),
                yes_no(plugin.hooks_config_path().is_some()),
                yes_no(plugin.mcp_config_path().is_some()),
                yes_no(plugin.app_manifest_path().is_some())
            );
            if let Some(summary) = plugin.manifest.description.as_deref() {
                println!("  description: {summary}");
            }
            if let Some(runtime) = plugin.runtime_config() {
                println!("  runtime command: {}", runtime.command);
                if !runtime.args.is_empty() {
                    println!("  runtime args: {}", runtime.args.join(" "));
                }
                println!("  runtime cwd: {}", runtime.cwd.display());
            }
            if let Some(path) = plugin.skills_root() {
                println!("  skills root: {}", path.display());
            }
            if let Some(path) = plugin.hooks_config_path() {
                println!("  hooks: {}", path.display());
            }
            if let Some(path) = plugin.mcp_config_path() {
                println!("  mcp: {}", path.display());
            }
            if let Some(path) = plugin.app_manifest_path() {
                println!("  app manifest: {}", path.display());
            }
            println!(
                "  validation: errors={} warnings={}",
                report.errors.len(),
                report.warnings.len()
            );
            for error in report.errors {
                println!("    error: {error}");
            }
            for warning in report.warnings {
                println!("    warning: {warning}");
            }
        }
        Err(error) => eprintln!("{error}"),
    }
}

fn validate_plugin(config: &RuntimeConfig, name: Option<&str>) {
    let plugins = match discover_visible_plugins(config) {
        Ok(plugins) => plugins,
        Err(error) => {
            eprintln!(
                "Failed to discover plugins in {}: {error}",
                config.paths.plugins_dir.display()
            );
            return;
        }
    };

    let filtered = plugins
        .into_iter()
        .filter(|plugin| name.is_none_or(|expected| plugin.manifest.name == expected))
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        if let Some(name) = name {
            println!("No plugin named `{name}` was found.");
        } else {
            println!("No plugins found.");
        }
        return;
    }

    for plugin in filtered {
        let report = claude_plugins::validate_plugin_bundle(&plugin);
        println!(
            "{}  errors={} warnings={} skills={} runtime={} mcp={}",
            report.plugin_name,
            report.errors.len(),
            report.warnings.len(),
            report.bundled_skills,
            report.has_runtime,
            report.has_mcp
        );
        for error in report.errors {
            println!("  error: {error}");
        }
        for warning in report.warnings {
            println!("  warning: {warning}");
        }
    }
}

fn set_plugin_enabled(config: &RuntimeConfig, name: &str, enabled: bool) {
    let destination = config.paths.plugins_dir.join(name);
    if !destination.exists() {
        println!(
            "No installed plugin named `{name}` exists in {}.",
            config.paths.plugins_dir.display()
        );
        return;
    }
    if !destination.starts_with(&config.paths.plugins_dir) {
        eprintln!(
            "Refusing to mutate plugin outside {}",
            config.paths.plugins_dir.display()
        );
        return;
    }

    let marker_path = destination.join(claude_plugins::PLUGIN_DISABLED_MARKER);
    if enabled {
        if !marker_path.exists() {
            println!("Plugin {name} already enabled.");
            return;
        }
        if let Err(error) = std::fs::remove_file(&marker_path) {
            eprintln!("Failed to enable plugin {name}: {error}");
            return;
        }
        println!("Plugin {name} enabled.");
    } else {
        if marker_path.exists() {
            println!("Plugin {name} already disabled.");
            return;
        }
        if let Err(error) = std::fs::write(&marker_path, b"disabled\n") {
            eprintln!("Failed to disable plugin {name}: {error}");
            return;
        }
        println!("Plugin {name} disabled.");
    }
}

fn resolve_plugin(
    config: &RuntimeConfig,
    name: &str,
) -> anyhow::Result<claude_plugins::PluginBundle> {
    let plugins = discover_visible_plugins(config)?;
    let mut matches = plugins
        .into_iter()
        .filter(|plugin| plugin.manifest.name == name)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => anyhow::bail!("No plugin named `{name}` was found"),
        1 => Ok(matches.pop().expect("single plugin must exist")),
        _ => {
            let locations = matches
                .into_iter()
                .map(|plugin| plugin.manifest_path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("Plugin `{name}` is ambiguous across: {locations}");
        }
    }
}

fn discover_visible_plugins(
    config: &RuntimeConfig,
) -> Result<Vec<claude_plugins::PluginBundle>, claude_plugins::PluginError> {
    if !setting_source_enabled(config, SettingSource::User) {
        return Ok(Vec::new());
    }
    claude_plugins::discover_plugins_including_disabled(&config.paths.plugins_dir)
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[allow(dead_code)]
fn _display_path(path: &Path) -> String {
    path.display().to_string()
}
