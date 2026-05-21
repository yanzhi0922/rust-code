//! Metrics and statistics commands: `/usage`, `/extraUsage`, `/stats`, `/insights`.

use claude_config::RuntimeConfig;
use claude_provider::cost::CostTracker;
use claude_session::SessionStore;

/// Dispatch `/usage` — show detailed token usage statistics.
pub fn render_usage(config: &RuntimeConfig, store: &SessionStore) {
    println!("Usage statistics:");
    println!("  session: {}", config.session_id);

    match store.load_session_bundle(config.session_id) {
        Ok(bundle) => {
            println!("  input tokens:  {}", bundle.stats.usage.input_tokens);
            println!("  output tokens: {}", bundle.stats.usage.output_tokens);
            println!(
                "  total tokens:  {}",
                bundle.stats.usage.input_tokens + bundle.stats.usage.output_tokens
            );
            println!("  tool calls:    {}", bundle.stats.tool_call_count);
            println!("  errors:        {}", bundle.stats.error_count);
        }
        Err(error) => println!("  (unable to load session bundle: {error})"),
    }
}

/// Dispatch `/extraUsage` — show extra usage details.
pub fn render_extra_usage(config: &RuntimeConfig, store: &SessionStore) {
    println!("Extra usage details:");
    println!("  session: {}", config.session_id);
    println!("  provider: {}", config.provider.name);
    println!(
        "  model:    {}",
        config.provider.model.as_deref().unwrap_or("(default)")
    );

    match store.load_session_bundle(config.session_id) {
        Ok(bundle) => {
            println!("  tool calls: {}", bundle.stats.tool_call_count);
            println!("  errors:     {}", bundle.stats.error_count);
            println!("  events:     {}", bundle.stats.total_events);
        }
        Err(error) => println!("  (unable to load session bundle: {error})"),
    }
}

/// Dispatch `/stats` — show session/tool/error statistics.
pub fn render_stats(config: &RuntimeConfig, store: &SessionStore, cost_tracker: &CostTracker) {
    println!("Statistics:");
    println!("  session: {}", config.session_id);

    match store.list_sessions() {
        Ok(sessions) => {
            println!("  total sessions: {}", sessions.len());
        }
        Err(error) => println!("  (unable to list sessions: {error})"),
    }

    match store.load_session_bundle(config.session_id) {
        Ok(bundle) => {
            println!("  conversation entries: {}", bundle.conversation.len());
            println!("  tool calls:         {}", bundle.stats.tool_call_count);
            println!("  errors:             {}", bundle.stats.error_count);
            println!("  input tokens:       {}", bundle.stats.usage.input_tokens);
            println!("  output tokens:      {}", bundle.stats.usage.output_tokens);
        }
        Err(error) => println!("  (unable to load session bundle: {error})"),
    }

    print!("{}", cost_tracker.summary());
}

/// Dispatch `/insights` — show session analysis report.
pub fn render_insights(config: &RuntimeConfig, store: &SessionStore) {
    println!("Insights for session {}:", config.session_id);

    match store.load_session_bundle(config.session_id) {
        Ok(bundle) => {
            let total_tokens = bundle.stats.usage.input_tokens + bundle.stats.usage.output_tokens;
            let tool_ratio = if total_tokens > 0 {
                bundle.stats.tool_call_count as f64 / total_tokens as f64 * 1000.0
            } else {
                0.0
            };
            let error_rate = if bundle.stats.tool_call_count > 0 {
                bundle.stats.error_count as f64 / bundle.stats.tool_call_count as f64 * 100.0
            } else {
                0.0
            };

            println!("  title:         {}", bundle.summary.title);
            println!("  entries:       {}", bundle.conversation.len());
            println!("  total tokens:  {total_tokens}");
            println!("  tool calls:    {}", bundle.stats.tool_call_count);
            println!("  errors:        {}", bundle.stats.error_count);
            println!("  tool density:  {tool_ratio:.2} calls/1k tokens");
            println!("  error rate:    {error_rate:.1}%");

            if error_rate > 50.0 {
                println!("  ⚠ High error rate — consider reviewing tool permissions.");
            }
            if total_tokens > 100_000 {
                println!("  ℹ High token usage — consider using /compact to reduce context.");
            }
        }
        Err(error) => println!("  (unable to load session bundle: {error})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::tempdir;

    fn build_test_config() -> (RuntimeConfig, SessionStore) {
        let temp = tempdir().expect("tempdir should work");
        let root = temp.keep();
        let config = load_runtime_config(
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
        .expect("config should load");
        let store = SessionStore::open(config.paths.clone()).expect("store should open");
        (config, store)
    }

    #[test]
    fn usage_shows_token_stats() {
        let (config, store) = build_test_config();
        render_usage(&config, &store);
    }

    #[test]
    fn extra_usage_shows_cache_details() {
        let (config, store) = build_test_config();
        render_extra_usage(&config, &store);
    }

    #[test]
    fn stats_shows_overall_statistics() {
        let (config, store) = build_test_config();
        let cost_tracker = CostTracker::new();
        render_stats(&config, &store, &cost_tracker);
    }

    #[test]
    fn insights_shows_analysis() {
        let (config, store) = build_test_config();
        render_insights(&config, &store);
    }

    #[test]
    fn usage_displays_session_id() {
        let (config, store) = build_test_config();
        // Verify the function runs without panic for a fresh session
        render_usage(&config, &store);
    }

    #[test]
    fn extra_usage_displays_provider_info() {
        let (config, store) = build_test_config();
        render_extra_usage(&config, &store);
    }

    #[test]
    fn stats_with_cost_tracker() {
        let (config, store) = build_test_config();
        let cost_tracker = CostTracker::new();
        render_stats(&config, &store, &cost_tracker);
    }

    #[test]
    fn insights_for_empty_session() {
        let (config, store) = build_test_config();
        render_insights(&config, &store);
    }
}
