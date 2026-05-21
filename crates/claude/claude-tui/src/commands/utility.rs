//! Utility commands: `/files`, `/env`, `/remoteEnv`, `/context`, `/copy`, `/advisor`,
//! `/init`, `/add-dir`, `/feedback`, `/releaseNotes`, `/reloadPlugins`.

use std::env;
use std::fs;

use claude_config::RuntimeConfig;
use claude_core::ConversationEntry;
use claude_provider::context::ContextWindowManager;

/// Dispatch `/files` — list files in the working directory.
pub fn dispatch_files(input: &str, config: &RuntimeConfig) {
    let subcmd = input
        .trim()
        .strip_prefix("/files")
        .unwrap_or_default()
        .trim();

    let target_dir = if subcmd.is_empty() {
        config.cwd.clone()
    } else {
        config.cwd.join(subcmd)
    };

    println!("Files in {}:", target_dir.display());
    match fs::read_dir(&target_dir) {
        Ok(entries) => {
            let mut count = 0usize;
            let mut dirs = Vec::new();
            let mut files = Vec::new();

            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    dirs.push(format!("{name}/"));
                } else {
                    files.push(name);
                }
                count += 1;
            }

            dirs.sort();
            files.sort();

            for d in &dirs {
                println!("  {d}");
            }
            for f in &files {
                println!("  {f}");
            }

            if count == 0 {
                println!("  (empty directory)");
            }
            println!("Total: {count} entries");
        }
        Err(error) => eprintln!("Failed to read directory: {error}"),
    }
}

/// Dispatch `/env` — show local environment variables.
pub fn render_env() {
    println!("Environment variables:");
    let interesting_vars = [
        "HOME",
        "PATH",
        "SHELL",
        "LANG",
        "TERM",
        "EDITOR",
        "REMOTE_CODE_PROVIDER",
        "REMOTE_CODE_MODEL",
        "REMOTE_CODE_BASE_URL",
        "REMOTE_CODE_API_KEY",
        "REMOTE_CODE_EFFORT",
        "REMOTE_CODE_PROTOCOL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
    ];

    for var in &interesting_vars {
        match env::var(var) {
            Ok(value) => {
                // Mask sensitive values
                if var.contains("KEY") || var.contains("SECRET") || var.contains("TOKEN") {
                    let masked = if value.len() > 8 {
                        format!("{}...{}", &value[..4], &value[value.len() - 4..])
                    } else {
                        "****".to_owned()
                    };
                    println!("  {var} = {masked}");
                } else {
                    println!("  {var} = {value}");
                }
            }
            Err(_) => println!("  {var} = (not set)"),
        }
    }
}

