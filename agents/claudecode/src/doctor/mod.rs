mod checks;
pub(crate) mod install;
mod network;
mod providers;

use anyhow::Result;
use claude_config::RuntimeConfig;

use crate::cli::DoctorArgs;
use checks::DoctorReport;

pub(crate) async fn run_doctor(config: &RuntimeConfig, args: DoctorArgs) -> Result<()> {
    let report = checks::collect_report(config, &args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report);
    }
    Ok(())
}

fn print_text_report(report: &DoctorReport) {
    println!("=== Remote Code Rust — Doctor Report ===");
    println!();
    println!("[Runtime]");
    println!("  version:        {}", report.runtime.version);
    println!("  cwd:            {}", report.runtime.cwd.display());
    println!("  profile dir:    {}", report.runtime.profile_dir.display());
    println!("  session id:     {}", report.runtime.session_id);
    println!(
        "  session name:   {}",
        report.runtime.session_name.as_deref().unwrap_or("(auto)")
    );
    println!("  permission mode: {}", report.runtime.permission_mode);
    println!("  input format:   {}", report.runtime.input_format);
    println!("  output format:  {}", report.runtime.output_format);
    println!(
        "  allowed sources: {}",
        if report.runtime.allowed_setting_sources.is_empty() {
            "(none)".to_owned()
        } else {
            report.runtime.allowed_setting_sources.join(", ")
        }
    );
    println!(
        "  settings files: {}",
        if report.runtime.settings_files.is_empty() {
            "(auto discovery only)".to_owned()
        } else {
            report
                .runtime
                .settings_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "  setting sources: {}",
        if report.runtime.setting_sources.is_empty() {
            "(defaults)".to_owned()
        } else {
            report.runtime.setting_sources.join(", ")
        }
    );

    println!();
    println!("[Install]");
    println!("  source:         {}", report.install.source);
    println!("  executable:     {}", report.install.executable.display());
    println!("  update support: {}", report.install.update_supported);
    println!("  repository:     {}", report.install.repository_url);
    println!(
        "  release repo:   {}",
        report
            .install
            .repository_slug
            .as_deref()
            .unwrap_or("(non-github)")
    );

    println!();
    println!("[Provider]");
    println!("  name:           {}", report.provider.name);
    println!("  protocol:       {}", report.provider.protocol);
    println!(
        "  base URL:       {}",
        report.provider.base_url.as_deref().unwrap_or("(missing)")
    );
    println!(
        "  model:          {}",
        report.provider.model.as_deref().unwrap_or("(missing)")
    );
    println!(
        "  api key:        {}",
        if report.provider.api_key_present {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "  auth source:    {}",
        report
            .provider
            .auth_source
            .as_deref()
            .unwrap_or("(missing)")
    );
    println!(
        "  effort:         {}",
        report.provider.effort.as_deref().unwrap_or("(default)")
    );
    println!(
        "  fallback model: {}",
        report
            .provider
            .fallback_model
            .as_deref()
            .unwrap_or("(none)")
    );
    println!(
        "  context window: {} tokens (output reserve: {})",
        report.provider.context_window_tokens, report.provider.output_reserve_tokens
    );
    println!(
        "  capabilities:   multimodal={}, reasoning={}",
        report.provider.multimodal, report.provider.reasoning
    );
    if let Some(probe) = &report.provider.probe {
        println!(
            "  probe:          {} ({} ms, {})",
            probe.outcome.label(),
            probe.latency_ms,
            probe.detail
        );
    }

    println!();
    println!("[Tools]");
    println!("  builtin tools:  {}", report.tools.builtin_tools);
    println!(
        "  allow-list:     {}",
        if report.tools.allowed_tools.is_empty() {
            "(all)".to_owned()
        } else {
            report.tools.allowed_tools.join(", ")
        }
    );
    println!(
        "  deny-list:      {}",
        if report.tools.disallowed_tools.is_empty() {
            "(none)".to_owned()
        } else {
            report.tools.disallowed_tools.join(", ")
        }
    );

    println!();
    println!("[Permissions]");
    println!("  layered rules:  {}", report.permissions.layered_rules);
    if report.permissions.rule_sources.is_empty() {
        println!("  sources:        (none)");
    } else {
        for source in &report.permissions.rule_sources {
            println!("  source:         {} ({})", source.source, source.count);
        }
    }

    println!();
    println!("[Extensions]");
    println!("  skills:         {}", report.extensions.skills);
    println!("  plugins:        {}", report.extensions.plugins);
    println!("  disabled plugins: {}", report.extensions.disabled_plugins);
    println!("  plugin runtimes: {}", report.extensions.plugin_runtimes);
    println!("  mcp servers:    {}", report.extensions.mcp_servers);
    println!(
        "  disabled mcp:   {}",
        report.extensions.disabled_mcp_servers
    );
    println!("  hooks:          {}", report.extensions.hooks);

    if let Some(network) = &report.network {
        println!();
        println!("[Network]");
        for probe in network {
            println!(
                "  {}: {} ({} ms, {})",
                probe.label,
                probe.outcome.label(),
                probe.latency_ms,
                probe.detail
            );
        }
    }

    if let Some(env_providers) = &report.env_providers {
        println!();
        println!("[Env Providers]");
        if env_providers.is_empty() {
            println!("  (none discovered)");
        } else {
            for provider in env_providers {
                println!(
                    "  {}  {}  {}  model={}  key={}",
                    provider.name,
                    provider.protocol,
                    provider.base_url.as_deref().unwrap_or("(default)"),
                    provider.model.as_deref().unwrap_or("(default)"),
                    if provider.api_key_present {
                        "present"
                    } else {
                        "missing"
                    }
                );
            }
        }
    }

    println!();
    println!("[Readiness]");
    println!(
        "  status:         {}",
        if report.ok { "READY" } else { "NOT READY" }
    );

    if !report.issues.is_empty() {
        println!();
        println!("[Issues]");
        for issue in &report.issues {
            println!("  - {issue}");
        }
    }

    if !report.warnings.is_empty() {
        println!();
        println!("[Warnings]");
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }
}
