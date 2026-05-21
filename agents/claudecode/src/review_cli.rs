use anyhow::Result;
use claude_config::RuntimeConfig;
use claude_tools::{ToolExecutionContext, git};
use serde_json::Value;

use crate::cli::ReviewArgs;

pub(crate) fn run_review(config: &RuntimeConfig, args: ReviewArgs) -> Result<()> {
    let output = build_review_output(config)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("Review surface:");
    println!(
        "  suggested title: {}",
        output["suggested_title"].as_str().unwrap_or("(missing)")
    );
    let diff_stat = output["diff_stat"].as_str().unwrap_or_default();
    if diff_stat.is_empty() {
        println!("  diff stat:        (clean or unavailable)");
    } else {
        println!("  diff stat:");
        for line in diff_stat.lines() {
            println!("    {line}");
        }
    }
    let recent_commits = output["recent_commits"].as_str().unwrap_or_default();
    if !recent_commits.is_empty() {
        println!("  recent commits:");
        for line in recent_commits.lines().take(10) {
            println!("    {line}");
        }
    }
    if let Some(note) = output["note"].as_str() {
        println!("  note:            {note}");
    }
    Ok(())
}

fn build_review_output(config: &RuntimeConfig) -> Result<Value> {
    let context = ToolExecutionContext::from_runtime_config(config);
    let payload = git::suggest_pr_tool(&context)?;
    Ok(serde_json::from_str(&payload)?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use tempfile::tempdir;

    use super::build_review_output;

    #[test]
    fn review_output_contains_expected_keys() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
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

        let output = build_review_output(&config).expect("review output");
        assert!(output.get("suggested_title").is_some());
        assert!(output.get("diff_stat").is_some());
        assert!(output.get("recent_commits").is_some());
    }
}
