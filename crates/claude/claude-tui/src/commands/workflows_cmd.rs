//! `/workflows` command — list and manage workflow scripts.

use claude_config::RuntimeConfig;

/// Dispatch the `/workflows` command with subcommands.
pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let mut parts = input.split_whitespace();
    let _command = parts.next(); // skip "/workflows"
    let subcommand = parts.next();

    match subcommand {
        None | Some("list") => render_list(config),
        Some("show") => match parts.next() {
            Some(name) => render_show(name, config),
            None => println!("Usage: /workflows show <name>"),
        },
        Some("run") => match parts.next() {
            Some(name) => render_run(name, config),
            None => println!("Usage: /workflows run <name>"),
        },
        Some(other) => {
            println!("Unknown subcommand '{other}'");
            println!("Usage: /workflows [list|show <name>|run <name>]");
        }
    }
}

fn render_list(config: &RuntimeConfig) {
    println!("Workflow Scripts");
    println!("────────────────");

    let workflows_dir = config.cwd.join(".remote-code-rust").join("workflows");
    if workflows_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&workflows_dir)
            .map(|dir| {
                dir.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|ext| ext == "sh" || ext == "ps1" || ext == "md")
                    })
                    .collect()
            })
            .unwrap_or_default();

        if entries.is_empty() {
            println!("  (no workflow scripts found)");
        } else {
            for entry in &entries {
                let name = entry.file_name().to_string_lossy().to_string();
                println!("  {name}");
            }
        }
    } else {
        println!("  (no workflows directory found)");
        println!("  Create .remote-code-rust/workflows/ to add workflow scripts");
    }
    println!();
    println!("Usage: /workflows [list|show <name>|run <name>]");
}

fn render_show(name: &str, config: &RuntimeConfig) {
    let workflows_dir = config.cwd.join(".remote-code-rust").join("workflows");
    let path = workflows_dir.join(name);

    if !path.exists() {
        println!("Workflow '{name}' not found");
        return;
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            println!("Workflow: {name}");
            println!("────────────────");
            println!("{content}");
        }
        Err(e) => println!("Error reading workflow '{name}': {e}"),
    }
}

fn render_run(name: &str, config: &RuntimeConfig) {
    let workflows_dir = config.cwd.join(".remote-code-rust").join("workflows");
    let path = workflows_dir.join(name);

    if !path.exists() {
        println!("Workflow '{name}' not found");
        return;
    }

    println!("Workflow execution for '{name}' is not yet supported in this version.");
    println!("To run manually:");
    println!("  bash {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::tempdir;

    fn build_test_config() -> RuntimeConfig {
        let temp = tempdir().expect("tempdir should work");
        let root = temp.keep();
        load_runtime_config(
            Some(root.clone()),
            Some(root.join(".remote-code-rust")),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("glm-coding".to_owned()),
                base_url: Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("glm-5.1".to_owned()),
                protocol: Some(claude_core::ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load")
    }

    #[test]
    fn dispatch_list_no_panic() {
        let config = build_test_config();
        dispatch("/workflows list", &config);
    }

    #[test]
    fn dispatch_no_args_no_panic() {
        let config = build_test_config();
        dispatch("/workflows", &config);
    }

    #[test]
    fn dispatch_show_no_name() {
        let config = build_test_config();
        dispatch("/workflows show", &config);
    }

    #[test]
    fn dispatch_show_nonexistent() {
        let config = build_test_config();
        dispatch("/workflows show nonexistent", &config);
    }

    #[test]
    fn dispatch_run_nonexistent() {
        let config = build_test_config();
        dispatch("/workflows run nonexistent", &config);
    }

    #[test]
    fn dispatch_unknown_subcommand() {
        let config = build_test_config();
        dispatch("/workflows foo", &config);
    }
}
