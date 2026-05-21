use claude_config::RuntimeConfig;
use claude_core::ConversationEntry;
use claude_permissions::PermissionBroker;
use claude_provider::context::ContextWindowManager;
use claude_provider::cost::CostTracker;
use claude_tools::tasks::task_snapshots;

use super::{mcp, plugins, skills};

pub fn render(
    config: &RuntimeConfig,
    conversation: &[ConversationEntry],
    context_manager: &ContextWindowManager,
    cost_tracker: &CostTracker,
    broker: &dyn PermissionBroker,
) {
    let (enabled_plugins, disabled_plugins) = plugins::discovered_plugin_counts(config);
    println!("Session:  {}", config.session_id);
    println!(
        "Name:     {}",
        config.session_name.as_deref().unwrap_or("(auto)")
    );
    println!("CWD:      {}", config.cwd.display());
    println!(
        "Provider: {} ({})",
        config.provider.name,
        config.provider.protocol.as_str()
    );
    println!(
        "Model:    {}",
        config.provider.model.as_deref().unwrap_or("(missing)")
    );
    println!(
        "Fallback: {}",
        config.fallback_model.as_deref().unwrap_or("(none)")
    );
    println!(
        "Effort:   {}",
        config.effort.as_deref().unwrap_or("(default)")
    );
    println!(
        "Auth:     {}",
        config.auth_source.as_deref().unwrap_or("(missing)")
    );
    println!(
        "Permission mode: {}",
        config.permission_mode.as_legacy_str()
    );
    println!("Conversation entries: {}", conversation.len());
    println!("Tracked tasks: {}", task_snapshots().len());
    println!(
        "Surface counts: mcp={} plugins={} disabled_plugins={} skills={}",
        mcp::discovered_server_count(config),
        enabled_plugins,
        disabled_plugins,
        skills::discovered_skill_count(config)
    );
    println!(
        "Tool filters: allow={} deny={}",
        config.allowed_tools.len(),
        config.disallowed_tools.len()
    );
    if !config.setting_sources.is_empty() {
        println!("Setting sources:");
        for source in config.setting_sources.iter().take(6) {
            println!("  - {source}");
        }
        if config.setting_sources.len() > 6 {
            println!("  - ... {} more", config.setting_sources.len() - 6);
        }
    }
    println!(
        "Allowed setting sources: {}",
        config
            .allowed_setting_sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "Settings files: {}",
        if config.settings_files.is_empty() {
            "(auto discovery only)".to_owned()
        } else {
            config
                .settings_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "Permission rules: {} loaded, {} decisions recorded",
        broker.layered_rules().len(),
        broker.audit_records().len()
    );
    let usage_ratio = context_manager.usage_ratio(conversation);
    println!("Context usage: {:.1}%", usage_ratio * 100.0);
    let total_cost = cost_tracker.total_cost_usd();
    if total_cost > 0.0 {
        println!("Estimated cost: ${total_cost:.6} USD");
    }
}
