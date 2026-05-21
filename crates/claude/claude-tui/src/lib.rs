//! Interactive TUI for Remote Code Rust.
//!
//! Uses ratatui with crossterm backend for a full terminal UI with:
//! - Multi-turn conversation with the provider
//! - Automatic tool execution with permission checks
//! - Context window compaction
//! - Cost tracking across turns
//! - Slash commands for session management
//! - Vim mode with Normal / Insert / Command / Visual / Search modes
//! - Virtual scrolling for 10 000+ messages
//! - Sidebar with sessions, tools, MCP, and help tabs
//!
//! # Architecture
//!
//! - [`app`] — Application state machine
//! - [`vim`] — Vim mode state machine
//! - [`scroll`] — Virtual scroll engine
//! - [`event`] — Event handling (keyboard / mouse / resize)
//! - [`render`] — Main render entry point
//! - [`components`] — UI components (chat, input, status bar, sidebar, etc.)
//! - [`style`] — Style / theme system
//! - [`message`] — Message types and rendering helpers
//! - [`syntax`] — Syntax highlighting for code blocks
//! - [`layout`] — Layout calculation
//! - [`commands`] — Slash command handlers
//! - [`slash_commands`] — Slash command dispatch
//! - [`tab_complete`] — Tab completion logic
//! - [`theme`] — Legacy theme (crossterm colors, used by command subsystem)

// New ratatui-based modules.
pub mod app;
pub mod components;
pub mod event;
pub mod history_search;
pub mod ide;
pub mod keybindings;
pub mod layout;
pub mod message;
pub mod notifications;
pub mod output_styles;
mod prompt_runtime;
pub mod render;
mod runtime_hooks;
pub mod scroll;
pub mod style;
pub mod syntax;
pub mod vim;
pub mod virtual_scroll;

// Preserved modules (existing functionality).
mod commands;
mod slash_commands;
mod tab_complete;
mod theme;

use std::io;
use std::sync::Arc;

use anyhow::Result;
use claude_config::{RuntimeConfig, restamp_runtime_session};
use claude_core::{ConversationEntry, Message, SystemMessageSubtype};
use claude_permissions::PermissionBroker;
use claude_provider::context::ContextWindowManager;
use claude_provider::cost::CostTracker;
use claude_provider::{ConversationBackend, ProviderClient, ProviderCompatBackend};
use claude_session::resume_state::{PendingToolCall, ResumeState};
use claude_session::runtime_context::{
    persist_runtime_config_session_context, repair_interrupted_tool_batch,
    restore_runtime_config_session_context,
};
use claude_session::{SessionStore, conversation::ensure_conversation_initialized};
use claude_tools::{
    ToolExecutionContext, ToolRuntimePolicy,
    agent::{parse_delegate_progress_event, render_delegate_progress_event},
    configure_tool_runtime_policy, execute_tool_call,
    git::{apply_worktree_tool_result_to_runtime, sync_tool_context_from_runtime},
    mcp_catalog::{clear_runtime_mcp_catalog_cache, execute_runtime_mcp_prompt_command},
    mcp_runtime::runtime_mcp_policy_entries,
    plan_mode::normalize_exit_plan_mode_tool_calls,
    runtime_plan_mode::{
        build_runtime_plan_mode, inject_plan_mode_runtime_messages, install_plan_mode_runtime,
    },
    shell::ShellExecutionPolicy,
    tasks::stop_and_clear_tracked_tasks,
    tool_result_storage,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{App, AppAction};
use event::{convert_event, handle_event};
use prompt_runtime::{
    PromptRuntimeOverrides, clear_runtime_system_prompt_state,
    conversation_with_runtime_user_context_with_settings, refresh_runtime_system_prompt,
    runtime_prompt_settings,
};
use runtime_hooks::{
    PreparedToolCall, SessionHookRunOutcome, ToolHookRunOutcome, apply_post_tool_hooks,
    apply_pre_tool_use_hooks, discover_runtime_session_hooks, ensure_session_start_hooks,
    run_session_end_hooks,
};

// ---------------------------------------------------------------------------
// Public types re-exports
// ---------------------------------------------------------------------------

pub use app::{ActivePanel, SidebarTab};
pub use message::{
    ChatMessage, McpServerStatus, MessageRole, ModelInfo, PermissionRequest, StatusBarInfo,
    ToolCallInfo,
};

struct McpPromptCommandInput<'a> {
    command_name: &'a str,
    args: &'a str,
}

fn mcp_prompt_command_input(input: &str) -> Option<McpPromptCommandInput<'_>> {
    let trimmed = input.trim();
    let command_name = trimmed.split_whitespace().next()?;
    command_name.strip_prefix("/mcp__")?;
    let args = trimmed
        .strip_prefix(command_name)
        .map(str::trim_start)
        .unwrap_or_default();
    Some(McpPromptCommandInput {
        command_name: command_name.trim_start_matches('/'),
        args,
    })
}

