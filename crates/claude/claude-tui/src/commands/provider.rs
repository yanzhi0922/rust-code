use claude_config::RuntimeConfig;

pub fn render(config: &RuntimeConfig) {
    println!("Provider surface:");
    println!("  name:       {}", config.provider.name);
    println!("  protocol:   {}", config.provider.protocol.as_str());
    println!(
        "  base URL:   {}",
        config.provider.base_url.as_deref().unwrap_or("(missing)")
    );
    println!(
        "  auth:       {}",
        config.auth_source.as_deref().unwrap_or("(missing)")
    );
    println!("  timeout:    {} ms", config.provider.timeout_ms);
    println!("  retries:    {}", config.provider.max_retries);
    if !config.setting_sources.is_empty() {
        println!("  sources:    {}", config.setting_sources.join(", "));
    }
}
