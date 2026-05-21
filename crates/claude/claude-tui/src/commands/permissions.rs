use claude_config::RuntimeConfig;
use claude_permissions::{
    PermissionBroker,
    rules::{RuleAction, summarize_rule_sources},
};

pub fn dispatch(input: &str, config: &RuntimeConfig, broker: &dyn PermissionBroker) {
    let remainder = input
        .trim()
        .strip_prefix("/permissions")
        .unwrap_or_default()
        .trim();
    if remainder.is_empty() {
        render(config, broker);
        return;
    }

    let mut parts = remainder.split_whitespace();
    let action = parts.next().unwrap_or_default();
    let pattern = remainder[action.len()..].trim();
    match action {
        "allow" | "ask" | "deny" => {
            if pattern.is_empty() {
                println!("Usage: /permissions {action} <tool-pattern>");
                return;
            }
            let rule_action = match action {
                "allow" => RuleAction::Allow,
                "ask" => RuleAction::Ask,
                "deny" => RuleAction::Deny,
                _ => unreachable!(),
            };
            match broker.add_session_rule(rule_action, pattern.to_owned()) {
                Ok(()) => println!(
                    "Added session permission rule: {} {}",
                    rule_action.as_str(),
                    pattern
                ),
                Err(error) => eprintln!("Failed to add session rule: {error}"),
            }
        }
        "reset" | "clear-session" => match broker.clear_session_rules() {
            Ok(removed) => println!("Cleared {removed} session permission rule(s)."),
            Err(error) => eprintln!("Failed to clear session rules: {error}"),
        },
        other => {
            println!("Unknown /permissions subcommand '{other}'.");
            println!("Usage: /permissions [allow|ask|deny <tool-pattern>|reset]");
        }
    }

    render(config, broker);
}

pub fn render(config: &RuntimeConfig, broker: &dyn PermissionBroker) {
    println!("Permission surface:");
    println!("  mode:       {}", config.permission_mode.as_legacy_str());
    if config.allowed_tools.is_empty() {
        println!("  allow-list: (all tools allowed unless denied)");
    } else {
        println!("  allow-list: {}", config.allowed_tools.join(", "));
    }
    if config.disallowed_tools.is_empty() {
        println!("  deny-list:  (none)");
    } else {
        println!("  deny-list:  {}", config.disallowed_tools.join(", "));
    }

    let layered_rules = broker.layered_rules();
    if layered_rules.is_empty() {
        println!("  layered:    (no layered rules loaded)");
    } else {
        println!("  layered:    {} rules loaded", layered_rules.len());
        for (source, count) in summarize_rule_sources(&layered_rules) {
            println!("    {}: {} rule(s)", source.as_str(), count);
            for rule in layered_rules
                .iter()
                .filter(|rule| rule.source == source)
                .take(3)
            {
                println!("      - {} {}", rule.action.as_str(), rule.tool_pattern);
            }
            let extra = layered_rules
                .iter()
                .filter(|rule| rule.source == source)
                .count()
                .saturating_sub(3);
            if extra > 0 {
                println!("      ... and {extra} more");
            }
        }
    }

    let audits = broker.audit_records();
    if audits.is_empty() {
        println!("  recent:     (no permission decisions recorded yet)");
    } else {
        println!("  recent:     {} decision(s)", audits.len());
        for audit in audits.iter().rev().take(5) {
            let source = audit
                .source
                .map(|value| value.as_str())
                .unwrap_or("fallback");
            let pattern = audit
                .matched_pattern
                .as_deref()
                .unwrap_or("(no layered match)");
            println!(
                "    - {} {} => {} [{}]",
                audit.action.as_str(),
                audit.tool_name,
                if audit.final_allowed {
                    "allowed"
                } else {
                    "denied"
                },
                source
            );
            println!("      pattern: {pattern}");
            if let Some(reason) = &audit.reason {
                println!("      reason: {reason}");
            }
        }
    }
}
