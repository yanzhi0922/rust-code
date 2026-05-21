mod agents;
mod cli;
mod conversation;
mod conversation_backend;
mod doctor;
mod extract_memories;
mod headless;
mod hooks;
mod interactive;
mod mcp_cli;
mod memory_file_detection;
mod plugins;
mod query_engine_compat;
mod remote;
mod repl_hook_runtime;
mod review_cli;
#[allow(dead_code)]
mod runtime_hooks;
mod session_file_access;
mod session_memory_runtime;
mod sessions;
mod skills_cli;
mod status;
mod tasks_cli;
mod updater;
mod worktree_cli;

use std::{
    fs,
    future::Future,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    pin::Pin,
};

use anyhow::{Context, Result, anyhow};
use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
use claude_core::{InputFormat, OutputFormat, PermissionMode};
use claude_session::SessionStore;
use claude_telemetry::install_tracing;
use claude_tools::mcp_runtime::runtime_mcp_policy_entries;
use claude_tools::shell::ShellExecutionPolicy;
use claude_tools::{ToolRuntimePolicy, configure_tool_runtime_policy};
use uuid::Uuid;

use agents::run_agents;
use clap::Parser;
use cli::{Cli, Commands, SettingSourceArgValue};
use conversation::{
    reapply_cli_overrides, restore_session_context, run_first_run_wizard, run_migrate,
    run_oneshot_text,
};
use doctor::run_doctor;
use extract_memories::drain_pending_extractions;
use headless::{
    run_headless, run_headless_json_print, run_headless_stream_json_print, run_headless_text_print,
    should_run_headless,
};
use hooks::run_hooks;
use interactive::run_interactive_shell;
use mcp_cli::run_mcp;
use plugins::run_plugins;
use remote::run_remote;
use review_cli::run_review;
use sessions::{run_export, run_sessions};
use skills_cli::run_skills;
use status::run_status;
use tasks_cli::run_tasks;
use worktree_cli::run_worktree;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedPromptOverrides {
    pub(crate) system_prompt: Option<String>,
    pub(crate) append_system_prompt: Option<String>,
}

type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + 'a>>;

fn main() -> Result<()> {
    std::thread::Builder::new()
        .name("remote-code-main".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(run_app)?
        .join()
        .map_err(|panic| {
            if let Some(message) = panic.downcast_ref::<&str>() {
                anyhow!("remote-code main thread panicked: {message}")
            } else if let Some(message) = panic.downcast_ref::<String>() {
                anyhow!("remote-code main thread panicked: {message}")
            } else {
                anyhow!("remote-code main thread panicked")
            }
        })?
}

#[tokio::main]
async fn run_app() -> Result<()> {
    install_tracing("remote_code_rust", false)?;
    let permission_mode_explicit = permission_mode_override_was_explicit();
    let cli = Cli::parse();
    validate_cli_mode(&cli)?;
    let prompt_overrides = resolve_cli_prompt_overrides(&cli)?;
    let structured_output_schema = parse_json_schema_arg(cli.json_schema.as_deref())?;
    let mcp_config_args = resolve_mcp_config_args(&cli.mcp_config)?;
    let mcp_config_paths = mcp_config_args.paths.clone();
    let effective_permission_mode = effective_permission_mode_from_cli(&cli);

    let resume_session = resolve_resume_session(&cli, &prompt_overrides)?;
    let overrides = ProviderOverrides {
        provider: cli.provider.clone(),
        base_url: cli.base_url.clone(),
        api_key: cli.api_key.clone(),
        model: cli.model.clone(),
        protocol: cli.protocol,
    };
    let mut config = load_runtime_config(
        cli.cwd.clone(),
        cli.profile_dir.clone(),
        resume_session,
        effective_permission_mode,
        cli.input_format,
        cli.output_format,
        cli.print_mode,
        cli.verbose,
        cli.replay_user_messages,
        cli.include_partial_messages,
        cli.max_turns,
        overrides,
        runtime_overrides_from_cli(
            &cli,
            &prompt_overrides,
            structured_output_schema.clone(),
            mcp_config_paths.clone(),
        ),
    )?;
    let store = SessionStore::open(config.paths.clone())?;
    if resume_session.is_some() {
        restore_session_context(&store, &mut config)?;
        reapply_cli_overrides(
            &cli,
            &prompt_overrides,
            &mut config,
            permission_mode_explicit,
        );
        if cli.dangerously_skip_permissions {
            config.permission_mode = PermissionMode::BypassPermissions;
        }
    }
    hydrate_api_key_helper(&mut config).await?;
    configure_runtime_policy(&config)?;
    if cli.show_setting_sources && !should_run_headless(&config) {
        print_setting_sources(&config);
    }

    // Launch the first-run wizard if no API key or settings are detected.
    // Only runs for interactive sessions (no subcommand or Resume without prompt).
    if should_run_first_run_wizard(&cli) {
        run_first_run_wizard(&mut config)?;
    }

    let prompt_parts = cli.prompt;
    let command = cli.command;
    let result = dispatch_command(command, prompt_parts, &mut config, &store).await;
    drain_pending_extractions(std::time::Duration::from_secs(60)).await;
    cleanup_temporary_mcp_config_paths(&mcp_config_args.temporary_paths);
    result
}

async fn hydrate_api_key_helper(config: &mut claude_config::RuntimeConfig) -> Result<()> {
    if config.provider.api_key.is_some() {
        return Ok(());
    }
    if !config.print_mode
        && matches!(
            config.api_key_helper_source,
            Some(SettingSource::Project | SettingSource::Local)
        )
    {
        return Ok(());
    }
    let Some(helper) = config.api_key_helper.as_deref() else {
        return Ok(());
    };
    let helper = helper.trim();
    if helper.is_empty() {
        return Ok(());
    }

    let result = claude_auth::execute_api_key_helper_cached(helper).await?;
    config.provider.api_key = Some(result.key);
    config.auth_source = Some("apiKeyHelper".to_owned());
    Ok(())
}

fn dispatch_command<'a>(
    command: Option<Commands>,
    prompt_parts: Vec<String>,
    config: &'a mut claude_config::RuntimeConfig,
    store: &'a SessionStore,
) -> CommandFuture<'a> {
    match command {
        Some(Commands::Doctor(args)) => Box::pin(async move { run_doctor(config, args).await }),
        Some(Commands::Status(args)) => Box::pin(async move { run_status(config, store, args) }),
        Some(Commands::Hooks { command }) => {
            Box::pin(async move { run_hooks(config, command).await })
        }
        Some(Commands::Remote { command }) => Box::pin(async move { run_remote(command).await }),
        Some(Commands::Sessions { command }) => {
            Box::pin(async move { run_sessions(store, command) })
        }
        Some(Commands::Review(args)) => Box::pin(async move { run_review(config, args) }),
        Some(Commands::Worktree { command }) => {
            Box::pin(async move { run_worktree(config, command) })
        }
        Some(Commands::Tasks { command }) => Box::pin(async move { run_tasks(config, command) }),
        Some(Commands::Export(args)) => Box::pin(async move { run_export(store, args) }),
        Some(Commands::Agents { command }) => {
            Box::pin(async move { run_agents(config, command).await })
        }
        Some(Commands::Plugins { command }) => {
            Box::pin(async move { run_plugins(config, command).await })
        }
        Some(Commands::Mcp { command }) => Box::pin(async move { run_mcp(config, command).await }),
        Some(Commands::Skills { command }) => Box::pin(async move { run_skills(config, command) }),
        Some(Commands::Migrate { command }) => {
            Box::pin(async move { run_migrate(config, command) })
        }
        Some(Commands::Resume(args)) => Box::pin(async move {
            run_session_entry(config, store, resolve_prompt_input(config, args.prompt)?).await
        }),
        Some(Commands::Tui) => {
            Box::pin(async move { claude_tui::run_tui_app(config.clone(), store).await })
        }
        Some(Commands::Ssh(args)) => Box::pin(async move { run_ssh(args).await }),
        Some(Commands::Update { command }) => Box::pin(async move {
            use cli::UpdateCommand;
            match command {
                UpdateCommand::Check => updater::run_check().await,
                UpdateCommand::Run => updater::run_update().await,
            }
        }),
        None => Box::pin(async move {
            run_session_entry(config, store, resolve_prompt_input(config, prompt_parts)?).await
        }),
    }
}

async fn run_session_entry(
    config: &mut claude_config::RuntimeConfig,
    store: &SessionStore,
    prompt: Option<String>,
) -> Result<()> {
    if should_run_headless(config) {
        return run_headless(config, prompt).await;
    }
    if config.print_mode {
        let prompt = prompt.ok_or_else(|| {
            anyhow!(
                "Input must be provided either through stdin or as a prompt argument when using --print"
            )
        })?;
        return match config.output_format {
            OutputFormat::Text => run_headless_text_print(config, store, prompt).await,
            OutputFormat::Json => run_headless_json_print(config, store, prompt).await,
            OutputFormat::StreamJson => run_headless_stream_json_print(config, store, prompt).await,
        };
    }
    if let Some(prompt) = prompt {
        run_oneshot_text(config, store, prompt).await
    } else {
        run_interactive_shell(config.clone(), store).await
    }
}

fn validate_cli_mode(cli: &Cli) -> Result<()> {
    if matches!(cli.input_format, InputFormat::StreamJson)
        && !matches!(cli.output_format, OutputFormat::StreamJson)
    {
        return Err(anyhow!(
            "--input-format=stream-json requires --output-format=stream-json"
        ));
    }

    if cli.replay_user_messages
        && !(matches!(cli.input_format, InputFormat::StreamJson)
            && matches!(cli.output_format, OutputFormat::StreamJson))
    {
        return Err(anyhow!(
            "--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json"
        ));
    }

    if cli.include_partial_messages
        && !(cli.print_mode && matches!(cli.output_format, OutputFormat::StreamJson))
    {
        return Err(anyhow!(
            "--include-partial-messages requires --print and --output-format=stream-json"
        ));
    }

    if cli.print_mode && matches!(cli.output_format, OutputFormat::StreamJson) && !cli.verbose {
        return Err(anyhow!(
            "When using --print, --output-format=stream-json requires --verbose"
        ));
    }

    if cli.brief && cli.no_brief {
        return Err(anyhow!("--brief and --no-brief cannot be used together"));
    }

    if cli.proactive && cli.no_proactive {
        return Err(anyhow!(
            "--proactive and --no-proactive cannot be used together"
        ));
    }

    if cli.permission_prompt_tool.is_some()
        && !(cli.print_mode && matches!(cli.output_format, OutputFormat::StreamJson))
    {
        return Err(anyhow!(
            "--permission-prompt-tool requires --print and --output-format=stream-json"
        ));
    }

    if !cli.print_mode
        && !matches!(cli.input_format, InputFormat::StreamJson)
        && !matches!(cli.output_format, OutputFormat::Text)
    {
        return Err(anyhow!(
            "--output-format=json and --output-format=stream-json can only be used with --print"
        ));
    }

    Ok(())
}

