use claude_plugins::{HookEvent, HookResult};
use claude_plugins::HookRunner as PluginHookRunner;

pub struct HookContext {
    pub event: HookEvent,
    pub data: std::collections::HashMap<String, serde_json::Value>,
}

pub async fn run_hooks(
    runner: &PluginHookRunner,
    event: &HookEvent,
    context: &HookContext,
) -> HookResult {
    let plugin_ctx = claude_plugins::HookContext {
        event: event.clone(),
        data: context.data.clone(),
    };
    runner.run_hooks(event, &plugin_ctx).await
}