fn config_prompt_runtime_overrides(config: &RuntimeConfig) -> PromptRuntimeOverrides {
    PromptRuntimeOverrides {
        system_prompt: config.system_prompt.clone(),
        append_system_prompt: config.append_system_prompt.clone(),
        ..PromptRuntimeOverrides::default()
    }
}
pub use style::StyleConfig;
pub use vim::{VimAction, VimMode, VimStateMachine};

/// Built-in slash command names in the SDK/headless init format.
///
/// Claude Code emits command names without the leading `/` in `system/init`.
/// The interactive TUI still accepts commands with `/`; this helper is for
/// protocol surfaces that need the user-invocable command catalog.
#[must_use]
pub fn builtin_protocol_slash_command_names() -> Vec<String> {
    commands::command_names()
        .into_iter()
        .map(|command| command.trim_start_matches('/').to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Text-based dashboard that prints session info and recent sessions.
///
/// This is a non-interactive overview used by the `remote-code tui` subcommand.
pub fn run_dashboard(config: &RuntimeConfig, store: &SessionStore) -> Result<()> {
    println!("Remote Code Rust — Dashboard");
    println!();
    println!("Profile:  {}", config.paths.profile_dir.display());
    println!(
        "Provider: {} ({})",
        config.provider.name,
        config.provider.protocol.as_str()
    );
    println!(
        "Model:    {}",
        config.provider.model.as_deref().unwrap_or("(missing)")
    );
    println!(
        "Base URL: {}",
        config.provider.base_url.as_deref().unwrap_or("(missing)")
    );
    println!(
        "API key:  {}",
        if config.provider.api_key.is_some() {
            "present"
        } else {
            "missing"
        }
    );
    println!();

    match store.list_sessions() {
        Ok(sessions) => {
            println!("Recent Sessions:");
            if sessions.is_empty() {
                println!("  (no sessions)");
            } else {
                for session in sessions.iter().take(20) {
                    println!(
                        "  {}  {}  {}",
                        session.session_id, session.updated_at, session.title
                    );
                }
            }
        }
        Err(error) => println!("  (error listing sessions: {error})"),
    }
    Ok(())
}

/// Run the interactive TUI application with ratatui.
///
/// This is the main interactive mode entry point, providing:
/// - Multi-turn conversation with the provider
/// - Automatic tool execution with permission checks
/// - Context window compaction
/// - Cost tracking across turns
/// - Slash commands for session management
/// - Full Vim mode with ratatui rendering
#[allow(clippy::too_many_lines)]
pub async fn run_tui_app(mut config: RuntimeConfig, store: &SessionStore) -> Result<()> {
    let provider_client = Arc::new(ProviderClient::new()?);
    let mut backend = ProviderCompatBackend::new(Arc::clone(&provider_client), &config.provider);
    let (mut plan_mode_controller, mut broker) = build_runtime_plan_mode(&config, store)?;
    let mut _plan_mode_runtime_guard = install_plan_mode_runtime(plan_mode_controller.clone())?;
    let mut session_hooks = discover_runtime_session_hooks(&config);

    let model_name = config
        .provider
        .model
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    let context_manager = ContextWindowManager::for_model(&model_name);
    let cost_tracker = CostTracker::new();
    let mut conversation = load_or_create_conversation(store, &config)?;
    let prompt_overrides = config_prompt_runtime_overrides(&config);
    refresh_runtime_system_prompt(
        &config,
        &mut conversation,
        &prompt_overrides,
        &backend.discovered_tool_scope(),
    )
    .await?;
    let startup_hook_outcome =
        ensure_session_start_hooks(&session_hooks, &config, store, &mut conversation).await?;

    // Set up ratatui terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend_term = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_term)?;

    // Initialize app state.
    let mut app = App::new();
    app.status.model_name = model_name.to_owned();
    app.model_info.name = model_name.to_owned();
    app.model_info.provider = config.provider.name.clone();
    seed_session_banner_messages(&mut app, &config);
    render_transcript_display_events(&mut app, store, config.session_id)?;
    render_session_hook_outcome(&mut app, &startup_hook_outcome);

    let mut theme = theme::Theme::dark();

    // Main event loop.
    loop {
        // Update scroll viewport.
        let area = terminal
            .size()
            .map_or(ratatui::layout::Rect::new(0, 0, 80, 24), |s| {
                ratatui::layout::Rect::new(0, 0, s.width, s.height)
            });
        app.update_scroll_viewport(area.height.saturating_sub(3) as usize);

        // Render.
        terminal.draw(|f| {
            render::render(f, &app);
        })?;

        // Poll for events.
        if !crossterm::event::poll(std::time::Duration::from_millis(100))? {
            app.tick_spinner();
            continue;
        }

        let crossterm_event = match crossterm::event::read()? {
            crossterm::event::Event::Key(key) => {
                // Suppress key release events.
                if key.kind == crossterm::event::KeyEventKind::Release {
                    continue;
                }
                crossterm::event::Event::Key(key)
            }
            crossterm::event::Event::Resize(_, _) => {
                continue;
            }
            _ => continue,
        };

        let Some(app_event) = convert_event(crossterm_event) else {
            continue;
        };

        let action = handle_event(&mut app, app_event);

        match action {
            AppAction::None => {}
            AppAction::Quit => break,
            AppAction::Cancel => {}
            AppAction::Submit(input) => {
                // Add user message.
                app.add_message(ChatMessage::user(input.clone()));

                // Temporarily restore terminal for conversation output.
                disable_raw_mode()?;
                crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;

                // Execute conversation turn.
                if let Err(error) = run_conversation_turn(
                    &backend,
                    &mut config,
                    store,
                    &session_hooks,
                    &mut conversation,
                    &context_manager,
                    broker.as_ref(),
                    &cost_tracker,
                    &input,
                )
                .await
                {
                    let err_str = format!("{error:#}");
                    let is_transient = err_str.contains("timeout")
                        || err_str.contains("429")
                        || err_str.contains("rate limit")
                        || err_str.contains("503")
                        || err_str.contains("500")
                        || err_str.contains("connection");
                    if is_transient {
                        eprintln!("⚠ Transient error (recovered): {err_str}");
                        eprintln!(
                            "  Your session is preserved. The next request will retry automatically."
                        );
                    } else {
                        eprintln!("⚠ Error: {err_str}");
                        eprintln!(
                            "  Your message was saved. Type to continue or /help for options."
                        );
                    }
                }

                // Update status bar with cost info.
                app.status.cost = cost_tracker.total_cost_usd();

                // Restore ratatui terminal.
                enable_raw_mode()?;
                crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
                let new_backend = CrosstermBackend::new(io::stdout());
                terminal = Terminal::new(new_backend)?;
            }
            AppAction::SlashCommand(cmd) => {
                if let Some(command_input) = mcp_prompt_command_input(&cmd) {
                    let mut tool_context = ToolExecutionContext::from_runtime_config(&config);
                    tool_context.sub_agent = Some(backend.sub_agent_completion());
                    let run_prompt = execute_runtime_mcp_prompt_command(
                        command_input.command_name,
                        command_input.args,
                        &tool_context,
                    )
                    .await;
                    match run_prompt {
                        Ok(blocks) => {
                            let prompt_entry = ConversationEntry::user_with_content_blocks(blocks);
                            let preview = prompt_entry
                                .text
                                .trim()
                                .lines()
                                .next()
                                .unwrap_or(command_input.command_name)
                                .to_owned();
                            store.append_conversation_entry(config.session_id, &prompt_entry)?;
                            conversation.push(prompt_entry);
                            app.add_message(ChatMessage::system(format!(
                                "Executed: {}",
                                command_input.command_name
                            )));
                            app.add_message(ChatMessage::user(preview));

                            disable_raw_mode()?;
                            crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;

                            if let Err(error) = run_conversation_turn_with_messages(
                                &backend,
                                &mut config,
                                store,
                                &session_hooks,
                                &mut conversation,
                                &context_manager,
                                broker.as_ref(),
                                &cost_tracker,
                                Vec::new(),
                            )
                            .await
                            {
                                eprintln!("⚠ Error: {error:#}");
                                eprintln!(
                                    "  The MCP prompt was saved. Type to continue or /help for options."
                                );
                            }

                            app.status.cost = cost_tracker.total_cost_usd();

                            enable_raw_mode()?;
                            crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
                            let new_backend = CrosstermBackend::new(io::stdout());
                            terminal = Terminal::new(new_backend)?;
                        }
                        Err(error) => {
                            app.add_message(ChatMessage::system(format!(
                                "MCP prompt command failed: {error:#}"
                            )));
                        }
                    }
                    continue;
                }
                let is_clear_command = cmd.trim() == "/clear";
                let mut pre_outputs = Vec::new();
                if is_clear_command {
                    match run_session_end_hooks(&session_hooks, &config, store).await {
                        Ok(outcome) => {
                            pre_outputs.extend(outcome.warnings);
                        }
                        Err(error) => pre_outputs
                            .push(format!("SessionEnd hook error before /clear: {error:#}")),
                    }
                }
                let sc_action = handle_slash_command_safe(
                    &cmd,
                    &config,
                    store,
                    &mut conversation,
                    &context_manager,
                    &cost_tracker,
                    broker.as_ref(),
                    &mut theme,
                    Some(plan_mode_controller.as_ref()),
                );
                let action = sc_action.action;
                let outputs = sc_action.outputs;
                let queued_prompt = sc_action.queued_prompt;
                let next_session_id = sc_action.next_session_id;
                let config_patch = sc_action.config_patch;
                let meta_messages = sc_action.meta_messages;
                let mut post_switch_hook_outcome = SessionHookRunOutcome::default();

                match action {
                    slash_commands::SlashCommandAction::Quit => {
                        // Restore terminal before exiting.
                        disable_raw_mode()?;
                        crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
                        let cost = cost_tracker.total_cost_usd();
                        if cost > 0.0 {
                            println!();
                            print!("{}", cost_tracker.summary());
                        }
                        return Ok(());
                    }
                    slash_commands::SlashCommandAction::ResetScroll => {
                        app.scroll.scroll_to_bottom();
                    }
                    slash_commands::SlashCommandAction::Continue => {}
                }

                if let Some(next_session_id) = next_session_id {
                    if is_clear_command {
                        stop_and_clear_tracked_tasks("stopped by session clear")?;
                        let session_dir = config
                            .paths
                            .sessions_dir
                            .join(config.session_id.to_string());
                        let _ = tool_result_storage::cleanup_tool_results_for_session(
                            &session_dir,
                            std::time::SystemTime::now(),
                        );
                        clear_runtime_system_prompt_state(config.session_id);
                    }
                    clear_runtime_mcp_catalog_cache().await;
                    restamp_runtime_session(&mut config, next_session_id);
                    restore_runtime_session_context(store, &mut config)?;
                    refresh_runtime_tool_policy(&config)?;
                    backend =
                        ProviderCompatBackend::new(Arc::clone(&provider_client), &config.provider);
                    let (new_controller, new_broker) = build_runtime_plan_mode(&config, store)?;
                    plan_mode_controller = new_controller;
                    broker = new_broker;
                    _plan_mode_runtime_guard =
                        install_plan_mode_runtime(plan_mode_controller.clone())?;
                    session_hooks = discover_runtime_session_hooks(&config);
                    conversation = load_or_create_conversation(store, &config)?;
                    let prompt_overrides = config_prompt_runtime_overrides(&config);
                    if let Err(error) = refresh_runtime_system_prompt(
                        &config,
                        &mut conversation,
                        &prompt_overrides,
                        &backend.discovered_tool_scope(),
                    )
                    .await
                    {
                        post_switch_hook_outcome.warnings.push(format!(
                            "Failed to rebuild runtime system prompt for session {}: {error:#}",
                            config.session_id
                        ));
                    }
                    match ensure_session_start_hooks(
                        &session_hooks,
                        &config,
                        store,
                        &mut conversation,
                    )
                    .await
                    {
                        Ok(outcome) => post_switch_hook_outcome = outcome,
                        Err(error) => {
                            post_switch_hook_outcome.warnings.push(format!(
                                "SessionStart hook error for session {}: {error:#}",
                                config.session_id
                            ));
                        }
                    }
                    if is_clear_command {
                        seed_session_banner_messages(&mut app, &config);
                        render_transcript_display_events(&mut app, store, config.session_id)?;
                    }
                }

                if let Some(config_patch) = config_patch {
                    if let Some(brief_enabled) = config_patch.brief_enabled {
                        config.brief_enabled = brief_enabled;
                    }
                    if let Some(proactive_active) = config_patch.proactive_active {
                        config.proactive_active = proactive_active;
                    }
                    if let Some(output_style) = config_patch.output_style {
                        config.output_style = output_style;
                    }
                    if let Some(language) = config_patch.language {
                        config.language = language;
                    }
                    persist_runtime_config_session_context(store, &config)?;
                }

                for meta_message in meta_messages {
                    let meta_entry = ConversationEntry::user(meta_message);
                    store.append_conversation_entry(config.session_id, &meta_entry)?;
                    conversation.push(meta_entry);
                }

                if outputs.is_empty() && pre_outputs.is_empty() {
                    app.add_message(ChatMessage::system(format!("Executed: {cmd}")));
                } else {
                    for output in pre_outputs {
                        app.add_message(ChatMessage::system(output));
                    }
                    for output in outputs {
                        app.add_message(ChatMessage::system(output));
                    }
                }
                render_session_hook_outcome(&mut app, &post_switch_hook_outcome);

                if let Some(prompt) = queued_prompt {
                    app.add_message(ChatMessage::user(prompt.clone()));

                    disable_raw_mode()?;
                    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;

                    if let Err(error) = run_conversation_turn(
                        &backend,
                        &mut config,
                        store,
                        &session_hooks,
                        &mut conversation,
                        &context_manager,
                        broker.as_ref(),
                        &cost_tracker,
                        &prompt,
                    )
                    .await
                    {
                        let err_str = format!("{error:#}");
                        let is_transient = err_str.contains("timeout")
                            || err_str.contains("429")
                            || err_str.contains("rate limit")
                            || err_str.contains("503")
                            || err_str.contains("500")
                            || err_str.contains("connection");
                        if is_transient {
                            eprintln!("⚠ Transient error (recovered): {err_str}");
                            eprintln!(
                                "  Your session is preserved. The next request will retry automatically."
                            );
                        } else {
                            eprintln!("⚠ Error: {err_str}");
                            eprintln!(
                                "  Your message was saved. Type to continue or /help for options."
                            );
                        }
                    }

                    app.status.cost = cost_tracker.total_cost_usd();

                    enable_raw_mode()?;
                    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
                    let new_backend = CrosstermBackend::new(io::stdout());
                    terminal = Terminal::new(new_backend)?;
                }
            }
        }
    }

    // Restore terminal.
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;

    // Print cost summary on exit.
    let cost = cost_tracker.total_cost_usd();
    if cost > 0.0 {
        println!();
        print!("{}", cost_tracker.summary());
    }

    Ok(())
}

fn refresh_runtime_tool_policy(config: &RuntimeConfig) -> Result<()> {
    let session_dir = config
        .paths
        .sessions_dir
        .join(config.session_id.to_string());
    configure_tool_runtime_policy(ToolRuntimePolicy {
        allowed_tools: config.allowed_tools.clone(),
        disallowed_tools: config.disallowed_tools.clone(),
        task_output_dir: Some(
            config
                .paths
                .artifacts_dir
                .join("tasks")
                .join(config.session_id.to_string()),
        ),
        tasks_dir: Some(claude_tools::tasks::task_list_base_dir()),
        tool_results_dir: Some(session_dir.join("tool-results")),
        shell_policy: ShellExecutionPolicy {
            block_inline_cwd: true,
            allow_background: true,
            block_destructive_git: true,
            max_capture_chars: ShellExecutionPolicy::default().max_capture_chars,
            output_dir: Some(
                config
                    .paths
                    .artifacts_dir
                    .join("shell")
                    .join(config.session_id.to_string()),
            ),
            tool_results_dir: Some(session_dir.join("tool-results")),
            task_output_dir: Some(
                config
                    .paths
                    .artifacts_dir
                    .join("tasks")
                    .join(config.session_id.to_string()),
            ),
        },
        mcp_servers: runtime_mcp_policy_entries(config, &[]),
    })
}

fn seed_session_banner_messages(app: &mut App, config: &RuntimeConfig) {
    let model_name = config
        .provider
        .model
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    app.reset_for_new_session();
    app.status.model_name = model_name.to_owned();
    app.model_info.name = model_name.to_owned();
    app.model_info.provider = config.provider.name.clone();
    app.add_message(ChatMessage::system(
        "Remote Code Rust — Interactive Mode".to_owned(),
    ));
    app.add_message(ChatMessage::system(format!(
        "Session: {} | Model: {} | Provider: {}",
        config.session_id, model_name, config.provider.name,
    )));
    app.add_message(ChatMessage::system(
        "Type /help for commands, /quit to exit. Vim mode: Esc=normal, i=insert.".to_owned(),
    ));
}

fn render_transcript_display_events(
    app: &mut App,
    store: &SessionStore,
    session_id: uuid::Uuid,
) -> Result<()> {
    let transcript = store.load_transcript(session_id)?;
    for event in transcript.iter_events() {
        if let Some(entry) = event.conversation.as_ref() {
            if entry.role == claude_core::ConversationRole::System
                && entry.name.as_deref() == Some("memory_saved")
            {
                continue;
            }
            render_conversation_entry(app, entry);
        }
    }
    for message in transcript.memory_saved_messages() {
        if let Message::System(system) = message
            && system.subtype == SystemMessageSubtype::MemorySaved
            && let Some((written_paths, team_count)) = parse_memory_saved_payload(&system.text)
        {
            app.add_message(ChatMessage::memory_saved(written_paths, team_count));
        }
    }
    Ok(())
}

fn parse_memory_saved_payload(text: &str) -> Option<(Vec<String>, Option<usize>)> {
    let payload = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let written_paths = payload
        .get("writtenPaths")
        .or_else(|| payload.get("written_paths"))?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    let team_count = payload
        .get("teamCount")
        .or_else(|| payload.get("team_count"))
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as usize);
    Some((written_paths, team_count))
}

fn restore_runtime_session_context(store: &SessionStore, config: &mut RuntimeConfig) -> Result<()> {
    restore_runtime_config_session_context(store, config)
}

fn render_conversation_entry(app: &mut App, entry: &ConversationEntry) {
    match entry.role {
        claude_core::ConversationRole::User => {
            app.add_message(ChatMessage::user(entry.text.clone()));
        }
        claude_core::ConversationRole::Assistant => {
            app.add_message(ChatMessage::assistant(entry.text.clone()));
        }
        claude_core::ConversationRole::System => {
            app.add_message(ChatMessage::system(entry.text.clone()));
        }
        claude_core::ConversationRole::Tool => {
            app.add_message(ChatMessage::tool(entry.text.clone()));
        }
    }
}

fn render_session_hook_outcome(app: &mut App, outcome: &SessionHookRunOutcome) {
    for warning in &outcome.warnings {
        app.add_message(ChatMessage::system(format!("Hook warning: {warning}")));
    }
    for entry in &outcome.appended_entries {
        render_conversation_entry(app, entry);
    }
}

// ---------------------------------------------------------------------------
// Conversation logic (preserved from original)
// ---------------------------------------------------------------------------

/// Load an existing conversation or create a new one with a system prompt.
fn load_or_create_conversation(
    store: &SessionStore,
    config: &RuntimeConfig,
) -> Result<Vec<ConversationEntry>> {
    persist_runtime_config_session_context(store, config)?;
    let mut conversation = ensure_conversation_initialized(
        store,
        config.session_id,
        &config.cwd,
        &config.provider.name,
        config.provider.model.as_deref(),
        config.session_name.as_deref(),
    )?;
    repair_interrupted_tool_batch(store, config.session_id, &mut conversation)?;
    inject_plan_mode_runtime_messages(store, config.session_id, &mut conversation)?;
    Ok(conversation)
}

/// Run a full multi-turn conversation turn.
///
/// This implements the core loop:
/// 1. Add user message
/// 2. Call provider
/// 3. If tool calls → execute tools → go to 2
/// 4. If no tool calls → display response → done
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_conversation_turn(
    backend: &ProviderCompatBackend,
    config: &mut RuntimeConfig,
    store: &SessionStore,
    discovery: &runtime_hooks::RuntimeSessionHookDiscovery,
    conversation: &mut Vec<ConversationEntry>,
    context_manager: &ContextWindowManager,
    broker: &dyn PermissionBroker,
    cost_tracker: &CostTracker,
    prompt: &str,
) -> Result<()> {
    run_conversation_turn_with_messages(
        backend,
        config,
        store,
        discovery,
        conversation,
        context_manager,
        broker,
        cost_tracker,
        vec![ConversationEntry::user(prompt)],
    )
    .await
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_conversation_turn_with_messages(
    backend: &ProviderCompatBackend,
    config: &mut RuntimeConfig,
    store: &SessionStore,
    discovery: &runtime_hooks::RuntimeSessionHookDiscovery,
    conversation: &mut Vec<ConversationEntry>,
    context_manager: &ContextWindowManager,
    broker: &dyn PermissionBroker,
    cost_tracker: &CostTracker,
    user_entries: Vec<ConversationEntry>,
) -> Result<()> {
    for user_entry in user_entries {
        store.append_conversation_entry(config.session_id, &user_entry)?;
        conversation.push(user_entry);
    }

    let mut tool_context = ToolExecutionContext {
        cwd: config.cwd.clone(),
        original_cwd: config.original_cwd.clone(),
        active_worktree_session: config.active_worktree_session.clone(),
        timeout_ms: config.provider.timeout_ms,
        sub_agent: Some(backend.sub_agent_completion()),
        progress_cb: Some(Arc::new(|msg: &str| {
            if let Some(event) = parse_delegate_progress_event(msg) {
                println!("{}", render_delegate_progress_event(&event));
            } else {
                println!("{msg}");
            }
        })),
        task_stack: std::sync::Arc::new(parking_lot::Mutex::new(
            claude_core::task_stack::TaskStack::default(),
        )),
        read_file_state: claude_tools::FileStateCache::new(),
        sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };

    let model_name = config
        .provider
        .model
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());

    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;

    for turn in 0..config.max_turns {
        let budget_snapshot = context_manager.budget_snapshot(conversation);
        println!(
            "  [context] {:.0}% of {} tokens used (threshold {})",
            budget_snapshot.usage_ratio * 100.0,
            budget_snapshot.max_input_tokens,
            budget_snapshot.threshold_tokens(),
        );

        // Compact conversation if context window is getting full.
        if budget_snapshot.exceeds_threshold() {
            let compacted = context_manager.compact(conversation);
            let removed = conversation.len().saturating_sub(compacted.len());
            *conversation = compacted;
            if removed > 0 {
                let after = context_manager.budget_snapshot(conversation);
                println!(
                    "  [context compacted: {removed} entries summarized, now {:.0}%]",
                    after.usage_ratio * 100.0
                );
            }
        }

        refresh_runtime_system_prompt(
            config,
            conversation,
            &config_prompt_runtime_overrides(config),
            &backend.discovered_tool_scope(),
        )
        .await?;

        // Call provider
        let prompt_settings = runtime_prompt_settings(config);
        let request_conversation = conversation_with_runtime_user_context_with_settings(
            config,
            conversation,
            &config_prompt_runtime_overrides(config),
            &prompt_settings,
        )
        .await;
        let mut response = backend.complete(&request_conversation).await?;
        normalize_exit_plan_mode_tool_calls(&mut response.tool_calls);
        total_input_tokens += response.usage.input_tokens;
        total_output_tokens += response.usage.output_tokens;

        // Record usage in cost tracker
        cost_tracker.record(
            &model_name,
            response.usage.input_tokens,
            response.usage.output_tokens,
        );

        // Build and persist assistant entry
        let assistant_entry = ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: claude_core::ConversationRole::Assistant,
            text: response.text.clone(),
            history_text: response.history_text.clone(),
            content_blocks: response.content_blocks.clone(),
            tool_calls: response.tool_calls.clone(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        };
        store.append_conversation_entry(config.session_id, &assistant_entry)?;
        conversation.push(assistant_entry);

        // If no tool calls, display the response and finish
        if response.tool_calls.is_empty() {
            store.clear_resume_state(config.session_id)?;
            println!();
            println!("{}", response.text);
            println!(
                "-- turn {}, {} input tokens, {} output tokens, stop={}",
                turn + 1,
                total_input_tokens,
                total_output_tokens,
                response.stop_reason,
            );
            return Ok(());
        }

        let pending_tool_calls = response
            .tool_calls
            .iter()
            .map(|tool_call| PendingToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                input: tool_call.input.clone(),
            })
            .collect::<Vec<_>>();
        store.save_resume_state(
            config.session_id,
            &ResumeState::from_pending_calls(pending_tool_calls),
        )?;

        // Execute tool calls
        println!();
        if !response.text.is_empty() {
            println!("{}", response.text);
        }

        for tool_call in &response.tool_calls {
            let tool_start = std::time::Instant::now();
            println!("  ⏳ [tool] {} — running...", tool_call.name);
            let audit_count_before = broker.audit_records().len();
            let PreparedToolCall {
                call: effective_tool_call,
                blocked_reason,
                appended_entries: pre_hook_entries,
            } = apply_pre_tool_use_hooks(discovery, config, store, conversation, tool_call).await?;
            print_hook_entries(&pre_hook_entries);

            let mut tool_result = match blocked_reason {
                Some(reason) => claude_core::ToolResult {
                    content: reason,
                    is_error: true,
                    content_blocks: Vec::new(),
                    follow_up_user_blocks: Vec::new(),
                },
                None => {
                    match execute_tool_call(&effective_tool_call, &tool_context, broker).await {
                        Ok(result) => result,
                        Err(error) => {
                            let elapsed = tool_start.elapsed();
                            eprintln!(
                                "  ✗ [tool] {} — error ({:.1}s): {error}",
                                effective_tool_call.name,
                                elapsed.as_secs_f64()
                            );
                            claude_core::ToolResult {
                                content: format!("Tool execution error: {error}"),
                                is_error: true,
                                content_blocks: Vec::new(),
                                follow_up_user_blocks: Vec::new(),
                            }
                        }
                    }
                }
            };
            let new_audits = broker
                .audit_records()
                .into_iter()
                .skip(audit_count_before)
                .collect::<Vec<_>>();
            for audit in new_audits {
                store.append_named_event(
                    config.session_id,
                    "permission_decision",
                    serde_json::to_value(&audit)?,
                )?;
            }
            let ToolHookRunOutcome {
                appended_entries: post_hook_entries,
            } = apply_post_tool_hooks(
                discovery,
                config,
                store,
                conversation,
                &effective_tool_call,
                &mut tool_result,
            )
            .await?;
            print_hook_entries(&post_hook_entries);
            let elapsed = tool_start.elapsed();
            let status = if tool_result.is_error { "✗" } else { "✓" };
            println!(
                "  {status} [tool] {} — done ({:.1}s)",
                effective_tool_call.name,
                elapsed.as_secs_f64()
            );

            let truncated_output =
                context_manager.truncate_tool_output_default(&tool_result.content);

            print_tool_result(&effective_tool_call.name, &tool_result, &truncated_output);

            if apply_worktree_tool_result_to_runtime(
                &effective_tool_call.name,
                &effective_tool_call.input,
                &tool_result,
                config,
                &mut tool_context,
            )? {
                persist_runtime_config_session_context(store, config)?;
                sync_tool_context_from_runtime(config, &mut tool_context);
            }

            let mut tool_entry = ConversationEntry::tool(
                effective_tool_call.id.clone(),
                effective_tool_call.name.clone(),
                truncated_output,
                tool_result.is_error,
            );
            tool_entry.content_blocks = tool_result.content_blocks.clone();
            store.append_conversation_entry(config.session_id, &tool_entry)?;
            conversation.push(tool_entry);
            if !tool_result.follow_up_user_blocks.is_empty() {
                let follow_up_entry = ConversationEntry::user_with_content_blocks(
                    tool_result.follow_up_user_blocks.clone(),
                );
                store.append_conversation_entry(config.session_id, &follow_up_entry)?;
                conversation.push(follow_up_entry);
            }
        }
        store.clear_resume_state(config.session_id)?;
    }

    eprintln!(
        "Maximum turn budget reached ({}) without a final assistant reply.",
        config.max_turns
    );
    Ok(())
}