fn runtime_overrides_from_cli(
    cli: &Cli,
    prompt_overrides: &ResolvedPromptOverrides,
    structured_output_schema: Option<serde_json::Value>,
    mcp_config_paths: Vec<PathBuf>,
) -> RuntimeOverrides {
    RuntimeOverrides {
        session_name: cli.name.clone(),
        system_prompt: prompt_overrides.system_prompt.clone(),
        append_system_prompt: prompt_overrides.append_system_prompt.clone(),
        settings_files: cli.settings_files.clone(),
        show_setting_sources: cli.show_setting_sources,
        allowed_setting_sources: setting_sources_from_cli(&cli.setting_sources),
        allowed_tools: runtime_allowed_tools_from_cli(cli),
        disallowed_tools: normalize_cli_tool_values(&cli.disallowed_tools),
        structured_output_schema,
        mcp_config_paths,
        strict_mcp_config: cli.strict_mcp_config || cli.bare,
        effort: normalize_cli_optional_string(cli.effort.as_deref()),
        fallback_model: normalize_cli_optional_string(cli.fallback_model.as_deref()),
        output_style: normalize_cli_optional_string(cli.output_style.as_deref()),
        language: normalize_cli_optional_string(cli.language.as_deref()),
        brief_enabled: bool_override(cli.brief, cli.no_brief),
        proactive_active: bool_override(cli.proactive, cli.no_proactive),
    }
}

fn runtime_allowed_tools_from_cli(cli: &Cli) -> Vec<String> {
    let mut tools = normalize_cli_tool_values(&cli.allowed_tools);
    let requested_tools = normalize_cli_tool_values(&cli.tools);
    if requested_tools
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case("default"))
    {
        return tools;
    }
    if cli.tools.iter().any(|tool| tool.trim().is_empty()) {
        return Vec::new();
    }
    tools.extend(requested_tools);
    tools
}

fn effective_permission_mode_from_cli(cli: &Cli) -> PermissionMode {
    if cli.dangerously_skip_permissions {
        PermissionMode::BypassPermissions
    } else {
        cli.permission_mode
    }
}

fn should_run_first_run_wizard(cli: &Cli) -> bool {
    !cli.print_mode
        && (cli.command.is_none()
            || matches!(&cli.command, Some(Commands::Resume(_)) if cli.prompt.is_empty()))
}

fn bool_override(enabled: bool, disabled: bool) -> Option<bool> {
    if enabled {
        Some(true)
    } else if disabled {
        Some(false)
    } else {
        None
    }
}

fn normalize_cli_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_resume_session(
    cli: &Cli,
    prompt_overrides: &ResolvedPromptOverrides,
) -> Result<Option<Uuid>> {
    if cli.session_id.is_some() && cli.resume.is_some() {
        return Err(anyhow!("--session-id and --resume cannot be used together"));
    }
    if cli.r#continue && cli.resume.is_some() {
        return Err(anyhow!("--continue and --resume cannot be used together"));
    }

    match &cli.command {
        Some(Commands::Resume(args)) => Ok(Some(args.session_id)),
        _ => {
            if let Some(session_id) = cli.session_id {
                return Ok(Some(session_id));
            }
            if let Some(raw_resume) = cli.resume.as_ref().and_then(|value| value.as_ref()) {
                return parse_resume_session_id(raw_resume).map(Some);
            }
            if !cli.r#continue && cli.resume.is_none() {
                return Ok(None);
            }
            let config = load_runtime_config(
                cli.cwd.clone(),
                cli.profile_dir.clone(),
                None,
                effective_permission_mode_from_cli(cli),
                cli.input_format,
                cli.output_format,
                cli.print_mode,
                cli.verbose,
                cli.replay_user_messages,
                cli.include_partial_messages,
                cli.max_turns,
                ProviderOverrides::default(),
                runtime_overrides_from_cli(
                    cli,
                    prompt_overrides,
                    parse_json_schema_arg(cli.json_schema.as_deref())?,
                    Vec::new(),
                ),
            )?;
            let store = SessionStore::open(config.paths.clone())?;
            Ok(store
                .latest_active_session()?
                .map(|summary| summary.session_id))
        }
    }
}

fn parse_resume_session_id(raw: &str) -> Result<Uuid> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("--resume session id cannot be empty"));
    }
    if trimmed.to_ascii_lowercase().ends_with(".jsonl") {
        return Err(anyhow!(
            "--resume JSONL transcript paths are not supported yet; use a session UUID"
        ));
    }
    Uuid::parse_str(trimmed).with_context(|| format!("invalid --resume session id `{trimmed}`"))
}

fn setting_sources_from_cli(values: &[SettingSourceArgValue]) -> Option<Vec<SettingSource>> {
    (!values.is_empty()).then(|| {
        values
            .iter()
            .map(|value| match value {
                SettingSourceArgValue::User => SettingSource::User,
                SettingSourceArgValue::Project => SettingSource::Project,
                SettingSourceArgValue::Local => SettingSource::Local,
            })
            .collect()
    })
}

fn parse_json_schema_arg(raw: Option<&str>) -> Result<Option<serde_json::Value>> {
    raw.map(|schema| {
        serde_json::from_str::<serde_json::Value>(schema)
            .map_err(|error| anyhow!("Invalid --json-schema: {error}"))
    })
    .transpose()
}

#[derive(Debug, Clone, Default)]
struct ResolvedMcpConfigArgs {
    paths: Vec<PathBuf>,
    temporary_paths: Vec<PathBuf>,
}

fn resolve_mcp_config_args(values: &[String]) -> Result<ResolvedMcpConfigArgs> {
    let mut resolved = ResolvedMcpConfigArgs::default();
    for value in values {
        let path = {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(anyhow!("--mcp-config cannot be empty"));
            }
            if trimmed.starts_with('{') {
                claude_mcp::McpConfig::from_json_str(trimmed)
                    .map_err(|error| anyhow!("Invalid --mcp-config JSON: {error}"))?;
                let mut path = std::env::temp_dir();
                path.push(format!("remote-code-mcp-{}.json", Uuid::new_v4()));
                fs::write(&path, trimmed)?;
                resolved.temporary_paths.push(path.clone());
                Ok(path)
            } else {
                resolve_cli_file_path(Path::new(trimmed))
            }
        }?;
        resolved.paths.push(path);
    }
    Ok(resolved)
}

fn cleanup_temporary_mcp_config_paths(paths: &[PathBuf]) {
    for path in paths {
        if let Err(error) = fs::remove_file(path) {
            tracing::debug!(
                "failed to remove temporary MCP config {}: {error}",
                path.display()
            );
        }
    }
}

fn normalize_cli_tool_values(values: &[String]) -> Vec<String> {
    let mut parsed = Vec::new();
    for value in values {
        let mut current = String::new();
        let mut paren_depth = 0usize;
        for ch in value.chars() {
            match ch {
                '(' => {
                    paren_depth += 1;
                    current.push(ch);
                }
                ')' => {
                    paren_depth = paren_depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' | ' ' if paren_depth == 0 => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parsed.push(trimmed.to_owned());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            parsed.push(trimmed.to_owned());
        }
    }
    parsed
}

fn configure_runtime_policy(config: &claude_config::RuntimeConfig) -> Result<()> {
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
        tasks_dir: Some(claude_swarm::team_helpers::claude_config_home_dir().join("tasks")),
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
        mcp_servers: runtime_mcp_policy_entries(config, &config.mcp_config_paths),
    })
}

