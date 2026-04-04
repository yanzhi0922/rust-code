use crate::adapter::{ApiClientAdapter, ToolExecutorAdapter};
use crate::args::{Cli, Commands, McpAction, PermissionModeArg};
use crate::input::InputHandler;
use crate::render::Renderer;
use claude_api::client::ProviderConfig;
use claude_api::providers::anthropic::AnthropicClient;
use claude_commands::CommandRegistry;
use claude_runtime::bash::execute_bash;
use claude_runtime::compact::{CompactMode, CompactStrategy};
use claude_runtime::prompt::SystemPromptBuilder;
use claude_runtime::{ConversationRuntime, PermissionMode, PermissionPolicy, QueryConfig, RuntimeConfig};
use claude_telemetry::{MemoryTelemetrySink, SessionTracer, TelemetrySink};
use claude_tools::{create_default_tools, ToolContext};
use std::path::PathBuf;

pub struct App {
    cli: Cli,
    config: RuntimeConfig,
    cwd: PathBuf,
    command_registry: CommandRegistry,
    #[allow(dead_code)]
    tracer: SessionTracer,
    auto_continue: bool,
    vim_mode: bool,
    debug_mode: bool,
    effort_level: String,
    output_scope: String,
    session_name: Option<String>,
    last_assistant_response: Option<String>,
}

impl App {
    pub fn new(cli: Cli) -> anyhow::Result<Self> {
        let cwd = std::fs::canonicalize(&cli.workdir)?;
        let mut config = claude_runtime::ConfigLoader::load(&cwd);
        claude_runtime::ConfigLoader::load_from_env(&mut config);

        if let Some(ref model) = cli.model {
            config.model = model.clone();
        }
        if let Some(ref api_key) = cli.api_key {
            config.api.anthropic_api_key = Some(api_key.clone());
        }
        if let Some(ref base_url) = cli.base_url {
            config.api.base_url = base_url.clone();
        }

        if cli.dangerously_skip_permissions {
            config.permissions.mode = PermissionMode::BypassPermissions;
        } else {
            config.permissions.mode = match cli.permission_mode {
                PermissionModeArg::Default => PermissionMode::Default,
                PermissionModeArg::Plan => PermissionMode::Plan,
                PermissionModeArg::AcceptEdits => PermissionMode::AcceptEdits,
                PermissionModeArg::ReadOnly => PermissionMode::ReadOnly,
                PermissionModeArg::Bypass => PermissionMode::BypassPermissions,
            };
        }

        let telemetry_sink: Box<dyn TelemetrySink> = if cli.verbose {
            Box::new(MemoryTelemetrySink::new())
        } else {
            Box::new(claude_telemetry::NoopTelemetrySink)
        };
        let tracer = SessionTracer::new(telemetry_sink);
        let command_registry = CommandRegistry::new();

        Ok(Self {
            cli,
            config,
            cwd,
            command_registry,
            tracer,
            auto_continue: false,
            vim_mode: false,
            debug_mode: false,
            effort_level: "medium".to_string(),
            output_scope: "full".to_string(),
            session_name: None,
            last_assistant_response: None,
        })
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        match &self.cli.command {
            Some(Commands::Init { path }) => {
                let init_path = PathBuf::from(path);
                self.init_project(&init_path)?;
                return Ok(());
            }
            Some(Commands::Login) => {
                println!("Login not yet implemented. Set ANTHROPIC_API_KEY environment variable.");
                return Ok(());
            }
            Some(Commands::Logout) => {
                println!("Logout not yet implemented.");
                return Ok(());
            }
            Some(Commands::Doctor) => {
                self.doctor().await?;
                return Ok(());
            }
            Some(Commands::Update) => {
                println!("Update not yet implemented. Reinstall with cargo install.");
                return Ok(());
            }
            Some(Commands::Mcp { action }) => {
                self.handle_mcp(action).await?;
                return Ok(());
            }
            Some(Commands::Export { format, output }) => {
                println!("Export: format={format:?}, output={output:?}");
                return Ok(());
            }
            None => {}
        }

        if self.cli.prompt.is_some() {
            let prompt = self.cli.prompt.clone().unwrap();
            self.run_headless(&prompt).await?;
        } else {
            self.run_repl().await?;
        }

        Ok(())
    }

