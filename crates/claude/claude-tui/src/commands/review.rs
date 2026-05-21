use claude_config::RuntimeConfig;
use claude_tools::{ToolExecutionContext, git};
use serde_json::Value;

pub fn render(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::suggest_pr_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            println!("Review surface:");
            println!(
                "  suggested title: {}",
                value["suggested_title"].as_str().unwrap_or("(missing)")
            );
            let diff_stat = value["diff_stat"].as_str().unwrap_or_default();
            if diff_stat.is_empty() {
                println!("  diff stat:        (clean or unavailable)");
            } else {
                println!("  diff stat:");
                for line in diff_stat.lines() {
                    println!("    {line}");
                }
            }
            let recent_commits = value["recent_commits"].as_str().unwrap_or_default();
            if !recent_commits.is_empty() {
                println!("  recent commits:");
                for line in recent_commits.lines().take(10) {
                    println!("    {line}");
                }
            }
            if let Some(note) = value["note"].as_str() {
                println!("  note:            {note}");
            }
        }
        Err(error) => eprintln!("Failed to load review surface: {error}"),
    }
}