fn print_setting_sources(config: &claude_config::RuntimeConfig) {
    println!("Setting sources:");
    if config.setting_sources.is_empty() {
        println!("  (defaults)");
    } else {
        for source in &config.setting_sources {
            println!("  {source}");
        }
    }
    println!(
        "Allowed setting sources: {}",
        if config.allowed_setting_sources.is_empty() {
            "(none)".to_owned()
        } else {
            config
                .allowed_setting_sources
                .iter()
                .map(|source| source.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "Settings files: {}",
        if config.settings_files.is_empty() {
            "(auto discovery only)".to_owned()
        } else {
            config
                .settings_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    if !config.cli_settings_files.is_empty() {
        println!(
            "Explicit settings mode: {}",
            config
                .cli_settings_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

async fn run_ssh(args: cli::SshArgs) -> Result<()> {
    use anyhow::Context as _;
    use std::process::Command as StdCommand;

    // Build the SSH command
    let mut cmd_args = Vec::new();

    // SSH config file
    if let Some(config) = &args.config {
        cmd_args.push("-F".to_owned());
        cmd_args.push(config.to_string_lossy().to_string());
    }

    // Port
    if args.port != 22 {
        cmd_args.push("-p".to_owned());
        cmd_args.push(args.port.to_string());
    }

    // Identity file
    if let Some(identity) = &args.identity {
        cmd_args.push("-i".to_owned());
        cmd_args.push(identity.to_string_lossy().to_string());
    }

    // Verbose
    if args.verbose {
        cmd_args.push("-v".to_owned());
    }

    // Agent forwarding
    if args.forward_agent {
        cmd_args.push("-A".to_owned());
    }

    // Local port forwarding
    for fwd in &args.local_forward {
        cmd_args.push("-L".to_owned());
        cmd_args.push(fwd.clone());
    }

    // Remote port forwarding
    for fwd in &args.remote_forward {
        cmd_args.push("-R".to_owned());
        cmd_args.push(fwd.clone());
    }

    // Connection timeout
    cmd_args.push("-o".to_owned());
    cmd_args.push(format!("ConnectTimeout={}", args.timeout));

    // Disable strict host key checking for convenience (can be overridden via config)
    cmd_args.push("-o".to_owned());
    cmd_args.push("StrictHostKeyChecking=accept-new".to_owned());

    // Build user@host
    let target = if let Some(user) = &args.user {
        format!("{user}@{}", args.host)
    } else {
        args.host.clone()
    };
    cmd_args.push(target);

    // Remote command
    if let Some(command) = &args.command {
        cmd_args.push(command.clone());
    } else {
        // Default: start remote-code on the remote host with any extra args.
        let mut remote_cmd = String::from("remote-code");
        for extra in &args.remote_args {
            remote_cmd.push(' ');
            remote_cmd.push_str(extra);
        }
        cmd_args.push(remote_cmd);
    }

    println!("Connecting via SSH: ssh {}", cmd_args.join(" "));

    let status = StdCommand::new("ssh")
        .args(&cmd_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("failed to execute ssh command — is ssh installed?")?;

    if !status.success() {
        if let Some(code) = status.code() {
            anyhow::bail!("SSH session exited with code {code}");
        } else {
            anyhow::bail!("SSH session terminated by signal");
        }
    }
    Ok(())
}

fn join_prompt(parts: Vec<String>) -> Option<String> {
    let prompt = parts.join(" ");
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn permission_mode_override_was_explicit() -> bool {
    std::env::var_os("REMOTE_CODE_PERMISSION_MODE").is_some()
        || std::env::args_os().skip(1).any(|arg| {
            let arg = arg.to_string_lossy();
            arg == "--permission-mode" || arg.starts_with("--permission-mode=")
        })
}

fn resolve_cli_prompt_overrides(cli: &Cli) -> Result<ResolvedPromptOverrides> {
    Ok(ResolvedPromptOverrides {
        system_prompt: resolve_cli_prompt_override(
            cli.system_prompt.clone(),
            cli.system_prompt_file.as_deref(),
            "--system-prompt",
            "--system-prompt-file",
            "System prompt file not found",
            "Error reading system prompt file",
        )?,
        append_system_prompt: resolve_cli_prompt_override(
            cli.append_system_prompt.clone(),
            cli.append_system_prompt_file.as_deref(),
            "--append-system-prompt",
            "--append-system-prompt-file",
            "Append system prompt file not found",
            "Error reading append system prompt file",
        )?,
    })
}

fn resolve_cli_prompt_override(
    inline_value: Option<String>,
    file_path: Option<&Path>,
    inline_flag: &str,
    file_flag: &str,
    missing_prefix: &str,
    read_prefix: &str,
) -> Result<Option<String>> {
    if inline_value.is_some() && file_path.is_some() {
        return Err(anyhow!(
            "Cannot use both {inline_flag} and {file_flag}. Please use only one."
        ));
    }

    let Some(file_path) = file_path else {
        return Ok(inline_value);
    };

    let resolved_path = resolve_cli_file_path(file_path)?;
    match fs::read_to_string(&resolved_path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(anyhow!("{missing_prefix}: {}", resolved_path.display()))
        }
        Err(error) => Err(anyhow!("{read_prefix}: {error}")),
    }
}

fn resolve_cli_file_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn resolve_prompt_input(
    config: &claude_config::RuntimeConfig,
    parts: Vec<String>,
) -> Result<Option<String>> {
    let prompt = join_prompt(parts);
    if prompt.is_some() || should_run_headless(config) {
        return Ok(prompt);
    }
    if io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut stdin = String::new();
    io::stdin().read_to_string(&mut stdin)?;
    Ok(normalize_prompt(stdin))
}

fn normalize_prompt(prompt: String) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use clap::Parser;
    use claude_control_plane::{
        ArtifactCreateRequest, ArtifactRecord, ControlPlaneMeta as RemoteControlPlaneMeta,
        SessionRecord as RemoteSessionRecord, SessionState as RemoteSessionState,
        SessionStateUpdateRequest, TimelineEvent as RemoteTimelineEvent,
        TimelineEventDetail as RemoteTimelineEventDetail,
    };
    use claude_runner::{
        ApprovalCreateRequest as SharedApprovalCreateRequest,
        ApprovalRequestRecord as RemoteApprovalRecord, ListResponse as RemoteListResponse,
        RunnerSnapshot as RemoteRunnerSnapshot,
    };
    use serial_test::serial;

    use crate::agents::{default_task_for_objective, parse_agent_spec, parse_task_spec};
    use crate::cli::{McpCallArgs, McpListArgs, RemoteEventKindValue};
    use crate::mcp_cli::{build_mcp_call_output, build_mcp_list_output, parse_mcp_call_arguments};
    use crate::remote::{
        RemoteFollowControl, StateLabel, build_remote_http_url, build_remote_ws_request_with_token,
        build_remote_ws_url, default_artifact_file_name, default_artifact_name,
        encode_remote_path_segment, follow_remote_timeline_stream,
        is_terminal_remote_session_state, merge_follow_sequence, normalize_remote_base_url,
        parse_repeated_key_value_args, remote_approval_path, remote_approvals_path,
        remote_approvals_stream_path, remote_artifact_download_path, remote_artifacts_path,
        remote_event_reaches_terminal_session_state, remote_events_path, remote_events_stream_path,
        remote_get_bytes, remote_get_json, remote_post_json, remote_runner_path,
        remote_session_commands_path, remote_session_state_path, remote_sessions_path,
    };
    use crate::{
        Cli, ResolvedPromptOverrides, cleanup_temporary_mcp_config_paths,
        effective_permission_mode_from_cli, hydrate_api_key_helper, normalize_cli_tool_values,
        parse_json_schema_arg, resolve_cli_prompt_overrides, resolve_mcp_config_args,
        resolve_resume_session, runtime_overrides_from_cli, should_run_first_run_wizard,
        validate_cli_mode,
    };

    use axum::{
        Router,
        extract::{
            Query, State,
            ws::{Message, WebSocketUpgrade},
        },
        response::IntoResponse,
        routing::get,
    };
    use chrono::{DateTime, Utc};
    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use claude_tools::mcp_runtime::{discover_runtime_mcp_servers, resolve_runtime_mcp_server};
    use futures::SinkExt;
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
        process::Command as ProcessCommand,
        sync::{Arc, Mutex as StdMutex},
        time::Duration,
    };
    use tempfile::tempdir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use uuid::Uuid;

    #[test]
    fn agent_spec_parser_extracts_paths_and_labels() {
        let agent = parse_agent_spec("runtime;implementer;src,crates;phase=local,os=windows")
            .unwrap_or_else(|error| panic!("failed to parse agent spec: {error}"));
        assert_eq!(agent.name, "runtime");
        assert_eq!(agent.role, "implementer");
        assert_eq!(agent.ownership_paths, vec!["src", "crates"]);
        assert_eq!(agent.labels.get("phase").map(String::as_str), Some("local"));
        assert_eq!(agent.labels.get("os").map(String::as_str), Some("windows"));
    }

    #[test]
    fn task_spec_parser_and_default_task_apply_budgets() {
        let task =
            parse_task_spec("Wire events;crates/rc-control-plane;phase=remote;Add websocket")
                .unwrap_or_else(|error| panic!("failed to parse task spec: {error}"));
        assert_eq!(task.title, "Wire events");
        assert_eq!(task.ownership_paths, vec!["crates/rc-control-plane"]);
        assert_eq!(
            task.required_labels.get("phase").map(String::as_str),
            Some("remote")
        );
        assert_eq!(task.description, "Add websocket");
        assert_eq!(task.budget.command_calls, 8);

        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let config = load_runtime_config(
            Some(tempdir.path().to_path_buf()),
            Some(tempdir.path().join(".remote-code-rust")),
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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));
        let default_task = default_task_for_objective("Ship the next slice", &config);
        assert!(default_task.description.contains("Ship the next slice"));
        assert_eq!(default_task.budget.edit_calls, 16);
    }

    #[test]
    fn normalize_remote_base_url_preserves_base_path() {
        let target = normalize_remote_base_url("http://127.0.0.1:8787/api/v1/")
            .unwrap_or_else(|error| panic!("base URL normalize failed: {error}"));
        assert_eq!(target, "http://127.0.0.1:8787/api/v1");
        assert_eq!(
            build_remote_http_url(&target, "sessions").unwrap_or_else(|error| panic!("{error}")),
            "http://127.0.0.1:8787/api/v1/sessions"
        );
    }

    #[test]
    fn build_remote_ws_url_switches_protocol_and_keeps_base_path() {
        let ws_url = build_remote_ws_url("https://example.com/control/", "/v1/events/stream")
            .unwrap_or_else(|error| panic!("ws URL build failed: {error}"));
        assert_eq!(ws_url, "wss://example.com/control/v1/events/stream");
    }

    #[test]
    fn build_remote_ws_request_prefers_authorization_header_over_query_token() {
        let request = build_remote_ws_request_with_token(
            "https://example.com/control/",
            "/v1/events/stream?after=42",
            Some("device-token"),
        )
        .unwrap_or_else(|error| panic!("ws request build failed: {error}"));

        assert_eq!(
            request.uri().to_string(),
            "wss://example.com/control/v1/events/stream?after=42"
        );
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer device-token")
        );
    }

    #[test]
    fn parse_repeated_key_value_args_collects_metadata() {
        let metadata = parse_repeated_key_value_args(
            "--meta",
            &["phase=remote".to_owned(), "owner=cli".to_owned()],
        )
        .unwrap_or_else(|error| panic!("metadata parse failed: {error}"));
        assert_eq!(metadata.get("phase").map(String::as_str), Some("remote"));
        assert_eq!(metadata.get("owner").map(String::as_str), Some("cli"));
    }

    #[test]
    fn artifact_default_names_use_path_parts_and_safe_fallbacks() {
        assert_eq!(
            default_artifact_name(Path::new("logs/transcript.json")),
            "transcript"
        );
        assert_eq!(
            default_artifact_file_name(Path::new("logs/transcript.json")),
            "transcript.json"
        );
        assert_eq!(default_artifact_name(Path::new("   ")), "artifact");
        assert_eq!(default_artifact_file_name(Path::new("   ")), "artifact.bin");
    }

    #[test]
    fn cli_validation_matches_headless_format_constraints() {
        let cli = Cli::parse_from([
            "remote-code",
            "--input-format",
            "stream-json",
            "--output-format",
            "json",
            "-p",
        ]);
        let error = validate_cli_mode(&cli).expect_err("stream input needs stream output");
        assert!(
            error
                .to_string()
                .contains("--input-format=stream-json requires --output-format=stream-json")
        );

        let cli = Cli::parse_from(["remote-code", "--replay-user-messages", "-p"]);
        let error = validate_cli_mode(&cli).expect_err("replay requires both stream formats");
        assert!(
            error
                .to_string()
                .contains("--replay-user-messages requires both")
        );

        let cli = Cli::parse_from([
            "remote-code",
            "--include-partial-messages",
            "--output-format",
            "json",
            "-p",
        ]);
        let error = validate_cli_mode(&cli).expect_err("partial messages need stream-json print");
        assert!(
            error
                .to_string()
                .contains("--include-partial-messages requires --print")
        );

        let cli = Cli::parse_from(["remote-code", "--output-format", "json"]);
        let error = validate_cli_mode(&cli).expect_err("json output needs print mode");
        assert!(
            error
                .to_string()
                .contains("--output-format=json and --output-format=stream-json")
        );

        let cli = Cli::parse_from([
            "remote-code",
            "-p",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "hello",
        ]);
        let error = validate_cli_mode(&cli).expect_err("stream-json print needs verbose");
        assert!(
            error
                .to_string()
                .contains("--output-format=stream-json requires --verbose")
        );

        let cli = Cli::parse_from([
            "remote-code",
            "-p",
            "--verbose",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "hello",
        ]);
        validate_cli_mode(&cli).expect("valid stream-json print mode");
    }

    #[test]
    fn cli_runtime_overrides_wire_reference_knobs() {
        let cli = Cli::parse_from([
            "remote-code",
            "--allowedTools",
            "Read",
            "--tools",
            "Edit,Bash(git:*)",
            "--effort",
            "high",
            "--fallback-model",
            "minimax-m2.7",
            "--output-style",
            "concise",
            "--language",
            "zh-CN",
            "--brief",
            "--no-proactive",
            "--strict-mcp-config",
            "hello",
        ]);
        let overrides = runtime_overrides_from_cli(
            &cli,
            &ResolvedPromptOverrides::default(),
            Some(serde_json::json!({"type": "object"})),
            vec![PathBuf::from("mcp.json")],
        );

        assert_eq!(overrides.allowed_tools, vec!["Read", "Edit", "Bash(git:*)"]);
        assert_eq!(overrides.effort.as_deref(), Some("high"));
        assert_eq!(overrides.fallback_model.as_deref(), Some("minimax-m2.7"));
        assert_eq!(overrides.output_style.as_deref(), Some("concise"));
        assert_eq!(overrides.language.as_deref(), Some("zh-CN"));
        assert_eq!(overrides.brief_enabled, Some(true));
        assert_eq!(overrides.proactive_active, Some(false));
        assert!(overrides.strict_mcp_config);
        assert_eq!(overrides.mcp_config_paths, vec![PathBuf::from("mcp.json")]);
        assert_eq!(
            overrides.structured_output_schema,
            Some(serde_json::json!({"type": "object"}))
        );
    }

    #[test]
    fn cli_tools_empty_disables_all_builtin_tools() {
        let cli = Cli::parse_from(["remote-code", "--tools", "", "hello"]);
        let overrides =
            runtime_overrides_from_cli(&cli, &ResolvedPromptOverrides::default(), None, Vec::new());

        assert!(overrides.allowed_tools.is_empty());
    }

    #[test]
    fn cli_dangerous_skip_permissions_maps_to_bypass_mode() {
        let cli = Cli::parse_from([
            "remote-code",
            "--permission-mode",
            "default",
            "--dangerously-skip-permissions",
            "hello",
        ]);

        assert_eq!(
            effective_permission_mode_from_cli(&cli),
            claude_core::PermissionMode::BypassPermissions
        );
    }

    #[tokio::test]
    #[serial]
    async fn api_key_helper_hydrates_runtime_provider_key() {
        claude_auth::clear_global_api_key_helper_cache();
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(&cwd).expect("workspace dir");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("settings.json"),
            r#"{
                "apiKeyHelper": "echo hydrated-helper-key",
                "provider": {
                    "name": "anthropic",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-sonnet-4-5"
                }
            }"#,
        )
        .expect("write settings");
        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
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
        .expect("config load failed");

        assert_eq!(config.auth_source.as_deref(), Some("apiKeyHelper"));
        assert!(config.provider.api_key.is_none());

        hydrate_api_key_helper(&mut config)
            .await
            .expect("helper should hydrate");

        assert_eq!(
            config.provider.api_key.as_deref(),
            Some("hydrated-helper-key")
        );
        assert_eq!(config.auth_source.as_deref(), Some("apiKeyHelper"));
        claude_auth::clear_global_api_key_helper_cache();
    }

    #[tokio::test]
    #[serial]
    async fn project_api_key_helper_is_not_hydrated_before_interactive_trust() {
        claude_auth::clear_global_api_key_helper_cache();
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            cwd.join(".remote-code").join("settings.json"),
            r#"{
                "apiKeyHelper": "echo should-not-run",
                "provider": {
                    "name": "anthropic",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-sonnet-4-5"
                }
            }"#,
        )
        .expect("write settings");
        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
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
        .expect("config load failed");

        assert_eq!(config.api_key_helper_source, Some(SettingSource::Project));

        hydrate_api_key_helper(&mut config)
            .await
            .expect("helper hydration should skip safely");

        assert!(config.provider.api_key.is_none());
        claude_auth::clear_global_api_key_helper_cache();
    }

    #[tokio::test]
    #[serial]
    async fn project_api_key_helper_runs_in_print_mode() {
        claude_auth::clear_global_api_key_helper_cache();
        let temp = tempdir().expect("tempdir should work");
        let cwd = temp.path().join("workspace");
        let profile_dir = temp.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code")).expect("workspace settings dir");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            cwd.join(".remote-code").join("settings.json"),
            r#"{
                "apiKeyHelper": "echo print-helper-key",
                "provider": {
                    "name": "anthropic",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-sonnet-4-5"
                }
            }"#,
        )
        .expect("write settings");
        let mut config = load_runtime_config(
            Some(cwd),
            Some(profile_dir),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            true,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config load failed");

        hydrate_api_key_helper(&mut config)
            .await
            .expect("print helper should hydrate");

        assert_eq!(config.provider.api_key.as_deref(), Some("print-helper-key"));
        claude_auth::clear_global_api_key_helper_cache();
    }

    #[test]
    fn cli_json_schema_and_mcp_config_values_are_resolved() {
        let schema = parse_json_schema_arg(Some(r#"{"type":"object"}"#))
            .expect("schema parses")
            .expect("schema present");
        assert_eq!(schema["type"], "object");
        assert!(parse_json_schema_arg(Some("{broken")).is_err());

        let temp = tempdir().expect("tempdir should work");
        let file = temp.path().join("mcp.json");
        fs::write(&file, r#"{"mcpServers":{"file-demo":{"command":"node"}}}"#).expect("write mcp");
        let resolved = resolve_mcp_config_args(&[
            file.display().to_string(),
            r#"{"mcpServers":{"inline-demo":{"command":"node"}}}"#.to_owned(),
        ])
        .expect("mcp configs should resolve");

        assert_eq!(resolved.paths.len(), 2);
        assert_eq!(resolved.paths[0], file);
        assert!(resolved.paths[1].exists());
        assert_eq!(resolved.temporary_paths, vec![resolved.paths[1].clone()]);
        cleanup_temporary_mcp_config_paths(&resolved.temporary_paths);
        assert!(!resolved.paths[1].exists());
    }

    #[test]
    fn cli_tool_values_accept_space_and_comma_separated_reference_form() {
        assert_eq!(
            normalize_cli_tool_values(&[
                "Bash(git commit:*)".to_owned(),
                "Edit,Read".to_owned(),
                "Bash(python -c \"print(1, 2)\")".to_owned(),
            ]),
            vec![
                "Bash(git commit:*)".to_owned(),
                "Edit".to_owned(),
                "Read".to_owned(),
                "Bash(python -c \"print(1, 2)\")".to_owned(),
            ]
        );
    }

    #[test]
    fn cli_resume_flag_accepts_upstream_uuid_shape() {
        let session_id = Uuid::nil();
        let session_id_arg = session_id.to_string();
        let cli = Cli::parse_from([
            "remote-code",
            "--resume",
            session_id_arg.as_str(),
            "continue the task",
        ]);

        let resolved = resolve_resume_session(&cli, &ResolvedPromptOverrides::default())
            .expect("resume id should parse");

        assert_eq!(resolved, Some(session_id));
    }

    #[test]
    fn first_run_wizard_never_runs_for_print_resume() {
        let session_id_arg = Uuid::nil().to_string();
        let cli = Cli::parse_from(["remote-code", "-p", "resume", session_id_arg.as_str()]);

        assert!(!should_run_first_run_wizard(&cli));
    }

    #[test]
    fn remote_approvals_path_supports_global_runner_and_session_scopes() {
        assert_eq!(
            remote_approvals_path(None, None).unwrap_or_else(|error| panic!("{error}")),
            "/v1/approvals"
        );
        assert_eq!(
            remote_approvals_path(Some(Uuid::nil()), None)
                .unwrap_or_else(|error| panic!("{error}")),
            format!("/v1/sessions/{}/approvals", Uuid::nil())
        );
        assert_eq!(
            remote_approvals_path(None, Some("runner-a")).unwrap_or_else(|error| panic!("{error}")),
            "/v1/runners/runner-a/approvals"
        );
        assert!(remote_approvals_path(Some(Uuid::nil()), Some("runner-a")).is_err());
    }

    #[test]
    fn remote_item_paths_encode_runner_segments_and_uuid_ids() {
        assert_eq!(
            remote_approval_path(Uuid::nil()),
            format!("/v1/approvals/{}", Uuid::nil())
        );
        assert_eq!(
            remote_runner_path("runner/a b"),
            "/v1/runners/runner%2Fa%20b"
        );
    }

    #[test]
    fn remote_approvals_stream_path_supports_scopes_and_after_query() {
        assert_eq!(
            remote_approvals_stream_path(None, None, Some(4))
                .unwrap_or_else(|error| panic!("{error}")),
            "/v1/approvals/stream?after=4"
        );
        assert_eq!(
            remote_approvals_stream_path(Some(Uuid::nil()), None, None)
                .unwrap_or_else(|error| panic!("{error}")),
            format!("/v1/sessions/{}/approvals/stream", Uuid::nil())
        );
        assert_eq!(
            remote_approvals_stream_path(None, Some("runner/a"), Some(8))
                .unwrap_or_else(|error| panic!("{error}")),
            "/v1/runners/runner%2Fa/approvals/stream?after=8"
        );
        assert!(remote_approvals_stream_path(Some(Uuid::nil()), Some("runner-a"), None).is_err());
    }

    #[test]
    fn remote_artifacts_path_supports_global_and_session_scopes() {
        assert_eq!(
            remote_artifacts_path(None, None).unwrap_or_else(|error| panic!("{error}")),
            "/v1/artifacts"
        );
        assert_eq!(
            remote_artifacts_path(Some(Uuid::nil()), None)
                .unwrap_or_else(|error| panic!("{error}")),
            format!("/v1/sessions/{}/artifacts", Uuid::nil())
        );
        assert_eq!(
            remote_artifacts_path(None, Some("runner/a")).unwrap_or_else(|error| panic!("{error}")),
            "/v1/runners/runner%2Fa/artifacts"
        );
        assert!(remote_artifacts_path(Some(Uuid::nil()), Some("runner-a")).is_err());
        assert_eq!(
            remote_artifact_download_path(Uuid::nil()),
            format!("/v1/artifacts/{}/download", Uuid::nil())
        );
    }

    #[test]
    fn remote_session_state_path_targets_session_control_endpoint() {
        assert_eq!(
            remote_session_state_path(Uuid::nil()),
            format!("/v1/sessions/{}/state", Uuid::nil())
        );
    }

    #[test]
    fn remote_session_commands_path_targets_session_command_endpoint() {
        assert_eq!(
            remote_session_commands_path(Uuid::nil()),
            format!("/v1/sessions/{}/commands", Uuid::nil())
        );
    }

    #[test]
    fn remote_sessions_path_supports_filters_and_runner_scope() {
        assert_eq!(remote_sessions_path(None, None, None), "/v1/sessions");
        assert_eq!(
            remote_sessions_path(
                Some("runner/a"),
                Some("default"),
                Some(RemoteSessionState::Running)
            ),
            "/v1/runners/runner%2Fa/sessions?workspace_id=default&state=running"
        );
    }

    #[test]
    fn default_artifact_helpers_fall_back_for_missing_names() {
        assert_eq!(default_artifact_name(Path::new("upload.log")), "upload");
        assert_eq!(
            default_artifact_file_name(Path::new("nested/report.json")),
            "report.json"
        );
        assert_eq!(default_artifact_name(Path::new("..")), "artifact");
        assert_eq!(default_artifact_file_name(Path::new("..")), "artifact.bin");
    }

    #[test]
    fn encode_remote_path_segment_escapes_reserved_bytes() {
        assert_eq!(encode_remote_path_segment("runner-a"), "runner-a");
        assert_eq!(
            encode_remote_path_segment("runner/a b?c"),
            "runner%2Fa%20b%3Fc"
        );
    }

    #[test]
    fn remote_events_path_builds_queries() {
        assert_eq!(
            remote_events_path(None, None, None, 20, None)
                .unwrap_or_else(|error| panic!("{error}")),
            "/v1/events?limit=20"
        );
        assert_eq!(
            remote_events_path(Some(Uuid::nil()), None, Some(41), 500, None)
                .unwrap_or_else(|error| panic!("{error}")),
            format!("/v1/sessions/{}/events?after=41&limit=200", Uuid::nil())
        );
        assert_eq!(
            remote_events_path(
                None,
                Some("runner/a"),
                Some(2),
                5,
                Some(RemoteEventKindValue::SessionCreated)
            )
            .unwrap_or_else(|error| panic!("{error}")),
            "/v1/runners/runner%2Fa/events?after=2&limit=5&kind=session_created"
        );
        assert!(remote_events_path(Some(Uuid::nil()), Some("runner-a"), None, 20, None).is_err());
    }

    #[test]
    fn remote_events_stream_path_appends_after_query() {
        assert_eq!(
            remote_events_stream_path(None, None, None, None)
                .unwrap_or_else(|error| panic!("{error}")),
            "/v1/events/stream"
        );
        assert_eq!(
            remote_events_stream_path(Some(Uuid::nil()), None, Some(41), None)
                .unwrap_or_else(|error| panic!("{error}")),
            format!("/v1/sessions/{}/events/stream?after=41", Uuid::nil())
        );
        assert_eq!(
            remote_events_stream_path(
                None,
                Some("runner/a"),
                Some(9),
                Some(RemoteEventKindValue::SessionCreated)
            )
            .unwrap_or_else(|error| panic!("{error}")),
            "/v1/runners/runner%2Fa/events/stream?after=9&kind=session_created"
        );
        assert!(
            remote_events_stream_path(Some(Uuid::nil()), Some("runner-a"), None, None).is_err()
        );
    }

    #[test]
    fn merge_follow_sequence_prefers_highest_seen_value() {
        assert_eq!(merge_follow_sequence(None, None), None);
        assert_eq!(merge_follow_sequence(Some(4), None), Some(4));
        assert_eq!(merge_follow_sequence(None, Some(6)), Some(6));
        assert_eq!(merge_follow_sequence(Some(4), Some(6)), Some(6));
    }

    #[test]
    fn terminal_remote_session_states_are_classified_correctly() {
        assert!(!is_terminal_remote_session_state(
            RemoteSessionState::Running
        ));
        assert!(is_terminal_remote_session_state(
            RemoteSessionState::Completed
        ));
        assert!(is_terminal_remote_session_state(RemoteSessionState::Failed));
        assert!(is_terminal_remote_session_state(
            RemoteSessionState::Cancelled
        ));
    }

    #[test]
    fn remote_events_detect_terminal_session_transitions() {
        let running_event = RemoteTimelineEvent {
            sequence: 1,
            recorded_at: DateTime::parse_from_rfc3339("2026-04-08T00:00:01Z")
                .unwrap_or_else(|error| panic!("time parse failed: {error}"))
                .with_timezone(&Utc),
            runner_id: Some("runner-a".to_owned()),
            session_id: Some(Uuid::nil()),
            detail: RemoteTimelineEventDetail::SessionStateChanged {
                previous_state: RemoteSessionState::Assigned,
                state: RemoteSessionState::Running,
            },
        };
        assert!(!remote_event_reaches_terminal_session_state(&running_event));

        let completed_event = RemoteTimelineEvent {
            sequence: 2,
            recorded_at: DateTime::parse_from_rfc3339("2026-04-08T00:00:02Z")
                .unwrap_or_else(|error| panic!("time parse failed: {error}"))
                .with_timezone(&Utc),
            runner_id: Some("runner-a".to_owned()),
            session_id: Some(Uuid::nil()),
            detail: RemoteTimelineEventDetail::SessionStateChanged {
                previous_state: RemoteSessionState::Running,
                state: RemoteSessionState::Completed,
            },
        };
        assert!(remote_event_reaches_terminal_session_state(
            &completed_event
        ));
    }

    #[tokio::test]
    async fn follow_remote_timeline_stream_reconnects_with_last_sequence() {
        #[derive(Clone)]
        struct FollowTestState {
            attempts: Arc<StdMutex<Vec<Option<u64>>>>,
        }

        #[derive(serde::Deserialize)]
        struct StreamQuery {
            after: Option<u64>,
        }

        async fn stream_events(
            ws: WebSocketUpgrade,
            Query(query): Query<StreamQuery>,
            State(state): State<FollowTestState>,
        ) -> impl IntoResponse {
            let attempt_index = {
                let mut attempts = state
                    .attempts
                    .lock()
                    .unwrap_or_else(|error| panic!("attempt lock failed: {error}"));
                attempts.push(query.after);
                attempts.len()
            };
            ws.on_upgrade(move |mut socket| async move {
                let event = match attempt_index {
                    1 => RemoteTimelineEvent {
                        sequence: 2,
                        recorded_at: DateTime::parse_from_rfc3339("2026-04-08T00:00:02Z")
                            .unwrap_or_else(|error| panic!("time parse failed: {error}"))
                            .with_timezone(&Utc),
                        runner_id: Some("runner-a".to_owned()),
                        session_id: Some(Uuid::nil()),
                        detail: RemoteTimelineEventDetail::SessionCreated {
                            workspace_id: "default".to_owned(),
                            owner_runner_id: Some("runner-a".to_owned()),
                            state: claude_control_plane::SessionState::Running,
                        },
                    },
                    _ => RemoteTimelineEvent {
                        sequence: 3,
                        recorded_at: DateTime::parse_from_rfc3339("2026-04-08T00:00:03Z")
                            .unwrap_or_else(|error| panic!("time parse failed: {error}"))
                            .with_timezone(&Utc),
                        runner_id: Some("runner-a".to_owned()),
                        session_id: Some(Uuid::nil()),
                        detail: RemoteTimelineEventDetail::SessionStateChanged {
                            previous_state: claude_control_plane::SessionState::Running,
                            state: claude_control_plane::SessionState::Completed,
                        },
                    },
                };
                let payload = serde_json::to_string(&event)
                    .unwrap_or_else(|error| panic!("serialize failed: {error}"));
                socket
                    .send(Message::Text(payload.into()))
                    .await
                    .unwrap_or_else(|error| panic!("ws send failed: {error}"));
                let _ = socket.close().await;
            })
        }

        let attempts = Arc::new(StdMutex::new(Vec::new()));
        let state = FollowTestState {
            attempts: Arc::clone(&attempts),
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listener bind failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr failed: {error}"));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/events/stream", get(stream_events))
                    .with_state(state),
            )
            .await
            .unwrap_or_else(|error| panic!("server failed: {error}"));
        });

        let received = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        follow_remote_timeline_stream(
            &format!("http://{address}"),
            Some(1),
            Duration::from_millis(20),
            |after| remote_events_stream_path(None, None, after, None),
            move |event| {
                received_clone
                    .lock()
                    .unwrap_or_else(|error| panic!("received lock failed: {error}"))
                    .push(event.sequence);
                if event.sequence >= 3 {
                    Ok(RemoteFollowControl::Stop)
                } else {
                    Ok(RemoteFollowControl::Continue)
                }
            },
        )
        .await
        .unwrap_or_else(|error| panic!("follow should succeed: {error}"));

        server.abort();
        assert_eq!(
            *attempts
                .lock()
                .unwrap_or_else(|error| panic!("attempt lock failed: {error}")),
            vec![Some(1), Some(2)]
        );
        assert_eq!(
            *received
                .lock()
                .unwrap_or_else(|error| panic!("received lock failed: {error}")),
            vec![2, 3]
        );
    }

    #[test]
    fn runtime_mcp_discovery_collects_cwd_profile_and_plugin_servers() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));

        fs::write(
            cwd.join("mcp.toml"),
            "[mcp_servers.local]\ncommand = \"python\"\n",
        )
        .unwrap_or_else(|error| panic!("cwd mcp write failed: {error}"));
        fs::write(
            profile.join("mcp.toml"),
            "[mcp_servers.profile]\nurl = \"https://example.com/mcp\"\n",
        )
        .unwrap_or_else(|error| panic!("profile mcp write failed: {error}"));

        let plugin_root = profile.join("plugins").join("example-plugin");
        fs::create_dir_all(plugin_root.join(".codex-plugin"))
            .unwrap_or_else(|error| panic!("plugin manifest dir create failed: {error}"));
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{
                "name": "example-plugin",
                "version": "0.1.0",
                "mcp": "mcp.toml"
            }"#,
        )
        .unwrap_or_else(|error| panic!("plugin manifest write failed: {error}"));
        fs::write(
            plugin_root.join("mcp.toml"),
            "[mcp_servers.plugin]\ncommand = \"python\"\n",
        )
        .unwrap_or_else(|error| panic!("plugin mcp write failed: {error}"));

        let config = load_runtime_config(
            Some(cwd.clone()),
            Some(profile.clone()),
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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));

        let discovery = discover_runtime_mcp_servers(&config, &[]);
        let names = discovery
            .servers
            .iter()
            .map(|entry| entry.server.name.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "local".to_owned(),
                "plugin".to_owned(),
                "profile".to_owned()
            ])
        );
        assert!(discovery.warnings.is_empty());
    }

    #[test]
    fn runtime_mcp_discovery_loads_explicit_config_paths() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let extra_dir = tempdir.path().join("custom");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));
        fs::create_dir_all(&extra_dir)
            .unwrap_or_else(|error| panic!("extra dir create failed: {error}"));
        fs::write(
            extra_dir.join("mcp.toml"),
            "[mcp_servers.explicit]\ncommand = \"python\"\n",
        )
        .unwrap_or_else(|error| panic!("extra mcp write failed: {error}"));

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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));

        let discovery = discover_runtime_mcp_servers(&config, &[extra_dir]);
        assert!(
            discovery
                .servers
                .iter()
                .any(|entry| entry.server.name == "explicit" && entry.origin_kind == "explicit")
        );
        assert!(discovery.warnings.is_empty());
    }

    #[tokio::test]
    async fn mcp_list_output_skips_disabled_servers_without_connecting() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));

        fs::write(
            profile.join("mcp.toml"),
            "[mcp_servers.disabled]\ncommand = \"python\"\nenabled = false\n",
        )
        .unwrap_or_else(|error| panic!("profile mcp write failed: {error}"));

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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));

        let output = build_mcp_list_output(
            &config,
            &McpListArgs {
                connect: true,
                json: false,
                servers: Vec::new(),
                include_disabled: false,
                config_paths: Vec::new(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("mcp list output build failed: {error}"));

        assert_eq!(output.servers.len(), 1);
        let live = output.servers[0]
            .live
            .as_ref()
            .unwrap_or_else(|| panic!("expected live inspection metadata"));
        assert_eq!(live.status, "disabled");
        assert!(
            live.error
                .as_deref()
                .unwrap_or_default()
                .contains("include-disabled")
        );
    }

    #[test]
    fn parse_mcp_call_arguments_merges_json_and_key_value_overrides() {
        let parsed = parse_mcp_call_arguments(&McpCallArgs {
            server: "mock".to_owned(),
            tool: "search".to_owned(),
            json: false,
            include_disabled: false,
            args: vec![
                "query=rust".to_owned(),
                "count=3".to_owned(),
                "exact=true".to_owned(),
            ],
            args_json: Some(r#"{"scope":"docs","count":1}"#.to_owned()),
            config_paths: Vec::new(),
        })
        .unwrap_or_else(|error| panic!("argument parse failed: {error}"));

        assert_eq!(
            parsed,
            serde_json::json!({
                "scope": "docs",
                "query": "rust",
                "count": 3,
                "exact": true
            })
        );
    }

    #[test]
    fn resolve_runtime_mcp_server_prefers_higher_precedence_source() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));

        fs::write(
            cwd.join("mcp.toml"),
            "[mcp_servers.shared]\ncommand = \"python\"\n",
        )
        .unwrap_or_else(|error| panic!("cwd mcp write failed: {error}"));
        fs::write(
            profile.join("mcp.toml"),
            "[mcp_servers.shared]\ncommand = \"python\"\n",
        )
        .unwrap_or_else(|error| panic!("profile mcp write failed: {error}"));

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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));

        let resolution = resolve_runtime_mcp_server(&config, "shared", &[])
            .expect("higher-precedence shared server should resolve");
        assert_eq!(resolution.entry.origin_kind, "cwd");
    }

    #[test]
    fn resolve_cli_prompt_overrides_reads_prompt_files() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let system_path = tempdir.path().join("system.txt");
        let append_path = tempdir.path().join("append.txt");
        fs::write(&system_path, "system from file")
            .unwrap_or_else(|error| panic!("system write failed: {error}"));
        fs::write(&append_path, "append from file")
            .unwrap_or_else(|error| panic!("append write failed: {error}"));

        let cli = Cli::parse_from([
            "remote-code",
            "--system-prompt-file",
            system_path.to_string_lossy().as_ref(),
            "--append-system-prompt-file",
            append_path.to_string_lossy().as_ref(),
            "status",
        ]);

        let overrides = resolve_cli_prompt_overrides(&cli)
            .unwrap_or_else(|error| panic!("prompt override resolution failed: {error}"));
        assert_eq!(overrides.system_prompt.as_deref(), Some("system from file"));
        assert_eq!(
            overrides.append_system_prompt.as_deref(),
            Some("append from file")
        );
    }

    #[test]
    fn resolve_cli_prompt_overrides_rejects_mixed_inline_and_file_sources() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let system_path = tempdir.path().join("system.txt");
        fs::write(&system_path, "system from file")
            .unwrap_or_else(|error| panic!("system write failed: {error}"));

        let cli = Cli::parse_from([
            "remote-code",
            "--system-prompt",
            "inline",
            "--system-prompt-file",
            system_path.to_string_lossy().as_ref(),
            "status",
        ]);

        let error = resolve_cli_prompt_overrides(&cli)
            .expect_err("mixed inline and file prompt sources should fail");
        assert!(
            error
                .to_string()
                .contains("Cannot use both --system-prompt and --system-prompt-file"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resolve_cli_prompt_overrides_reports_missing_prompt_file() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let missing = tempdir.path().join("missing.txt");

        let cli = Cli::parse_from([
            "remote-code",
            "--append-system-prompt-file",
            missing.to_string_lossy().as_ref(),
            "status",
        ]);

        let error =
            resolve_cli_prompt_overrides(&cli).expect_err("missing append prompt file should fail");
        assert!(
            error
                .to_string()
                .contains("Append system prompt file not found"),
            "unexpected error: {error}"
        );
        assert!(
            error
                .to_string()
                .contains(missing.to_string_lossy().as_ref()),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_call_output_invokes_stdio_tool() {
        let Some((python, mut prefix_args)) = python_command() else {
            eprintln!("Skipping MCP call output test because Python is unavailable.");
            return;
        };

        let tempdir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).unwrap_or_else(|error| panic!("cwd create failed: {error}"));
        fs::create_dir_all(&profile)
            .unwrap_or_else(|error| panic!("profile create failed: {error}"));

        let script = cwd.join("mock_tool_call.py");
        fs::write(&script, mock_tool_call_server_script())
            .unwrap_or_else(|error| panic!("mock tool script write failed: {error}"));
        prefix_args.push("mock_tool_call.py".to_owned());
        prefix_args.push("success".to_owned());

        fs::write(
            cwd.join("mcp.toml"),
            format!(
                "[mcp_servers.local]\ncommand = \"{}\"\nargs = [{}]\ncwd = \"{}\"\n",
                python,
                prefix_args
                    .iter()
                    .map(|arg| format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", "),
                cwd.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap_or_else(|error| panic!("cwd mcp write failed: {error}"));

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
        .unwrap_or_else(|error| panic!("config load failed: {error}"));

        let output = build_mcp_call_output(
            &config,
            &McpCallArgs {
                server: "local".to_owned(),
                tool: "echo".to_owned(),
                json: false,
                include_disabled: false,
                args: vec!["text=hello".to_owned()],
                args_json: None,
                config_paths: Vec::new(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("mcp call output build failed: {error}"));

        assert!(output.warnings.is_empty());
        assert_eq!(output.server.name, "local");
        assert_eq!(output.response.tool_name, "echo");
        assert_eq!(
            output.response.result.content[0]
                .fields
                .get("text")
                .and_then(serde_json::Value::as_str),
            Some("echo: hello")
        );
    }

    async fn read_http_request(socket: &mut TcpStream) -> (String, Vec<u8>) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read = socket
                .read(&mut buffer)
                .await
                .unwrap_or_else(|error| panic!("read failed: {error}"));
            assert!(
                read != 0,
                "connection closed before request headers completed"
            );
            request.extend_from_slice(&buffer[..read]);
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let header_text = String::from_utf8(request[..header_end].to_vec())
            .unwrap_or_else(|error| panic!("request header utf8 failed: {error}"));
        let content_length = header_text
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("Content-Length")
                        .then_some(value.trim())
                })
            })
            .map_or(0, |value| {
                value
                    .parse::<usize>()
                    .unwrap_or_else(|error| panic!("content length parse failed: {error}"))
            });
        while request.len() < header_end + content_length {
            let read = socket
                .read(&mut buffer)
                .await
                .unwrap_or_else(|error| panic!("read body failed: {error}"));
            assert!(read != 0, "connection closed before request body completed");
            request.extend_from_slice(&buffer[..read]);
        }
        (
            header_text,
            request[header_end..header_end + content_length].to_vec(),
        )
    }

    #[tokio::test]
    async fn remote_http_helpers_round_trip_control_plane_json() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listener bind failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr failed: {error}"));
        let server = tokio::spawn(async move {
            for _ in 0..17 {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .unwrap_or_else(|error| panic!("accept failed: {error}"));
                let (request_text, request_body) = read_http_request(&mut socket).await;
                let body = if request_text.starts_with("GET /v1/meta ") {
                    serde_json::json!({
                        "service": "remote-code-control-plane",
                        "version": "0.1.0-test",
                        "phase": "phase-3",
                        "bind": "127.0.0.1:7001",
                        "public_base_url": "http://127.0.0.1:7001",
                        "runner_lease_ttl_secs": 30,
                        "profile_dir": "C:/Users/test/.remote-code-rust",
                        "state_db_path": "C:/Users/test/.remote-code-rust/state.sqlite3",
                        "artifact_root_dir": "C:/Users/test/.remote-code-rust/artifacts",
                        "auth_required": false,
                        "bootstrap_secret_configured": false
                    })
                } else if request_text.starts_with("GET /v1/runners ") {
                    serde_json::json!({
                        "items": [
                            {
                                "registration": {
                                    "runner_id": "runner-a",
                                    "control_plane_url": "http://127.0.0.1:8787",
                                    "public_base_url": "http://127.0.0.1:9000",
                                    "workspaces": [
                                        {
                                            "workspace_id": "default",
                                            "root_dir": "C:/workspace",
                                            "writable": true
                                        }
                                    ],
                                    "labels": {
                                        "region": "local"
                                    },
                                    "capabilities": {
                                        "interactive_approvals": true,
                                        "background_sessions": true,
                                        "artifact_uploads": true,
                                        "max_parallel_sessions": 4
                                    },
                                    "platform": {
                                        "os": "windows",
                                        "arch": "x86_64",
                                        "family": "windows"
                                    }
                                },
                                "state": "idle",
                                "active_sessions": 0,
                                "queued_sessions": 0,
                                "registered_at": "2026-04-07T00:00:00Z",
                                "last_seen_at": "2026-04-07T00:00:00Z"
                            }
                        ]
                    })
                } else if request_text
                    .starts_with(&format!("GET {} ", remote_runner_path("runner/a")))
                {
                    serde_json::json!({
                        "registration": {
                            "runner_id": "runner/a",
                            "control_plane_url": "http://127.0.0.1:8787",
                            "public_base_url": "http://127.0.0.1:9000",
                            "workspaces": [
                                {
                                    "workspace_id": "default",
                                    "root_dir": "C:/workspace",
                                    "writable": true
                                }
                            ],
                            "labels": {
                                "region": "local"
                            },
                            "capabilities": {
                                "interactive_approvals": true,
                                "background_sessions": true,
                                "artifact_uploads": true,
                                "max_parallel_sessions": 4
                            },
                            "platform": {
                                "os": "windows",
                                "arch": "x86_64",
                                "family": "windows"
                            }
                        },
                        "state": "idle",
                        "active_sessions": 0,
                        "queued_sessions": 0,
                        "registered_at": "2026-04-07T00:00:00Z",
                        "last_seen_at": "2026-04-07T00:00:00Z"
                    })
                } else if request_text.starts_with("POST /v1/sessions ") {
                    serde_json::json!({
                        "session_id": Uuid::nil(),
                        "workspace_id": "default",
                        "owner_runner_id": "runner-a",
                        "state": "assigned",
                        "metadata": {"phase": "remote"},
                        "created_at": "2026-04-07T00:00:00Z",
                        "updated_at": "2026-04-07T00:00:01Z"
                    })
                } else if request_text.starts_with(
                    "GET /v1/runners/runner-a/sessions?workspace_id=default&state=running ",
                ) {
                    serde_json::json!({
                        "items": [
                            {
                                "session_id": Uuid::nil(),
                                "workspace_id": "default",
                                "owner_runner_id": "runner-a",
                                "state": "running",
                                "metadata": {"phase": "remote"},
                                "created_at": "2026-04-07T00:00:00Z",
                                "updated_at": "2026-04-07T00:00:01Z"
                            }
                        ]
                    })
                } else if request_text
                    .starts_with(&format!("POST {} ", remote_session_state_path(Uuid::nil())))
                {
                    let request: SessionStateUpdateRequest = serde_json::from_slice(&request_body)
                        .unwrap_or_else(|error| panic!("session state parse failed: {error}"));
                    assert_eq!(request.state, RemoteSessionState::Completed);
                    assert_eq!(
                        request.metadata.get("reason").map(String::as_str),
                        Some("operator-finished")
                    );
                    serde_json::json!({
                        "session_id": Uuid::nil(),
                        "workspace_id": "default",
                        "owner_runner_id": "runner-a",
                        "state": "completed",
                        "metadata": {
                            "phase": "remote",
                            "reason": "operator-finished"
                        },
                        "created_at": "2026-04-07T00:00:00Z",
                        "updated_at": "2026-04-07T00:00:06Z"
                    })
                } else if request_text.starts_with("GET /v1/approvals ") {
                    serde_json::json!({
                        "items": [
                            {
                                "approval_id": Uuid::nil(),
                                "session_id": Uuid::nil(),
                                "runner_id": "runner-a",
                                "state": "pending",
                                "title": "Run shell",
                                "description": "Need confirmation",
                                "metadata": {"tool": "bash_command"},
                                "created_at": "2026-04-07T00:00:02Z",
                                "updated_at": "2026-04-07T00:00:02Z",
                                "responded_at": null,
                                "responder": null,
                                "note": null
                            }
                        ]
                    })
                } else if request_text
                    .starts_with(&format!("POST /v1/sessions/{}/approvals ", Uuid::nil()))
                {
                    let request: SharedApprovalCreateRequest =
                        serde_json::from_slice(&request_body).unwrap_or_else(|error| {
                            panic!("approval create parse failed: {error}")
                        });
                    assert_eq!(request.title, "Execute shell command");
                    assert_eq!(request.description, "Needs operator confirmation");
                    assert_eq!(
                        request.metadata.get("tool").map(String::as_str),
                        Some("bash_command")
                    );
                    serde_json::json!({
                        "approval_id": Uuid::nil(),
                        "session_id": Uuid::nil(),
                        "runner_id": "runner-a",
                        "state": "pending",
                        "title": request.title,
                        "description": request.description,
                        "metadata": request.metadata,
                        "created_at": "2026-04-07T00:00:02Z",
                        "updated_at": "2026-04-07T00:00:02Z",
                        "responded_at": null,
                        "responder": null,
                        "note": null
                    })
                } else if request_text
                    .starts_with(&format!("GET {} ", remote_approval_path(Uuid::nil())))
                {
                    serde_json::json!({
                        "approval_id": Uuid::nil(),
                        "session_id": Uuid::nil(),
                        "runner_id": "runner-a",
                        "state": "pending",
                        "title": "Run shell",
                        "description": "Need confirmation",
                        "metadata": {"tool": "bash_command"},
                        "created_at": "2026-04-07T00:00:02Z",
                        "updated_at": "2026-04-07T00:00:02Z",
                        "responded_at": null,
                        "responder": null,
                        "note": null
                    })
                } else if request_text.starts_with("GET /v1/events?after=1&limit=5 ") {
                    serde_json::json!({
                        "items": [
                            {
                                "sequence": 2,
                                "recorded_at": "2026-04-07T00:00:03Z",
                                "runner_id": "runner-a",
                                "session_id": Uuid::nil(),
                                "detail": {
                                    "kind": "approval_requested",
                                    "approval_id": Uuid::nil(),
                                    "title": "Run shell",
                                    "state": "pending"
                                }
                            }
                        ]
                    })
                } else if request_text.starts_with(
                    "GET /v1/runners/runner-a/events?after=1&limit=5&kind=session_created ",
                ) {
                    serde_json::json!({
                        "items": [
                            {
                                "sequence": 3,
                                "recorded_at": "2026-04-07T00:00:04Z",
                                "runner_id": "runner-a",
                                "session_id": Uuid::nil(),
                                "detail": {
                                    "kind": "session_created",
                                    "workspace_id": "default",
                                    "owner_runner_id": "runner-a",
                                    "state": "running"
                                }
                            }
                        ]
                    })
                } else if request_text.starts_with("GET /v1/runners/runner-a/artifacts ") {
                    serde_json::json!({
                        "items": [
                            {
                                "artifact_id": Uuid::nil(),
                                "session_id": Uuid::nil(),
                                "runner_id": "runner-a",
                                "name": "runner-log",
                                "file_name": "runner-log.txt",
                                "media_type": "text/plain",
                                "size_bytes": 12,
                                "metadata": {"kind": "runner-log"},
                                "created_at": "2026-04-07T00:00:04Z"
                            }
                        ]
                    })
                } else if request_text.starts_with("GET /v1/artifacts ") {
                    serde_json::json!({
                        "items": [
                            {
                                "artifact_id": Uuid::nil(),
                                "session_id": Uuid::nil(),
                                "runner_id": "runner-a",
                                "name": "transcript",
                                "file_name": "transcript.json",
                                "media_type": "application/json",
                                "size_bytes": 14,
                                "metadata": {"kind": "export"},
                                "created_at": "2026-04-07T00:00:04Z"
                            }
                        ]
                    })
                } else if request_text.starts_with(&format!("GET /v1/artifacts/{} ", Uuid::nil())) {
                    serde_json::json!({
                        "artifact_id": Uuid::nil(),
                        "session_id": Uuid::nil(),
                        "runner_id": "runner-a",
                        "name": "transcript",
                        "file_name": "transcript.json",
                        "media_type": "application/json",
                        "size_bytes": 14,
                        "metadata": {"kind": "export"},
                        "created_at": "2026-04-07T00:00:04Z"
                    })
                } else if request_text
                    .starts_with(&format!("GET /v1/sessions/{}/artifacts ", Uuid::nil()))
                {
                    serde_json::json!({
                        "items": [
                            {
                                "artifact_id": Uuid::nil(),
                                "session_id": Uuid::nil(),
                                "runner_id": "runner-a",
                                "name": "transcript",
                                "file_name": "transcript.json",
                                "media_type": "application/json",
                                "size_bytes": 14,
                                "metadata": {"kind": "export"},
                                "created_at": "2026-04-07T00:00:04Z"
                            }
                        ]
                    })
                } else if request_text
                    .starts_with(&format!("POST /v1/sessions/{}/artifacts ", Uuid::nil()))
                {
                    let request: ArtifactCreateRequest = serde_json::from_slice(&request_body)
                        .unwrap_or_else(|error| panic!("artifact upload parse failed: {error}"));
                    assert_eq!(request.name, "session-export");
                    assert_eq!(request.file_name.as_deref(), Some("session-export.json"));
                    assert_eq!(request.media_type.as_deref(), Some("application/json"));
                    assert_eq!(
                        BASE64_STANDARD
                            .decode(request.content_base64.as_bytes())
                            .unwrap_or_else(|error| panic!(
                                "artifact upload decode failed: {error}"
                            )),
                        br#"{"ok":true}"#
                    );
                    assert_eq!(
                        request.metadata.get("kind").map(String::as_str),
                        Some("export")
                    );
                    serde_json::json!({
                        "artifact_id": Uuid::nil(),
                        "session_id": Uuid::nil(),
                        "runner_id": "runner-a",
                        "name": request.name,
                        "file_name": request.file_name.unwrap_or_else(|| "session-export.json".to_owned()),
                        "media_type": request.media_type.unwrap_or_else(|| "application/json".to_owned()),
                        "size_bytes": 11,
                        "metadata": request.metadata,
                        "created_at": "2026-04-07T00:00:05Z"
                    })
                } else if request_text
                    .starts_with(&format!("GET /v1/artifacts/{}/download ", Uuid::nil()))
                {
                    let payload = b"artifact-bytes".to_vec();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .unwrap_or_else(|error| panic!("response header write failed: {error}"));
                    socket
                        .write_all(&payload)
                        .await
                        .unwrap_or_else(|error| panic!("response body write failed: {error}"));
                    continue;
                } else {
                    panic!("unexpected request: {request_text}");
                };
                let payload = serde_json::to_vec(&body)
                    .unwrap_or_else(|error| panic!("serialize failed: {error}"));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .unwrap_or_else(|error| panic!("response header write failed: {error}"));
                socket
                    .write_all(&payload)
                    .await
                    .unwrap_or_else(|error| panic!("response body write failed: {error}"));
            }
        });

        let base_url = format!("http://{address}");
        let meta: RemoteControlPlaneMeta = remote_get_json(&base_url, "/v1/meta")
            .await
            .unwrap_or_else(|error| panic!("remote meta get failed: {error}"));
        assert_eq!(meta.service, "remote-code-control-plane");
        assert_eq!(meta.phase, "phase-3");

        let runners: RemoteListResponse<RemoteRunnerSnapshot> =
            remote_get_json(&base_url, "/v1/runners")
                .await
                .unwrap_or_else(|error| panic!("remote get failed: {error}"));
        assert_eq!(runners.items.len(), 1);
        assert_eq!(runners.items[0].registration.runner_id, "runner-a");

        let runner: RemoteRunnerSnapshot =
            remote_get_json(&base_url, &remote_runner_path("runner/a"))
                .await
                .unwrap_or_else(|error| panic!("remote runner show failed: {error}"));
        assert_eq!(runner.registration.runner_id, "runner/a");

        let created: RemoteSessionRecord = remote_post_json(
            &base_url,
            "/v1/sessions",
            &serde_json::json!({"workspace_id": "default"}),
        )
        .await
        .unwrap_or_else(|error| panic!("remote post failed: {error}"));
        assert_eq!(created.workspace_id, "default");
        assert_eq!(created.owner_runner_id.as_deref(), Some("runner-a"));

        let filtered_sessions: RemoteListResponse<RemoteSessionRecord> = remote_get_json(
            &base_url,
            &remote_sessions_path(
                Some("runner-a"),
                Some("default"),
                Some(RemoteSessionState::Running),
            ),
        )
        .await
        .unwrap_or_else(|error| panic!("remote filtered sessions get failed: {error}"));
        assert_eq!(filtered_sessions.items.len(), 1);
        assert_eq!(filtered_sessions.items[0].state.label(), "running");

        let approvals: RemoteListResponse<RemoteApprovalRecord> =
            remote_get_json(&base_url, "/v1/approvals")
                .await
                .unwrap_or_else(|error| panic!("remote approvals get failed: {error}"));
        assert_eq!(approvals.items.len(), 1);
        assert_eq!(approvals.items[0].title, "Run shell");
        assert_eq!(approvals.items[0].state.label(), "pending");

        let created_approval: RemoteApprovalRecord = remote_post_json(
            &base_url,
            &format!("/v1/sessions/{}/approvals", Uuid::nil()),
            &SharedApprovalCreateRequest {
                approval_id: None,
                title: "Execute shell command".to_owned(),
                description: "Needs operator confirmation".to_owned(),
                metadata: [("tool".to_owned(), "bash_command".to_owned())]
                    .into_iter()
                    .collect(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("remote approval create failed: {error}"));
        assert_eq!(created_approval.title, "Execute shell command");
        assert_eq!(created_approval.state.label(), "pending");
        assert_eq!(
            created_approval.metadata.get("tool").map(String::as_str),
            Some("bash_command")
        );

        let approval: RemoteApprovalRecord =
            remote_get_json(&base_url, &remote_approval_path(Uuid::nil()))
                .await
                .unwrap_or_else(|error| panic!("remote approval show failed: {error}"));
        assert_eq!(approval.title, "Run shell");

        let events: RemoteListResponse<RemoteTimelineEvent> = remote_get_json(
            &base_url,
            &remote_events_path(None, None, Some(1), 5, None)
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("remote events get failed: {error}"));
        assert_eq!(events.items.len(), 1);
        assert_eq!(events.items[0].sequence, 2);
        match &events.items[0].detail {
            RemoteTimelineEventDetail::ApprovalRequested { title, .. } => {
                assert_eq!(title, "Run shell");
            }
            other => panic!("unexpected event detail: {other:?}"),
        }

        let runner_events: RemoteListResponse<RemoteTimelineEvent> = remote_get_json(
            &base_url,
            &remote_events_path(
                None,
                Some("runner-a"),
                Some(1),
                5,
                Some(RemoteEventKindValue::SessionCreated),
            )
            .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("remote runner events get failed: {error}"));
        assert_eq!(runner_events.items.len(), 1);
        assert_eq!(runner_events.items[0].sequence, 3);
        match &runner_events.items[0].detail {
            RemoteTimelineEventDetail::SessionCreated {
                owner_runner_id, ..
            } => {
                assert_eq!(owner_runner_id.as_deref(), Some("runner-a"));
            }
            other => panic!("unexpected runner event detail: {other:?}"),
        }

        let runner_artifacts: RemoteListResponse<ArtifactRecord> = remote_get_json(
            &base_url,
            &remote_artifacts_path(None, Some("runner-a"))
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("remote runner artifacts get failed: {error}"));
        assert_eq!(runner_artifacts.items.len(), 1);
        assert_eq!(runner_artifacts.items[0].file_name, "runner-log.txt");

        let artifacts: RemoteListResponse<ArtifactRecord> =
            remote_get_json(&base_url, "/v1/artifacts")
                .await
                .unwrap_or_else(|error| panic!("remote artifacts get failed: {error}"));
        assert_eq!(artifacts.items.len(), 1);
        assert_eq!(artifacts.items[0].file_name, "transcript.json");

        let session_artifacts: RemoteListResponse<ArtifactRecord> = remote_get_json(
            &base_url,
            &format!("/v1/sessions/{}/artifacts", Uuid::nil()),
        )
        .await
        .unwrap_or_else(|error| panic!("remote session artifacts get failed: {error}"));
        assert_eq!(session_artifacts.items.len(), 1);

        let artifact: ArtifactRecord =
            remote_get_json(&base_url, &format!("/v1/artifacts/{}", Uuid::nil()))
                .await
                .unwrap_or_else(|error| panic!("remote artifact show failed: {error}"));
        assert_eq!(artifact.name, "transcript");

        let artifact_bytes = remote_get_bytes(
            &base_url,
            &format!("/v1/artifacts/{}/download", Uuid::nil()),
        )
        .await
        .unwrap_or_else(|error| panic!("remote artifact download failed: {error}"));
        assert_eq!(artifact_bytes, b"artifact-bytes");

        let uploaded: ArtifactRecord = remote_post_json(
            &base_url,
            &format!("/v1/sessions/{}/artifacts", Uuid::nil()),
            &ArtifactCreateRequest {
                name: "session-export".to_owned(),
                file_name: Some("session-export.json".to_owned()),
                media_type: Some("application/json".to_owned()),
                content_base64: BASE64_STANDARD.encode(br#"{"ok":true}"#),
                metadata: [("kind".to_owned(), "export".to_owned())]
                    .into_iter()
                    .collect(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("remote artifact upload failed: {error}"));
        assert_eq!(uploaded.name, "session-export");
        assert_eq!(uploaded.file_name, "session-export.json");
        assert_eq!(uploaded.size_bytes, 11);
        assert_eq!(
            uploaded.metadata.get("kind").map(String::as_str),
            Some("export")
        );

        let updated_session: RemoteSessionRecord = remote_post_json(
            &base_url,
            &remote_session_state_path(Uuid::nil()),
            &SessionStateUpdateRequest {
                state: RemoteSessionState::Completed,
                metadata: [("reason".to_owned(), "operator-finished".to_owned())]
                    .into_iter()
                    .collect(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("remote session state update failed: {error}"));
        assert_eq!(updated_session.state.label(), "completed");
        assert_eq!(
            updated_session.metadata.get("reason").map(String::as_str),
            Some("operator-finished")
        );

        server
            .await
            .unwrap_or_else(|error| panic!("server join failed: {error}"));
    }

    fn python_command() -> Option<(String, Vec<String>)> {
        let probe = |cmd: &str, args: &[&str]| -> bool {
            let mut cmd = ProcessCommand::new(cmd);
            cmd.args(args).args(["-c", "import json"]);
            cmd.output().is_ok_and(|output| output.status.success())
        };

        if let Ok(path) = std::env::var("PYTHON")
            && probe(&path, &[])
        {
            return Some((path, Vec::new()));
        }

        for candidate in ["python", "python3"] {
            if probe(candidate, &[]) {
                return Some((candidate.to_owned(), Vec::new()));
            }
        }

        if cfg!(windows) && probe("py", &["-3"]) {
            return Some(("py".to_owned(), vec!["-3".to_owned()]));
        }

        None
    }

    fn mock_tool_call_server_script() -> &'static str {
        r#"
import json
import sys

mode = sys.argv[1] if len(sys.argv) > 1 else "success"

while True:
    raw = sys.stdin.readline()
    if not raw:
        break
    raw = raw.strip()
    if not raw:
        continue
    message = json.loads(raw)
    method = message.get("method")
    message_id = message.get("id")

    if method == "initialize":
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.1.0"}
            }
        }), flush=True)
    elif method == "notifications/initialized":
        continue
    elif method == "tools/call":
        text = message["params"]["arguments"]["text"]
        if mode == "success":
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "content": [{"type": "text", "text": f"echo: {text}"}],
                    "structuredContent": {"echoed": text},
                    "isError": False
                }
            }), flush=True)
        else:
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": message_id,
                "error": {"code": -32001, "message": "tool call failed"}
            }), flush=True)
        break
"#
    }
}
