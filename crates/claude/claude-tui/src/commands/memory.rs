use std::path::Path;

use claude_config::RuntimeConfig;
use claude_runtime_prompt::RuntimePromptSettings;
use claude_session::session_memory::session_memory_path;
use directories::BaseDirs;

pub fn render(config: &RuntimeConfig) {
    let prompt_settings = RuntimePromptSettings::from_config(config);
    let user_claude = BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".claude").join("CLAUDE.md"))
        .unwrap_or_else(|| config.cwd.join("CLAUDE.md"));
    let project_claude = config.cwd.join("CLAUDE.md");
    let auto_memory_dir = prompt_settings
        .auto_memory_read_dir
        .as_deref()
        .or(prompt_settings.auto_memory_permission_dir.as_deref())
        .map(std::path::PathBuf::from);
    let team_memory_dir = prompt_settings
        .team_memory_read_dir
        .as_deref()
        .map(std::path::PathBuf::from);
    let session_summary = session_memory_path(config);

    println!("Memory surface:");
    print_file_entry("user", &user_claude);
    print_file_entry("project", &project_claude);
    if let Some(auto_memory_dir) = auto_memory_dir {
        print_dir_entry("auto dir", &auto_memory_dir);
        print_file_entry("auto idx", &auto_memory_dir.join("MEMORY.md"));
    }
    if let Some(team_memory_dir) = team_memory_dir {
        print_dir_entry("team dir", &team_memory_dir);
        print_file_entry("team idx", &team_memory_dir.join("MEMORY.md"));
    }
    print_file_entry("session", &session_summary);
}

fn print_file_entry(label: &str, path: &Path) {
    let exists = path.exists();
    let size_bytes = std::fs::metadata(path)
        .map(|meta| meta.len())
        .unwrap_or_default();
    println!(
        "  {label:<8} {} ({}, {} bytes)",
        path.display(),
        if exists { "present" } else { "missing" },
        size_bytes
    );
}

fn print_dir_entry(label: &str, path: &Path) {
    println!(
        "  {label:<8} {} ({})",
        path.display(),
        if path.exists() { "present" } else { "missing" }
    );
}
