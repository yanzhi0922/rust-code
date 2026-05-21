//! Slash command handler for the interactive TUI.

use claude_config::RuntimeConfig;
use claude_core::ConversationEntry;
use claude_permissions::PermissionBroker;
use claude_provider::context::ContextWindowManager;
use claude_provider::cost::CostTracker;
use claude_session::SessionStore;
use claude_tools::runtime_plan_mode::RuntimePlanModeController;

use crate::commands;
use crate::theme::Theme;

pub use crate::commands::{SlashCommandAction, SlashCommandResult};
// Re-export for convenience.

/// Handle slash commands via the modular command registry.
#[allow(clippy::too_many_arguments)]
pub fn handle_slash_command(
    input: &str,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    context_manager: &ContextWindowManager,
    cost_tracker: &CostTracker,
    broker: &dyn PermissionBroker,
    theme: &mut Theme,
    plan_mode_controller: Option<&RuntimePlanModeController>,
) -> SlashCommandResult {
    commands::dispatch_with_result(
        input,
        commands::SlashCommandContext {
            config,
            store,
            conversation,
            context_manager,
            cost_tracker,
            broker,
            theme,
            plan_mode_controller,
        },
    )
}
