use claude_config::RuntimeConfig;
use claude_provider::model_info::{ModelCapability, get_model_info};

pub fn render(config: &RuntimeConfig) {
    let model_name = config.provider.model.as_deref().unwrap_or("unknown");
    let info = get_model_info(model_name);
    let capabilities = info
        .capabilities
        .iter()
        .map(capability_name)
        .collect::<Vec<_>>()
        .join(", ");

    println!("Model surface:");
    println!("  active:     {model_name}");
    println!(
        "  fallback:   {}",
        config.fallback_model.as_deref().unwrap_or("(none)")
    );
    println!(
        "  effort:     {}",
        config.effort.as_deref().unwrap_or("(default)")
    );
    println!("  family:     {}", info.family);
    println!("  context:    {}", info.max_context);
    println!("  max output: {}", info.max_output);
    println!("  multimodal: {}", info.multimodal);
    println!("  features:   {capabilities}");
}

fn capability_name(capability: &ModelCapability) -> &'static str {
    match capability {
        ModelCapability::Text => "text",
        ModelCapability::Vision => "vision",
        ModelCapability::Video => "video",
        ModelCapability::Audio => "audio",
        ModelCapability::ToolUse => "tool-use",
        ModelCapability::Reasoning => "reasoning",
        ModelCapability::Code => "code",
        ModelCapability::ImageGeneration => "image-generation",
    }
}