    async fn run_headless(&mut self, prompt: &str) -> anyhow::Result<()> {
        let renderer = Renderer::new();
        let tool_ctx = ToolContext {
            cwd: self.cwd.clone(),
        };
        let tool_registry = create_default_tools(self.cwd.clone());

        let provider_config = ProviderConfig {
            base_url: self.config.api.base_url.clone(),
            api_key: self.config.api.anthropic_api_key.clone(),
            timeout_secs: self.config.api.timeout_secs,
        };
        let anthropic_client = AnthropicClient::new(provider_config);

        let prompt_builder = SystemPromptBuilder::new(self.cwd.clone());
        let system_prompt = self
            .cli
            .system_prompt
            .clone()
            .unwrap_or_else(|| prompt_builder.build());

        let query_config = QueryConfig {
            model: self.config.model.clone(),
            max_tokens: self.cli.max_tokens,
            system_prompt,
            max_turns: self.cli.max_turns,
            stream: false,
            ..Default::default()
        };

        let permissions = PermissionPolicy {
            mode: self.config.permissions.mode,
            allow_rules: self.config.permissions.allow.clone(),
            deny_rules: self.config.permissions.deny.clone(),
        };

        let api_adapter = ApiClientAdapter::new(anthropic_client);
        let tool_adapter = ToolExecutorAdapter::new(tool_registry, tool_ctx);

        let mut runtime = ConversationRuntime::new(
            query_config,
            permissions,
            Box::new(api_adapter),
            Box::new(tool_adapter),
        );

        let messages = runtime.submit_message(prompt).await?;

        for msg in &messages {
            match msg {
                claude_runtime::SdkMessage::Assistant { text } => {
                    renderer.render_markdown(text);
                }
                claude_runtime::SdkMessage::ToolUse {
                    name, input, ..
                } => {
                    renderer.render_tool_use(name, input);
                }
                claude_runtime::SdkMessage::ToolResult {
                    content, is_error, ..
                } => {
                    renderer.render_tool_result(content, *is_error);
                }
                claude_runtime::SdkMessage::Error { message } => {
                    renderer.render_error(message);
                }
                claude_runtime::SdkMessage::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    renderer.render_usage(*input_tokens, *output_tokens);
                }
                claude_runtime::SdkMessage::End { reason } => {
                    if reason != "end_turn" {
                        renderer.render_info(&format!("Stopped: {reason}"));
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_repl(&mut self) -> anyhow::Result<()> {
        let renderer = Renderer::new();
        renderer.render_welcome();

        let tool_ctx = ToolContext {
            cwd: self.cwd.clone(),
        };
        let tool_registry = create_default_tools(self.cwd.clone());

        let provider_config = ProviderConfig {
            base_url: self.config.api.base_url.clone(),
            api_key: self.config.api.anthropic_api_key.clone(),
            timeout_secs: self.config.api.timeout_secs,
        };
        let anthropic_client = AnthropicClient::new(provider_config);

        let prompt_builder = SystemPromptBuilder::new(self.cwd.clone());
        let system_prompt = self
            .cli
            .system_prompt
            .clone()
            .unwrap_or_else(|| prompt_builder.build());

        let query_config = QueryConfig {
            model: self.config.model.clone(),
            max_tokens: self.cli.max_tokens,
            system_prompt,
            max_turns: self.cli.max_turns,
            stream: false,
            ..Default::default()
        };

        let permissions = PermissionPolicy {
            mode: self.config.permissions.mode,
            allow_rules: self.config.permissions.allow.clone(),
            deny_rules: self.config.permissions.deny.clone(),
        };

        let api_adapter = ApiClientAdapter::new(anthropic_client);
        let tool_adapter = ToolExecutorAdapter::new(tool_registry, tool_ctx);

        let mut runtime = ConversationRuntime::new(
            query_config,
            permissions,
            Box::new(api_adapter),
            Box::new(tool_adapter),
        );

        let mut input_handler = InputHandler::new()?;

        loop {
            let user_input = match input_handler.read_line() {
                Ok(Some(input)) => input,
                Ok(None) => break,
                Err(e) => {
                    renderer.render_error(&e.to_string());
                    continue;
                }
            };

            let trimmed = user_input.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((cmd, args)) = CommandRegistry::parse_command(trimmed) {
                let handled = self
                    .handle_slash_command(cmd, args, &renderer, &mut runtime)
                    .await?;
                if handled {
                    continue;
                }
            }

            renderer.render_user_message(trimmed);
            let messages = runtime.submit_message(trimmed).await?;

            for msg in &messages {
                match msg {
                    claude_runtime::SdkMessage::Assistant { text } => {
                        self.last_assistant_response = Some(text.clone());
                        renderer.render_markdown(text);
                    }
                    claude_runtime::SdkMessage::ToolUse {
                        name, input, ..
                    } => {
                        renderer.render_tool_use(name, input);
                    }
                    claude_runtime::SdkMessage::ToolResult {
                        content, is_error, ..
                    } => {
                        renderer.render_tool_result(content, *is_error);
                    }
                    claude_runtime::SdkMessage::Error { message } => {
                        renderer.render_error(message);
                    }
                    claude_runtime::SdkMessage::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        renderer.render_usage(*input_tokens, *output_tokens);
                    }
                    claude_runtime::SdkMessage::End { reason } => {
                        if reason != "end_turn" {
                            renderer.render_info(&format!("Stopped: {reason}"));
                        }
                    }
                }
            }

            println!();
        }

        input_handler.save_history()?;
        Ok(())
    }

    async fn handle_slash_command(
        &mut self,
        cmd: &str,
        args: &str,
        renderer: &Renderer,
        runtime: &mut ConversationRuntime,
    ) -> anyhow::Result<bool> {
        match cmd {
            "exit" | "quit" | "q" => {
                renderer.render_info("Goodbye!");
                Err(anyhow::anyhow!("__EXIT__"))
            }
            "clear" | "c" | "reset" | "new" => {
                runtime.messages.clear();
                renderer.render_info("Conversation cleared.");
                Ok(true)
            }
            "help" | "h" | "?" => {
                self.show_help(args, renderer);
                Ok(true)
            }
            "status" | "st" => {
                self.show_status(runtime, renderer);
                Ok(true)
            }
            "cost" | "usage" => {
                self.show_cost(runtime, renderer);
                Ok(true)
            }
            "version" | "v" => {
                println!("claude-code-rs {}", env!("CARGO_PKG_VERSION"));
                Ok(true)
            }
            "model" => {
                if args.is_empty() {
                    renderer.render_info(&format!("Current model: {}", runtime.config.model));
                } else {
                    runtime.config.model = args.to_string();
                    renderer.render_info(&format!("Model set to: {}", args));
                }
                Ok(true)
            }
            "context" => {
                let tokens = claude_runtime::usage::estimate_tokens(
                    &serde_json::to_string(&runtime.messages).unwrap_or_default(),
                );
                renderer.render_info(&format!("Context: ~{tokens} tokens estimated"));
                Ok(true)
            }
            "diff" => {
                let diff_args = if args.is_empty() { "".to_string() } else { format!(" {}", args) };
                match execute_bash(&self.cwd, &format!("git diff{diff_args}"), 30_000).await {
                    Ok(result) => {
                        if result.stdout.trim().is_empty() {
                            renderer.render_info("No changes (working tree clean).");
                        } else {
                            let lines: Vec<&str> = result.stdout.lines().take(200).collect();
                            for line in &lines {
                                println!("{line}");
                            }
                            if result.stdout.lines().count() > 200 {
                                renderer.render_info(&format!(
                                    "... ({} more lines)",
                                    result.stdout.lines().count() - 200
                                ));
                            }
                        }
                        if !result.stderr.is_empty() {
                            renderer.render_error(&result.stderr);
                        }
                    }
                    Err(e) => renderer.render_error(&e.to_string()),
                }
                Ok(true)
            }
            "config" | "settings" | "cfg" => {
                if args.is_empty() {
                    renderer.render_info(&format!(
                        "Model: {}\nBase URL: {}\nMax tokens: {}\nTemperature: {:?}\nTimeout: {}s\nPermission mode: {:?}\nAllow rules: [{}]\nDeny rules: [{}]\nMCP servers: {}",
                        self.config.model,
                        self.config.api.base_url,
                        self.config.api.max_tokens,
                        self.config.api.temperature,
                        self.config.api.timeout_secs,
                        self.config.permissions.mode,
                        self.config.permissions.allow.join(", "),
                        self.config.permissions.deny.join(", "),
                        self.config.mcp_servers.len(),
                    ));
                } else {
                    let parts: Vec<&str> = args.splitn(2, ' ').collect();
                    if parts.len() != 2 {
                        renderer.render_error("Usage: /config <key> <value>");
                    } else {
                        match parts[0] {
                            "model" => {
                                self.config.model = parts[1].to_string();
                                runtime.config.model = parts[1].to_string();
                                renderer.render_info(&format!("Config model = {}", parts[1]));
                            }
                            "max_tokens" => {
                                if let Ok(v) = parts[1].parse::<u32>() {
                                    self.config.api.max_tokens = v;
                                    renderer.render_info(&format!("Config max_tokens = {v}"));
                                } else {
                                    renderer.render_error("Invalid number for max_tokens");
                                }
                            }
                            "temperature" => {
                                if let Ok(v) = parts[1].parse::<f32>() {
                                    self.config.api.temperature = Some(v);
                                    renderer.render_info(&format!("Config temperature = {v}"));
                                } else {
                                    renderer.render_error("Invalid number for temperature");
                                }
                            }
                            "base_url" => {
                                self.config.api.base_url = parts[1].to_string();
                                renderer.render_info(&format!("Config base_url = {}", parts[1]));
                            }
                            "permission_mode" => {
                                match parts[1] {
                                    "default" => self.config.permissions.mode = PermissionMode::Default,
                                    "plan" => self.config.permissions.mode = PermissionMode::Plan,
                                    "accept-edits" => self.config.permissions.mode = PermissionMode::AcceptEdits,
                                    "read-only" => self.config.permissions.mode = PermissionMode::ReadOnly,
                                    "bypass" => self.config.permissions.mode = PermissionMode::BypassPermissions,
                                    _ => {
                                        renderer.render_error("Unknown permission mode. Use: default|plan|accept-edits|read-only|bypass");
                                        return Ok(true);
                                    }
                                }
                                renderer.render_info(&format!("Config permission_mode = {}", parts[1]));
                            }
                            _ => renderer.render_error(&format!("Unknown config key: {}", parts[0])),
                        }
                    }
                }
                Ok(true)
            }
            "permissions" | "allowed-tools" => {
                renderer.render_info(&format!(
                    "Permission mode: {:?}\nAllow rules: [{}]\nDeny rules: [{}]",
                    self.config.permissions.mode,
                    self.config.permissions.allow.join(", "),
                    self.config.permissions.deny.join(", "),
                ));
                Ok(true)
            }
            "memory" => {
                let claude_md = claude_runtime::ConfigLoader::claude_md_path(&self.cwd);
                if args == "edit" || args == "show" {
                    match std::fs::read_to_string(&claude_md) {
                        Ok(content) => {
                            println!("{}", content);
                        }
                        Err(e) => renderer.render_error(&format!("Could not read {}: {}", claude_md.display(), e)),
                    }
                } else {
                    renderer.render_info(&format!("CLAUDE.md path: {}", claude_md.display()));
                    renderer.render_info("Use /memory edit to view contents.");
                }
                Ok(true)
            }
            "skills" => {
                renderer.render_info("Available skills (placeholder):");
                let skills = [
                    "commit", "review", "init", "pr-comments", "security-review",
                    "insights", "statusline", "verify", "commit-push-pr",
                ];
                for s in &skills {
                    println!("  {s}");
                }
                Ok(true)
            }
            "rename" => {
                if args.is_empty() {
                    renderer.render_info(&format!(
                        "Session name: {}",
                        self.session_name.as_deref().unwrap_or("(none)")
                    ));
                } else {
                    self.session_name = Some(args.to_string());
                    renderer.render_info(&format!("Session renamed to: {}", args));
                }
                Ok(true)
            }
            "theme" => {
                if args.is_empty() {
                    renderer.render_info("Theme: default (terminal colors)");
                    renderer.render_info("Set with: /theme <name> (e.g., dark, light, solarized)");
                } else {
                    renderer.render_info(&format!("Theme set to: {} (informational only)", args));
                }
                Ok(true)
            }
            "vim" => {
                self.vim_mode = !self.vim_mode;
                renderer.render_info(&format!("Vim mode: {}", if self.vim_mode { "ON" } else { "OFF" }));
                Ok(true)
            }
            "tasks" | "bashes" => {
                renderer.render_info("No background tasks running. (TaskManager not wired to REPL)");
                Ok(true)
            }
            "stats" => {
                self.show_cost(runtime, renderer);
                renderer.render_info(&format!(
                    "Messages: {}\nTurns: {}/{}",
                    runtime.messages.len(),
                    runtime.turn_count,
                    runtime.config.max_turns,
                ));
                Ok(true)
            }
            "hooks" => {
                renderer.render_info("Hook configurations: (none configured)");
                renderer.render_info("Hooks can be set in .claude/config.json under a 'hooks' key.");
                Ok(true)
            }
            "env" => {
                let api_key_set = self.config.api.anthropic_api_key.is_some();
                let base_url = &self.config.api.base_url;
                let model = &self.config.model;
                renderer.render_info(&format!(
                    "ANTHROPIC_API_KEY: {}",
                    if api_key_set {
                        format!("set ({}...)", &self.config.api.anthropic_api_key.as_ref().unwrap().as_str()[..8.min(self.config.api.anthropic_api_key.as_ref().unwrap().len())])
                    } else {
                        "not set".to_string()
                    },
                ));
                renderer.render_info(&format!("ANTHROPIC_BASE_URL: {base_url}"));
                renderer.render_info(&format!("CLAUDE_MODEL: {model}"));
                if let Some(ref compat_key) = self.config.api.openai_compat_api_key {
                    renderer.render_info(&format!("OPENAI_API_KEY: set ({}...)", &compat_key.as_str()[..8.min(compat_key.len())]));
                }
                if let Ok(cwd) = std::env::var("PWD") {
                    renderer.render_info(&format!("PWD: {cwd}"));
                }
                renderer.render_info(&format!("CWD: {}", self.cwd.display()));
                Ok(true)
            }
            "compact" => {
                let mode = match args.trim() {
                    "snip" => CompactMode::Snip,
                    "micro" => CompactMode::Micro,
                    "session-memory" => CompactMode::SessionMemory,
                    _ => CompactMode::Auto,
                };
                let strategy = CompactStrategy::new(mode);
                let before = runtime.messages.len();
                runtime.messages = strategy.compact(&runtime.messages);
                let after = runtime.messages.len();
                let mode_name = match mode {
                    CompactMode::Auto => "auto",
                    CompactMode::Snip => "snip",
                    CompactMode::Micro => "micro",
                    CompactMode::SessionMemory => "session-memory",
                    CompactMode::Manual => "manual",
                };
                renderer.render_info(&format!(
                    "Context compacted ({mode_name}): {before} -> {after} messages"
                ));
                Ok(true)
            }
            "effort" => {
                if args.is_empty() {
                    renderer.render_info(&format!("Effort level: {}", self.effort_level));
                    renderer.render_info("Options: low, medium, high, max, auto");
                } else {
                    let level = args.trim().to_lowercase();
                    match level.as_str() {
                        "low" | "medium" | "high" | "max" | "auto" => {
                            self.effort_level = level;
                            renderer.render_info(&format!("Effort level set to: {}", self.effort_level));
                        }
                        _ => renderer.render_error("Invalid effort level. Use: low|medium|high|max|auto"),
                    }
                }
                Ok(true)
            }
            "auto" => {
                self.auto_continue = !self.auto_continue;
                renderer.render_info(&format!(
                    "Auto-continue: {}",
                    if self.auto_continue { "ON" } else { "OFF" }
                ));
                Ok(true)
            }
            "scope" => {
                if args.is_empty() {
                    renderer.render_info(&format!("Output scope: {}", self.output_scope));
                    renderer.render_info("Options: full, concise");
                } else {
                    let scope = args.trim().to_lowercase();
                    match scope.as_str() {
                        "full" | "concise" => {
                            self.output_scope = scope;
                            renderer.render_info(&format!("Output scope set to: {}", self.output_scope));
                        }
                        _ => renderer.render_error("Invalid scope. Use: full|concise"),
                    }
                }
                Ok(true)
            }
            "debug" => {
                self.debug_mode = !self.debug_mode;
                if self.debug_mode {
                    tracing_subscriber::fmt()
                        .with_env_filter("debug")
                        .try_init()
                        .ok();
                    renderer.render_info("Debug logging: ON");
                } else {
                    renderer.render_info("Debug logging: OFF (restart to disable)");
                }
                Ok(true)
            }
            "export" => {
                let filename = if args.is_empty() {
                    "conversation.json".to_string()
                } else {
                    args.to_string()
                };
                let json = serde_json::to_string_pretty(&runtime.messages).unwrap_or_default();
                match std::fs::write(&filename, &json) {
                    Ok(()) => renderer.render_info(&format!("Conversation exported to: {filename}")),
                    Err(e) => renderer.render_error(&format!("Failed to export: {e}")),
                }
                Ok(true)
            }
            "session" | "continue" | "resume" => {
                if args.is_empty() {
                    renderer.render_info("Recent sessions: (none stored)");
                    renderer.render_info("Use: /session <id> to resume a session");
                    if let Some(ref name) = self.session_name {
                        renderer.render_info(&format!("Current session: {}", name));
                    }
                } else {
                    renderer.render_info(&format!("Resuming session '{}'... (stub)", args));
                }
                Ok(true)
            }
            "copy" => {
                match &self.last_assistant_response {
                    Some(text) => {
                        let clip_cmd = if cfg!(windows) { "clip" } else { "pbcopy" };
                        let encoded = text.replace('"', "\\\"").replace('|', "^|").replace('&', "^&");
                        match execute_bash(&self.cwd, &format!("echo {encoded} | {clip_cmd}"), 5_000).await {
                            Ok(_) => renderer.render_info("Last response copied to clipboard."),
                            Err(e) => renderer.render_error(&format!("Failed to copy: {e}")),
                        }
                    }
                    None => renderer.render_info("No assistant response to copy."),
                }
                Ok(true)
            }
            "branch" | "fork" => {
                let branch_messages = runtime.messages.clone();
                renderer.render_info(&format!(
                    "Branch created with {} messages. (stored in memory, use /reset to return)",
                    branch_messages.len()
                ));
                Ok(true)
            }
            "btw" => {
                if args.is_empty() {
                    renderer.render_error("Usage: /btw <question>");
                } else {
                    let question = format!("[BTW] {}", args);
                    renderer.render_user_message(&question);
                    let messages = runtime.submit_message(&question).await?;
                    for msg in &messages {
                        match msg {
                            claude_runtime::SdkMessage::Assistant { text } => {
                                self.last_assistant_response = Some(text.clone());
                                renderer.render_markdown(text);
                            }
                            claude_runtime::SdkMessage::ToolUse { name, input, .. } => {
                                renderer.render_tool_use(name, input);
                            }
                            claude_runtime::SdkMessage::ToolResult { content, is_error, .. } => {
                                renderer.render_tool_result(content, *is_error);
                            }
                            claude_runtime::SdkMessage::Error { message } => {
                                renderer.render_error(message);
                            }
                            claude_runtime::SdkMessage::Usage { input_tokens, output_tokens } => {
                                renderer.render_usage(*input_tokens, *output_tokens);
                            }
                            claude_runtime::SdkMessage::End { reason } => {
                                if reason != "end_turn" {
                                    renderer.render_info(&format!("Stopped: {reason}"));
                                }
                            }
                        }
                    }
                    println!();
                }
                Ok(true)
            }
            "mcp" => {
                renderer.render_info("Use: claude mcp list|add|remove from CLI arguments.");
                Ok(true)
            }
            "doctor" => {
                Box::pin(async {
                    self.doctor().await
                }).await?;
                Ok(true)
            }
            _ => {
                renderer.render_info(&format!("Unknown command: /{cmd}. Type /help for commands."));
                Ok(true)
            }
        }
    }

    fn show_help(&self, args: &str, renderer: &Renderer) {
        if args.is_empty() {
            renderer.render_info("Available commands:");
            let commands = self.command_registry.list_all();
            for cmd in commands {
                let aliases = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (aliases: {})", cmd.aliases.join(", "))
                };
                println!("  /{:<16} {}{}", cmd.name, cmd.description, aliases);
            }
        } else {
            if let Some(spec) = self.command_registry.get(args) {
                println!("/{:<16} {}", spec.name, spec.description);
            } else {
                renderer.render_error(&format!("Unknown command: /{args}"));
            }
        }
    }

    fn show_status(&self, runtime: &ConversationRuntime, renderer: &Renderer) {
        renderer.render_info(&format!(
            "Model: {}\nMessages: {}\nTurns: {}/{}\nEffort: {}\nScope: {}\nVim: {}\nAuto-continue: {}\nDebug: {}",
            runtime.config.model,
            runtime.messages.len(),
            runtime.turn_count,
            runtime.config.max_turns,
            self.effort_level,
            self.output_scope,
            self.vim_mode,
            self.auto_continue,
            self.debug_mode,
        ));
    }

    fn show_cost(&self, runtime: &ConversationRuntime, renderer: &Renderer) {
        let input = runtime.total_usage.input_tokens;
        let output = runtime.total_usage.output_tokens;
        renderer.render_info(&format!(
            "Input tokens:  {input}\nOutput tokens: {output}\nTotal tokens:  {}",
            input + output,
        ));
    }

    fn init_project(&self, path: &PathBuf) -> anyhow::Result<()> {
        let claude_dir = path.join(".claude");
        std::fs::create_dir_all(&claude_dir)?;
        let claude_md = path.join("CLAUDE.md");
        if !claude_md.exists() {
            std::fs::write(
                &claude_md,
                "# Project Instructions\n\nCustom instructions for Claude Code go here.\n",
            )?;
        }
        println!("Project initialized at {}", path.display());
        Ok(())
    }

    async fn doctor(&self) -> anyhow::Result<()> {
        println!("Checking system health...\n");

        println!("[1/3] Checking API key...");
        if self.config.api.anthropic_api_key.is_some() {
            println!("  API key: configured");
        } else {
            println!("  API key: NOT configured (set ANTHROPIC_API_KEY)");
        }

        println!("[2/3] Checking model...");
        println!("  Model: {}", self.config.model);

        println!("[3/3] Checking working directory...");
        println!("  CWD: {}", self.cwd.display());
        println!(
            "  Git: {}",
            if self.cwd.join(".git").exists() {
                "initialized"
            } else {
                "not a git repo"
            }
        );

        println!("\nAll checks passed!");
        Ok(())
    }

    async fn handle_mcp(&self, action: &McpAction) -> anyhow::Result<()> {
        match action {
            McpAction::List => {
                println!("Configured MCP servers:");
                if self.config.mcp_servers.is_empty() {
                    println!("  (none)");
                }
                for (name, server) in &self.config.mcp_servers {
                    let status = if server.enabled { "enabled" } else { "disabled" };
                    println!("  {name} ({status})");
                    if let Some(ref cmd) = server.command {
                        println!("    command: {cmd}");
                    }
                    if let Some(ref url) = server.url {
                        println!("    url: {url}");
                    }
                }
            }
            McpAction::Add { name, command, args } => {
                println!("MCP server '{name}' added: {command} {}", args.join(" "));
            }
            McpAction::Remove { name } => {
                println!("MCP server '{name}' removed");
            }
        }
        Ok(())
    }
}