/// Handle slash commands with a safe wrapper that returns the action.
#[allow(clippy::too_many_arguments)]
fn handle_slash_command_safe(
    input: &str,
    config: &RuntimeConfig,
    store: &SessionStore,
    conversation: &mut Vec<ConversationEntry>,
    context_manager: &ContextWindowManager,
    cost_tracker: &CostTracker,
    broker: &dyn PermissionBroker,
    theme: &mut theme::Theme,
    plan_mode_controller: Option<&claude_tools::runtime_plan_mode::RuntimePlanModeController>,
) -> slash_commands::SlashCommandResult {
    slash_commands::handle_slash_command(
        input,
        config,
        store,
        conversation,
        context_manager,
        cost_tracker,
        broker,
        theme,
        plan_mode_controller,
    )
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Format and print a tool execution result.
fn print_tool_result(tool_name: &str, result: &claude_core::ToolResult, display_text: &str) {
    if result.is_error {
        println!(
            "  [tool] {tool_name} — ERROR: {}",
            truncate_display(display_text, 200)
        );
    } else {
        println!("  [tool] {tool_name} — OK");
        for line in display_text.lines().take(5) {
            println!("    {}", truncate_display(line, 120));
        }
        let total_lines = display_text.lines().count();
        if total_lines > 5 {
            println!("    ... ({} more lines)", total_lines - 5);
        }
    }
}

fn print_hook_entries(entries: &[ConversationEntry]) {
    for entry in entries {
        match entry.role {
            claude_core::ConversationRole::System | claude_core::ConversationRole::User => {
                println!("{}", entry.text);
            }
            _ => {}
        }
    }
}

/// Truncate a string for display purposes.
fn truncate_display(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{
        ConversationRole, InputFormat, OutputFormat, PermissionMode, ProviderProtocol, ToolCall,
    };
    use claude_session::SessionStore;
    use claude_session::resume_state::{PendingToolCall, ResumeState};
    use tempfile::tempdir;

    #[test]
    fn truncate_display_short() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_display_long() {
        let result = truncate_display("abcdefghij", 5);
        assert_eq!(result, "abcde...");
    }

    #[test]
    fn mcp_prompt_command_input_parses_command_and_args() {
        let parsed = mcp_prompt_command_input("/mcp__docs__plan rust async")
            .expect("mcp prompt command should parse");

        assert_eq!(parsed.command_name, "mcp__docs__plan");
        assert_eq!(parsed.args, "rust async");
        assert!(mcp_prompt_command_input("/mcp list").is_none());
    }

    #[test]
    fn app_default_state() {
        let app = App::new();
        assert!(!app.should_quit());
        assert!(app.input.is_empty());
    }

    #[test]
    fn style_config_default() {
        let style = StyleConfig::dark();
        assert_eq!(style.name, "dark");
    }

    #[test]
    fn vim_mode_labels() {
        assert_eq!(VimMode::Normal.label(), "NORMAL");
        assert_eq!(VimMode::Insert.label(), "INSERT");
    }

    fn test_config() -> (tempfile::TempDir, RuntimeConfig, SessionStore) {
        let tempdir = tempdir().expect("tempdir should succeed");
        let config = load_runtime_config(
            Some(tempdir.path().to_path_buf()),
            Some(tempdir.path().join(".remote-code-rust")),
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
                provider: Some("mock-provider".to_owned()),
                base_url: Some("https://example.invalid/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");
        let store = SessionStore::open(config.paths.clone()).expect("store should open");
        (tempdir, config, store)
    }

    #[test]
    fn load_or_create_conversation_repairs_interrupted_tool_batches() {
        let (_tempdir, config, store) = test_config();

        let _ = load_or_create_conversation(&store, &config).expect("conversation should load");

        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls.push(ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: serde_json::json!({"path": "src/lib.rs"}),
        });
        store
            .append_conversation_entry(config.session_id, &assistant)
            .expect("assistant should append");
        store
            .save_resume_state(
                config.session_id,
                &ResumeState::from_pending_calls(vec![PendingToolCall {
                    id: "call-1".to_owned(),
                    name: "replace_in_file".to_owned(),
                    input: serde_json::json!({"path": "src/lib.rs"}),
                }]),
            )
            .expect("resume state should save");

        let repaired =
            load_or_create_conversation(&store, &config).expect("conversation should repair");
        let repaired_tool = repaired
            .iter()
            .find(|entry| entry.role == ConversationRole::Tool)
            .expect("synthetic tool result should exist");
        assert_eq!(repaired_tool.tool_call_id.as_deref(), Some("call-1"));
        assert!(repaired_tool.is_error);
        assert!(repaired_tool.text.contains("interrupted"));

        let transcript = store
            .load_transcript(config.session_id)
            .expect("transcript should load");
        let persisted = transcript
            .latest_named_event_as::<serde_json::Value>("session_context")
            .expect("session context event should exist");
        assert!(persisted.is_some());
    }
}