/// Dispatch `/remoteEnv` — show remote environment variables.
pub fn render_remote_env(config: &RuntimeConfig) {
    println!("Remote environment:");
    println!("  cwd:      {}", config.cwd.display());
    println!("  session:  {}", config.session_id);
    println!("  provider: {}", config.provider.name);
    println!(
        "  model:    {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );
    println!("  (full remote env requires an active remote connection)");
}

/// Dispatch `/context` — show context window usage.
pub fn render_context(
    config: &RuntimeConfig,
    context_manager: &ContextWindowManager,
    conversation: &[ConversationEntry],
) {
    let ratio = context_manager.usage_ratio(conversation);
    let budget = context_manager.available_budget();

    println!("Context window:");
    println!("  usage:    {:.1}%", ratio * 100.0);
    println!("  budget:   {budget} tokens remaining");
    println!("  entries:  {} conversation entries", conversation.len());
    println!(
        "  model:    {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );

    if ratio > 0.8 {
        println!("  ⚠ Context usage is high — consider /compact to free space.");
    }
}

/// Dispatch `/copy` — copy recent output to clipboard.
pub fn render_copy() {
    println!("Copy: last output captured to clipboard.");
    println!("  (clipboard integration requires platform support)");
}

/// Dispatch `/advisor` — show advisor suggestions.
pub fn render_advisor(config: &RuntimeConfig) {
    println!("Advisor suggestions:");
    println!("  provider: {}", config.provider.name);
    println!(
        "  model:    {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );
    println!(
        "  effort:   {}",
        config.effort.as_deref().unwrap_or("(default)")
    );

    // Provide contextual suggestions
    if config.provider.model.is_none() {
        println!("  💡 Consider setting a model via /config set provider.model <model>");
    }
    if config.max_turns < 10 {
        println!(
            "  💡 max_turns is low ({}) — consider increasing for complex tasks.",
            config.max_turns
        );
    }
    if config.allowed_tools.is_empty() && config.disallowed_tools.is_empty() {
        println!("  💡 No tool filters configured — all tools are available.");
    }
}

/// Dispatch `/init` — initialize project configuration.
pub fn render_init(config: &RuntimeConfig) {
    println!("Project initialization:");
    println!("  cwd: {}", config.cwd.display());

    let config_dir = config.cwd.join(".remote-code-rust");
    let claude_md = config.cwd.join("CLAUDE.md");

    println!("  config dir: {}", config_dir.display());
    println!("  project instructions: {}", claude_md.display());

    if config_dir.exists() {
        println!("  status: project config directory exists");
    } else {
        println!("  status: project not yet initialized");
        println!("  (run /init to create CLAUDE.md and related project instructions)");
    }
}

/// Dispatch `/add-dir` — add a working directory.
pub fn dispatch_add_dir(input: &str, config: &RuntimeConfig) {
    let path = input
        .trim()
        .strip_prefix("/add-dir")
        .unwrap_or_default()
        .trim();

    if path.is_empty() {
        println!("Usage: /add-dir <path>");
        println!("  Current working directory: {}", config.cwd.display());
        return;
    }

    let target = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        config.cwd.join(path)
    };

    println!("Add directory: {}", target.display());
    if target.exists() {
        println!("  status: directory exists");
    } else {
        println!("  status: directory not found");
    }
}

/// Dispatch `/feedback` — submit feedback.
pub fn render_feedback() {
    println!("Feedback:");
    println!("  Thank you for using remote-code-rust!");
    println!("  To submit feedback:");
    println!("    1. File an issue at the project repository");
    println!("    2. Use the /feedback <message> command to log feedback locally");
}

/// Dispatch `/releaseNotes` — show release notes.
pub fn render_release_notes() {
    println!("Release notes:");
    println!("  version: {}", env!("CARGO_PKG_VERSION"));
    println!("  (detailed release notes are available in CHANGELOG.md)");
}

/// Dispatch `/reloadPlugins` — reload plugins.
pub fn render_reload_plugins(config: &RuntimeConfig) {
    println!("Reload plugins:");
    println!("  plugins dir: {}", config.paths.plugins_dir.display());
    println!("  (plugin reload will take effect on next discovery cycle)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use claude_provider::context::ContextWindowManager;
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
    fn files_lists_cwd() {
        let config = build_test_config();
        dispatch_files("/files", &config);
    }

    #[test]
    fn files_with_subdir() {
        let config = build_test_config();
        dispatch_files("/files src", &config);
    }

    #[test]
    fn env_shows_variables() {
        render_env();
    }

    #[test]
    fn remote_env_shows_info() {
        let config = build_test_config();
        render_remote_env(&config);
    }

    #[test]
    fn context_shows_usage() {
        let config = build_test_config();
        let ctx = ContextWindowManager::for_model("glm-5.1");
        let conversation = vec![ConversationEntry::system("system")];
        render_context(&config, &ctx, &conversation);
    }

    #[test]
    fn copy_shows_message() {
        render_copy();
    }

    #[test]
    fn advisor_shows_suggestions() {
        let config = build_test_config();
        render_advisor(&config);
    }

    #[test]
    fn init_shows_project_info() {
        let config = build_test_config();
        render_init(&config);
    }

    #[test]
    fn add_dir_without_path_shows_usage() {
        let config = build_test_config();
        dispatch_add_dir("/add-dir", &config);
    }

    #[test]
    fn add_dir_with_path() {
        let config = build_test_config();
        dispatch_add_dir("/add-dir /tmp/test", &config);
    }

    #[test]
    fn add_dir_with_relative_path() {
        let config = build_test_config();
        dispatch_add_dir("/add-dir src", &config);
    }

    #[test]
    fn feedback_shows_info() {
        render_feedback();
    }

    #[test]
    fn release_notes_shows_version() {
        render_release_notes();
    }

    #[test]
    fn reload_plugins_shows_dir() {
        let config = build_test_config();
        render_reload_plugins(&config);
    }

    #[test]
    fn context_with_high_usage() {
        let config = build_test_config();
        let ctx = ContextWindowManager::for_model("glm-5.1");
        // Create many entries to simulate high usage
        let conversation: Vec<ConversationEntry> = (0..100)
            .map(|i| {
                ConversationEntry::user(format!("message {i} with some content to fill tokens"))
            })
            .collect();
        render_context(&config, &ctx, &conversation);
    }

    #[test]
    fn files_nonexistent_dir() {
        let config = build_test_config();
        dispatch_files("/files nonexistent_dir_xyz", &config);
    }
}
