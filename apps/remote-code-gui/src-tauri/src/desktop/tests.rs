use super::*;
use chrono::Utc;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;
use uuid::Uuid;

fn runtime_policy_test_mutex() -> &'static Mutex<()> {
    static RUNTIME_POLICY_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    RUNTIME_POLICY_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn test_runtime_config(project_dir: &Path, profile_dir: &Path) -> RuntimeConfig {
    load_runtime_config(
        Some(project_dir.to_path_buf()),
        Some(profile_dir.to_path_buf()),
        None,
        PermissionMode::AcceptEdits,
        claude_core::InputFormat::Text,
        claude_core::OutputFormat::Text,
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
            protocol: Some(ProviderProtocol::Anthropic),
        },
        RuntimeOverrides {
            session_name: Some("Parity".to_owned()),
            system_prompt: None,
            append_system_prompt: None,
            settings_files: Vec::new(),
            show_setting_sources: true,
            allowed_setting_sources: None,
            allowed_tools: vec!["read_file".to_owned()],
            disallowed_tools: vec!["bash_command".to_owned()],
            structured_output_schema: None,
            mcp_config_paths: Vec::new(),
            strict_mcp_config: false,
            effort: Some("medium".to_owned()),
            fallback_model: Some("glm-5-turbo".to_owned()),
            output_style: Some("Explanatory".to_owned()),
            language: Some("Chinese".to_owned()),
            brief_enabled: Some(true),
            proactive_active: Some(true),
        },
    )
    .expect("config should load")
}

#[test]
fn roo_provider_id_uses_protocol_and_endpoint_not_display_name() {
    let project_dir = tempdir().unwrap();
    let profile_dir = tempdir().unwrap();
    let mut config = test_runtime_config(project_dir.path(), profile_dir.path());

    config.provider.name = "MINIMAX TOKEN PLAN".to_string();
    config.provider.protocol = ProviderProtocol::Anthropic;
    config.provider.base_url = Some("https://api.minimaxi.com/anthropic/v1/messages".to_string());
    assert_eq!(roo_provider_id_from_runtime(&config.provider), "minimax");

    config.provider.name = "Production Claude".to_string();
    config.provider.base_url = Some("https://example.com/v1/messages".to_string());
    assert_eq!(roo_provider_id_from_runtime(&config.provider), "anthropic");

    config.provider.name = "OpenAI Work".to_string();
    config.provider.protocol = ProviderProtocol::OpenAi;
    config.provider.base_url = Some("https://gateway.example.com/v1/chat/completions".to_string());
    assert_eq!(roo_provider_id_from_runtime(&config.provider), "openai");

    config.provider.name = "KuaiKAT Coding Plan".to_string();
    config.provider.protocol = ProviderProtocol::Anthropic;
    config.provider.base_url = Some(
        "https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy"
            .to_string(),
    );
    assert_eq!(roo_provider_id_from_runtime(&config.provider), "kuaikat");

    config.provider.name = "DeepSeek".to_string();
    config.provider.base_url = Some("https://api.deepseek.com/anthropic".to_string());
    assert_eq!(roo_provider_id_from_runtime(&config.provider), "anthropic");
}

fn sample_stdio_mcp_server(name: &str, enabled: bool, command: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_owned(),
        enabled,
        transport: McpTransportConfig::Stdio {
            command: command.to_owned(),
            args: vec!["--serve".to_owned()],
            cwd: None,
            env: BTreeMap::new(),
        },
        capabilities: claude_mcp::McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(5),
        request_timeout_secs: Some(30),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: claude_mcp::McpToolPolicy::default(),
    }
}

#[test]
fn anthropic_base_url_is_normalized_from_gui_input() {
    let config = normalize_provider_config(ProviderConfig {
        name: "glm".to_owned(),
        protocol: "anthropic".to_owned(),
        base_url: Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
        api_key: Some("secret".to_owned()),
        model: Some("glm-5.1".to_owned()),
        profiles: vec![],
        active_profile: None,
        api_key_stored: false,
    })
    .expect("provider config should normalize");

    assert_eq!(
        config.base_url.as_deref(),
        Some("https://open.bigmodel.cn/api/anthropic/v1/messages")
    );
}

#[test]
fn permission_mode_aliases_map_to_runtime_modes() {
    assert_eq!(
        parse_permission_mode(Some("suggest")),
        Some(PermissionMode::Default)
    );
    assert_eq!(
        parse_permission_mode(Some("auto-edit")),
        Some(PermissionMode::AcceptEdits)
    );
    assert_eq!(
        parse_permission_mode(Some("full-auto")),
        Some(PermissionMode::BypassPermissions)
    );
    assert_eq!(
        parse_permission_mode(Some("yolo")),
        Some(PermissionMode::BypassPermissions)
    );
}

#[test]
fn permission_request_dto_preserves_permission_suggestions() {
    let request = PermissionRequest {
        tool_name: "read_file".to_owned(),
        permission_class: Some(claude_permissions::PermissionClass::Read),
        tool_input: json!({"path": "..\\outside.txt"}),
        working_directory: None,
        tool_use_id: Some("tool-1".to_owned()),
        title: Some("Read outside workspace".to_owned()),
        description: Some("Read requires approval".to_owned()),
        blocked_path: Some("C:\\outside.txt".to_owned()),
        permission_suggestions: vec![json!({
            "action": "addRules",
            "toolPattern": "Read(C:\\outside.txt)",
        })],
    };

    let dto = permission_request_dto("request-1".to_owned(), &request);

    assert_eq!(dto.request_id, "request-1");
    assert_eq!(dto.tool_use_id, "tool-1");
    assert_eq!(dto.blocked_path.as_deref(), Some("C:\\outside.txt"));
    assert_eq!(dto.permission_suggestions.len(), 1);
    assert_eq!(dto.permission_suggestions[0]["action"], "addRules");
    assert_eq!(
        dto.permission_suggestions[0]["toolPattern"],
        "Read(C:\\outside.txt)"
    );
}

#[test]
fn codex_permission_decision_preserves_session_scope() {
    let session_update = PermissionUpdate::SetMode {
        destination: PermissionUpdateDestination::Session,
        mode: claude_permissions::ExtendedPermissionMode::AcceptEdits,
    };
    let user_update = PermissionUpdate::SetMode {
        destination: PermissionUpdateDestination::UserSettings,
        mode: claude_permissions::ExtendedPermissionMode::AcceptEdits,
    };

    assert_eq!(
        codex_permission_decision(true, &[session_update]),
        AgentPermissionDecision::AllowAll
    );
    assert_eq!(
        codex_permission_decision(true, &[user_update]),
        AgentPermissionDecision::Allow
    );
    assert_eq!(
        codex_permission_decision(false, &[]),
        AgentPermissionDecision::Deny
    );
}

#[test]
fn usage_info_from_codex_token_usage_handles_progress_payload() {
    let payload = json!({
        "method": "thread/tokenUsage/updated",
        "params": {
            "tokenUsage": {
                "total": {
                    "inputTokens": 12,
                    "cachedInputTokens": 3,
                    "outputTokens": 5,
                    "totalTokens": 20,
                    "reasoningOutputTokens": 0
                }
            }
        }
    });

    let usage = usage_info_from_codex_token_usage(&payload).expect("usage should parse");

    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.cache_read, 3);
    assert_eq!(usage.cache_write, 0);
}

#[test]
fn provider_config_sanitizer_trims_blank_fields() {
    let config = normalize_provider_config(ProviderConfig {
        name: "  minimax  ".to_owned(),
        protocol: "anthropic".to_owned(),
        base_url: Some(" https://api.minimaxi.com/anthropic/ ".to_owned()),
        api_key: Some("  token  ".to_owned()),
        model: Some(" minimax-m2.7 ".to_owned()),
        profiles: vec![],
        active_profile: None,
        api_key_stored: false,
    })
    .expect("provider config should sanitize");

    assert_eq!(config.name, "minimax");
    assert_eq!(config.api_key.as_deref(), Some("token"));
    assert_eq!(config.model.as_deref(), Some("minimax-m2.7"));
}

#[test]
fn runtime_status_snapshot_includes_auth_and_tool_filters() {
    let temp = std::env::temp_dir().join(format!("remote-code-gui-status-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp).expect("temp dir should work");
    let config = test_runtime_config(&temp, &temp.join(".remote-code-rust"));

    let snapshot = runtime_status_snapshot_from_config(&config);
    assert_eq!(snapshot.provider.name, "glm-coding");
    assert_eq!(snapshot.provider.effort.as_deref(), Some("medium"));
    assert_eq!(snapshot.permission_mode, "acceptEdits");
    assert_eq!(snapshot.output_style.as_deref(), Some("Explanatory"));
    assert_eq!(snapshot.language.as_deref(), Some("Chinese"));
    assert!(snapshot.brief_enabled);
    assert!(snapshot.proactive_active);
    assert_eq!(
        snapshot.allowed_setting_sources,
        vec!["user", "project", "local"]
    );
    assert_eq!(snapshot.allowed_tools, vec!["read_file"]);
    assert_eq!(snapshot.disallowed_tools, vec!["bash_command"]);
    assert_eq!(snapshot.mcp.total_servers, 0);
    assert_eq!(snapshot.mcp.enabled_servers, 0);
    assert_eq!(snapshot.mcp.disabled_servers, 0);
    let _ = std::fs::remove_dir_all(&temp);
}

#[tokio::test]
async fn build_runtime_mcp_inventory_uses_project_override_and_runtime_origins() {
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    let plugin_root = profile_dir.join("plugins").join("sample");
    fs::create_dir_all(&project_dir).expect("project dir should exist");
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
    fs::write(
        plugin_root.join(".codex-plugin").join("plugin.json"),
        r#"{"name":"sample","version":"0.1.0","mcp":"./mcp.toml"}"#,
    )
    .expect("plugin manifest write");

    let mut plugin_mcp = McpConfig::default();
    plugin_mcp.servers.insert(
        "plugin-demo".to_owned(),
        sample_stdio_mcp_server("plugin-demo", true, "plugin-cmd"),
    );
    plugin_mcp
        .save(plugin_root.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("plugin MCP config should save");

    let mut profile_mcp = McpConfig::default();
    profile_mcp.servers.insert(
        "profile-demo".to_owned(),
        sample_stdio_mcp_server("profile-demo", true, "profile-cmd"),
    );
    profile_mcp
        .save(profile_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("profile MCP config should save");

    let mut project_mcp = McpConfig::default();
    project_mcp.servers.insert(
        "project-demo".to_owned(),
        sample_stdio_mcp_server("project-demo", true, "project-cmd"),
    );
    project_mcp.servers.insert(
        "disabled-demo".to_owned(),
        sample_stdio_mcp_server("disabled-demo", false, "disabled-cmd"),
    );
    project_mcp
        .save(project_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("project MCP config should save");

    let config = test_runtime_config(temp.path(), &profile_dir);
    let inventory = build_runtime_mcp_inventory(
        &config,
        Some(project_dir.to_str().expect("utf8 project path")),
        &[],
        false,
        true,
    )
    .await
    .expect("runtime inventory should build");

    assert_eq!(inventory.effective_cwd, project_dir.display().to_string());
    assert_eq!(inventory.warnings.len(), 0);
    assert_eq!(inventory.summary.total_servers, 4);
    assert_eq!(inventory.summary.status_counts.pending, 3);
    assert_eq!(inventory.summary.status_counts.disabled, 1);
    assert_eq!(inventory.servers.len(), 4);
    assert!(
        inventory
            .servers
            .iter()
            .any(|server| server.name == "project-demo"
                && server.status == "pending"
                && server.origin_kind == "cwd")
    );
    assert!(
        inventory
            .servers
            .iter()
            .any(|server| server.name == "disabled-demo"
                && server.status == "disabled"
                && server.origin_kind == "cwd")
    );
    assert!(
        inventory
            .servers
            .iter()
            .any(|server| server.name == "profile-demo" && server.origin_kind == "profile")
    );
    assert!(
        inventory
            .servers
            .iter()
            .any(|server| server.name == "plugin-demo"
                && server.origin_kind == "plugin"
                && server.origin_name == "sample")
    );

    let snapshot = runtime_status_snapshot_from_config(&config);
    assert_eq!(snapshot.mcp.total_servers, 2);
    assert_eq!(snapshot.mcp.enabled_servers, 2);
    assert_eq!(snapshot.mcp.disabled_servers, 0);
    assert_eq!(snapshot.mcp.origins.profile, 1);
    assert_eq!(snapshot.mcp.origins.plugin, 1);
}

#[test]
fn configure_runtime_policy_for_config_populates_runtime_mcp_inventory() {
    let _runtime_policy_guard = runtime_policy_test_mutex()
        .lock()
        .expect("runtime policy test mutex");
    let original_policy = claude_tools::current_tool_runtime_policy();
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    let plugin_root = profile_dir.join("plugins").join("sample");
    fs::create_dir_all(&project_dir).expect("project dir should exist");
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
    fs::write(
        plugin_root.join(".codex-plugin").join("plugin.json"),
        r#"{"name":"sample","version":"0.1.0","mcp":"./mcp.toml"}"#,
    )
    .expect("plugin manifest write");

    let mut plugin_mcp = McpConfig::default();
    plugin_mcp.servers.insert(
        "plugin-demo".to_owned(),
        sample_stdio_mcp_server("plugin-demo", true, "plugin-cmd"),
    );
    plugin_mcp
        .save(plugin_root.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("plugin MCP config should save");

    let mut profile_mcp = McpConfig::default();
    profile_mcp.servers.insert(
        "profile-demo".to_owned(),
        sample_stdio_mcp_server("profile-demo", true, "profile-cmd"),
    );
    profile_mcp
        .save(profile_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("profile MCP config should save");

    let mut project_mcp = McpConfig::default();
    project_mcp.servers.insert(
        "project-demo".to_owned(),
        sample_stdio_mcp_server("project-demo", true, "project-cmd"),
    );
    project_mcp
        .save(project_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("project MCP config should save");

    let config = test_runtime_config(&project_dir, &profile_dir);
    configure_runtime_policy_for_config(&config).expect("runtime policy should configure");

    let policy = claude_tools::current_tool_runtime_policy();
    let names = policy
        .mcp_servers
        .iter()
        .map(|entry| entry.server.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["plugin-demo", "profile-demo", "project-demo"])
    );
    assert!(
        policy
            .mcp_servers
            .iter()
            .any(|entry| entry.server.name == "plugin-demo"
                && entry.origin_kind == "plugin"
                && entry.origin_name == "sample")
    );

    claude_tools::configure_tool_runtime_policy(original_policy)
        .expect("runtime policy should restore");
}

#[tokio::test]
async fn build_gui_doctor_report_counts_managed_mcp_servers() {
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    fs::create_dir_all(&project_dir).expect("project dir should exist");

    let config = test_runtime_config(&project_dir, &profile_dir);

    let mut profile_mcp = McpConfig::default();
    profile_mcp.servers.insert(
        "profile-demo".to_owned(),
        sample_stdio_mcp_server("profile-demo", true, "profile-cmd"),
    );
    profile_mcp
        .save(profile_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("profile MCP config should save");

    let mut project_mcp = McpConfig::default();
    project_mcp.servers.insert(
        "project-demo".to_owned(),
        sample_stdio_mcp_server("project-demo", true, "project-cmd"),
    );
    project_mcp
        .save(project_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("project MCP config should save");

    let report = build_gui_doctor_report(&config, false, false, false, false)
        .await
        .expect("doctor report should build");

    assert!(report.ok);
    assert_eq!(report.extensions.managed_mcp_servers, 2);
    assert_eq!(report.extensions.plugin_mcp_servers, 0);
    assert_eq!(report.mcp_runtime.summary.total_servers, 2);
    assert_eq!(report.mcp_runtime.summary.status_counts.pending, 2);
    assert_eq!(report.mcp_runtime.summary.status_counts.failed, 0);
    assert!(report.network.is_empty());
    assert!(report.env_providers.is_empty());
}

#[tokio::test]
async fn build_gui_doctor_report_respects_setting_sources() {
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    let plugin_root = profile_dir.join("plugins").join("sample");
    fs::create_dir_all(&project_dir).expect("project dir should exist");
    fs::create_dir_all(profile_dir.join("skills").join("demo")).expect("profile skills");
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("plugin dir");
    fs::write(
        profile_dir.join("skills").join("demo").join("SKILL.md"),
        "# Demo\n\nSummary.\n",
    )
    .expect("profile skill write");
    fs::write(
        plugin_root.join(".codex-plugin").join("plugin.json"),
        r#"{"name":"sample","version":"0.1.0","mcp":"./mcp.toml"}"#,
    )
    .expect("plugin manifest write");
    let mut plugin_mcp = McpConfig::default();
    plugin_mcp.servers.insert(
        "plugin-demo".to_owned(),
        sample_stdio_mcp_server("plugin-demo", true, "plugin-cmd"),
    );
    plugin_mcp
        .save(plugin_root.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("plugin MCP config should save");

    let mut profile_mcp = McpConfig::default();
    profile_mcp.servers.insert(
        "profile-demo".to_owned(),
        sample_stdio_mcp_server("profile-demo", true, "profile-cmd"),
    );
    profile_mcp
        .save(profile_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("profile MCP config should save");

    let mut project_mcp = McpConfig::default();
    project_mcp.servers.insert(
        "project-demo".to_owned(),
        sample_stdio_mcp_server("project-demo", true, "project-cmd"),
    );
    project_mcp
        .save(project_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("project MCP config should save");

    let mut project_only = test_runtime_config(&project_dir, &profile_dir);
    project_only.allowed_setting_sources = vec![SettingSource::Project];
    let report = build_gui_doctor_report(&project_only, false, false, false, false)
        .await
        .expect("project-only doctor report");
    assert_eq!(report.runtime.allowed_setting_sources, vec!["project"]);
    assert_eq!(report.extensions.skills, 0);
    assert_eq!(report.extensions.plugins, 0);
    assert_eq!(report.extensions.disabled_plugins, 0);
    assert_eq!(report.extensions.managed_mcp_servers, 1);
    assert_eq!(report.extensions.plugin_mcp_servers, 0);
    assert_eq!(report.mcp_runtime.summary.total_servers, 1);

    let mut user_only = test_runtime_config(&project_dir, &profile_dir);
    user_only.allowed_setting_sources = vec![SettingSource::User];
    let report = build_gui_doctor_report(&user_only, false, false, false, false)
        .await
        .expect("user-only doctor report");
    assert_eq!(report.runtime.allowed_setting_sources, vec!["user"]);
    assert_eq!(report.extensions.skills, 1);
    assert_eq!(report.extensions.plugins, 1);
    assert_eq!(report.extensions.managed_mcp_servers, 1);
    assert_eq!(report.extensions.plugin_mcp_servers, 1);
    assert_eq!(report.mcp_runtime.summary.total_servers, 2);
}

#[tokio::test]
async fn build_mcp_server_list_respects_setting_sources() {
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    fs::create_dir_all(&project_dir).expect("project dir should exist");

    let mut profile_mcp = McpConfig::default();
    profile_mcp.servers.insert(
        "profile-demo".to_owned(),
        sample_stdio_mcp_server("profile-demo", true, "profile-cmd"),
    );
    profile_mcp
        .save(profile_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("profile MCP config should save");

    let mut project_mcp = McpConfig::default();
    project_mcp.servers.insert(
        "project-demo".to_owned(),
        sample_stdio_mcp_server("project-demo", true, "project-cmd"),
    );
    project_mcp
        .save(project_dir.join(DEFAULT_MCP_CONFIG_FILE))
        .expect("project MCP config should save");
    let projects = vec![ProjectEntry {
        path: project_dir.clone(),
        name: "project".to_owned(),
    }];

    let mut user_only = test_runtime_config(&project_dir, &profile_dir);
    user_only.allowed_setting_sources = vec![SettingSource::User];
    let list = build_mcp_server_list(
        &user_only,
        ConfigScopeDto::Project,
        Some(project_dir.to_str().expect("utf8 project path")),
        &projects,
        false,
        false,
    )
    .await
    .expect("project list should build");
    assert!(list.servers.is_empty());
    assert_eq!(list.warnings.len(), 1);

    let mut project_only = test_runtime_config(&project_dir, &profile_dir);
    project_only.allowed_setting_sources = vec![SettingSource::Project];
    let list = build_mcp_server_list(
        &project_only,
        ConfigScopeDto::Profile,
        None,
        &[],
        false,
        false,
    )
    .await
    .expect("profile list should build");
    assert!(list.servers.is_empty());
    assert_eq!(list.warnings.len(), 1);
}

#[tokio::test]
async fn managed_mcp_helpers_round_trip_and_reset() {
    let temp = tempdir().expect("tempdir should work");
    let project_dir = temp.path().join("project");
    let profile_dir = temp.path().join(".remote-code-rust");
    fs::create_dir_all(&project_dir).expect("project dir should exist");

    let config = test_runtime_config(&project_dir, &profile_dir);
    let config_path =
        mcp_config_path_for_scope(&config, ConfigScopeDto::Profile, None, &[]).expect("path");

    let request = McpServerUpsertRequestDto {
        scope: ConfigScopeDto::Profile,
        project_path: None,
        name: "demo".to_owned(),
        transport: "stdio".to_owned(),
        command: Some("demo-mcp".to_owned()),
        url: None,
        args: vec!["serve".to_owned()],
        cwd: Some(project_dir.display().to_string()),
        env: BTreeMap::from([("TOKEN".to_owned(), "secret".to_owned())]),
        headers: BTreeMap::new(),
        metadata: BTreeMap::from([("team".to_owned(), "gui".to_owned())]),
        disabled: false,
        startup_timeout_secs: Some(10),
        request_timeout_secs: Some(20),
    };

    let saved = save_managed_mcp_server_at_path(&config_path, ConfigScopeDto::Profile, &request)
        .expect("save should succeed");
    assert_eq!(saved.status, "created");
    assert_eq!(saved.enabled, Some(true));

    let listed = build_mcp_server_list(&config, ConfigScopeDto::Profile, None, &[], false, true)
        .await
        .expect("list should succeed");
    assert_eq!(listed.servers.len(), 1);
    assert_eq!(listed.servers[0].name, "demo");
    assert_eq!(listed.servers[0].command.as_deref(), Some("demo-mcp"));
    assert_eq!(listed.servers[0].env_keys, vec!["TOKEN"]);
    assert_eq!(listed.servers[0].metadata_keys, vec!["team"]);

    let toggled = toggle_managed_mcp_server_at_path(
        &config_path,
        ConfigScopeDto::Profile,
        "demo",
        false,
        false,
    )
    .expect("toggle should succeed");
    assert_eq!(toggled.status, "disabled");
    assert_eq!(toggled.enabled, Some(false));

    let enabled_only =
        build_mcp_server_list(&config, ConfigScopeDto::Profile, None, &[], false, false)
            .await
            .expect("filtered list should succeed");
    assert!(enabled_only.servers.is_empty());

    let removed =
        remove_managed_mcp_server_at_path(&config_path, ConfigScopeDto::Profile, "demo", false)
            .expect("remove should succeed");
    assert_eq!(removed.status, "removed");

    save_managed_mcp_server_at_path(&config_path, ConfigScopeDto::Profile, &request)
        .expect("save after remove should succeed");
    let reset = reset_managed_mcp_config_at_path(&config_path, ConfigScopeDto::Profile, false)
        .expect("reset should succeed");
    assert_eq!(reset.status, "reset");
    assert!(!config_path.exists());
}

#[test]
fn export_session_bundle_helper_writes_requested_formats() {
    let temp = tempdir().expect("tempdir should work");
    let paths = AppPaths::discover(Some(temp.path().join(".remote-code-rust"))).expect("paths");
    let store = SessionStore::open(paths).expect("store should open");
    let session_id = Uuid::new_v4();
    store
        .ensure_session(
            session_id,
            temp.path(),
            "glm-coding",
            Some("glm-5.1"),
            Some("Export parity"),
        )
        .expect("session should be created");
    store
        .append_named_event(
            session_id,
            "result",
            json!({
                "is_error": false,
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 8}
            }),
        )
        .expect("event should append");

    let json_export =
        export_session_bundle_for_store(&store, session_id, SessionExportFormatDto::Json)
            .expect("json export should succeed");
    assert!(Path::new(&json_export.path).exists());
    assert!(json_export.path.ends_with(".json"));

    let ndjson_export =
        export_session_bundle_for_store(&store, session_id, SessionExportFormatDto::Ndjson)
            .expect("ndjson export should succeed");
    assert!(Path::new(&ndjson_export.path).exists());
    assert!(ndjson_export.path.ends_with(".ndjson"));
}

#[test]
fn normalize_project_entries_deduplicates_equivalent_paths() {
    let projects = normalize_project_entries(vec![
        ProjectEntry {
            path: PathBuf::from(r"C:\Work\Alpha"),
            name: "Alpha".to_owned(),
        },
        ProjectEntry {
            path: PathBuf::from(r"C:\Work\Alpha\"),
            name: "Alpha Duplicate".to_owned(),
        },
    ]);

    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "Alpha");
}

#[test]
fn ensure_sessions_have_projects_promotes_orphan_session_folders() {
    let mut managed = vec![ProjectEntry {
        path: PathBuf::from(r"C:\Work\Alpha"),
        name: "Alpha".to_owned(),
    }];
    let sessions = vec![
        SessionSummary {
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            title: "alpha-session".to_owned(),
            cwd: PathBuf::from(r"C:\Work\Alpha"),
            provider_name: "glm".to_owned(),
            model: Some("glm-5.1".to_owned()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            transcript_path: PathBuf::from("alpha.ndjson"),
            archived: false,
        },
        SessionSummary {
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            title: "orphan-session".to_owned(),
            cwd: PathBuf::from(r"C:\Work\Beta"),
            provider_name: "minimax".to_owned(),
            model: Some("minimax-m2.7".to_owned()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            transcript_path: PathBuf::from("beta.ndjson"),
            archived: false,
        },
    ];

    assert!(ensure_sessions_have_projects(&mut managed, &sessions));
    assert_eq!(managed.len(), 2);
    assert!(managed.iter().any(|project| project.name == "Alpha"));
    assert!(managed.iter().any(|project| project.name == "Beta"));
}

#[test]
fn build_project_infos_groups_sessions_under_project_nodes() {
    let managed = vec![ProjectEntry {
        path: PathBuf::from(r"C:\Work\Alpha"),
        name: "Alpha".to_owned(),
    }];
    let sessions = vec![
        SessionSummary {
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            title: "alpha-session".to_owned(),
            cwd: PathBuf::from(r"C:\Work\Alpha"),
            provider_name: "glm".to_owned(),
            model: Some("glm-5.1".to_owned()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            transcript_path: PathBuf::from("alpha.ndjson"),
            archived: false,
        },
        SessionSummary {
            session_id: Uuid::new_v4(),
            parent_session_id: None,
            title: "orphan-session".to_owned(),
            cwd: PathBuf::from(r"C:\Work\Beta"),
            provider_name: "minimax".to_owned(),
            model: Some("minimax-m2.7".to_owned()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            transcript_path: PathBuf::from("beta.ndjson"),
            archived: false,
        },
    ];

    let projects = build_project_infos(&managed, &sessions);

    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "Alpha");
    assert_eq!(projects[0].session_count, 1);
}

// ── recv_with_liveness_check tests ─────────────────────────────

#[tokio::test]
async fn recv_with_liveness_returns_item_immediately() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);
    tx.send("hello".to_string())
        .await
        .expect("test channel should accept the initial message");
    drop(tx);

    let result = recv_with_liveness_check(&mut rx, || true).await;
    assert_eq!(result.expect("message should be received"), "hello");
}

#[tokio::test]
async fn supervised_turn_result_returns_completed_value() {
    let inner = tokio::spawn(async { Ok::<_, String>("done") });
    let result = session_commands::supervised_turn_result("Test", inner, |error| error).await;
    assert_eq!(
        result,
        session_commands::SupervisedTurnResult::Completed("done")
    );
}

#[tokio::test]
async fn supervised_turn_result_maps_task_error() {
    let inner = tokio::spawn(async { Err::<(), _>("provider failed".to_owned()) });
    let result = session_commands::supervised_turn_result("Test", inner, |error| error).await;
    assert_eq!(
        result,
        session_commands::SupervisedTurnResult::Failed("provider failed".to_owned())
    );
}

#[tokio::test]
async fn supervised_turn_result_maps_cancelled_join() {
    let inner = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        Ok::<_, String>(())
    });
    inner.abort();
    let result = session_commands::supervised_turn_result("Test", inner, |error| error).await;
    assert_eq!(result, session_commands::SupervisedTurnResult::Cancelled);
}

#[tokio::test]
async fn supervised_turn_result_maps_panic_join() {
    let inner = tokio::spawn(async {
        panic!("supervised turn panic");
        #[allow(unreachable_code)]
        Ok::<_, String>(())
    });
    let result = session_commands::supervised_turn_result("Test", inner, |error| error).await;
    match result {
        session_commands::SupervisedTurnResult::Failed(message) => {
            assert!(message.contains("Test agent panicked unexpectedly"));
        }
        other => panic!("expected failed supervised turn, got {other:?}"),
    }
}

#[tokio::test]
async fn recv_with_liveness_detects_closed_channel() {
    let (_tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);
    drop(_tx);

    let result = recv_with_liveness_check(&mut rx, || true).await;
    assert!(result.is_err());
    assert!(
        result
            .expect_err("closed channel should return an error")
            .contains("channel closed")
    );
}

#[tokio::test(start_paused = true)]
async fn recv_with_liveness_detects_dead_worker_on_timeout() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);
    // Keep channel open but empty. is_alive returns false, so when the
    // internal 30s timeout fires it should detect the dead worker.

    let handle = tokio::spawn(async move { recv_with_liveness_check(&mut rx, || false).await });

    // Advance virtual time past the 30s internal timeout
    tokio::time::advance(std::time::Duration::from_secs(31)).await;

    let result = handle.await.expect("task should complete");
    assert!(result.is_err());
    assert!(
        result
            .expect_err("dead worker should return an error")
            .contains("crashed")
    );
    drop(tx);
}

#[tokio::test]
async fn recv_with_liveness_retries_on_timeout_when_alive() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);

    // Send a value after a short delay — simulates a slow agent
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = tx_clone.send("delayed".to_string()).await;
    });

    let result = recv_with_liveness_check(&mut rx, || true).await;
    assert_eq!(
        result.expect("delayed message should be received"),
        "delayed"
    );
    drop(tx);
}

// ── MCP server overrides format tests ──

#[test]
fn codex_url_transport_uses_http_headers_key() {
    let mut server = serde_json::Map::new();
    let url = "https://mcp.example.com/sse";
    let headers = BTreeMap::from([("Authorization".to_owned(), "Bearer token".to_owned())]);
    let _transport = McpTransportConfig::Sse {
        url: url.to_owned(),
        headers: headers.clone(),
        headers_helper: None,
    };
    // Codex format: no "type", uses "http_headers"
    server.insert("url".to_owned(), serde_json::json!(url));
    if !headers.is_empty() {
        server.insert("http_headers".to_owned(), serde_json::json!(headers));
    }
    let val = serde_json::Value::Object(server);
    assert_eq!(val["url"], "https://mcp.example.com/sse");
    assert_eq!(val["http_headers"]["Authorization"], "Bearer token");
    assert!(
        val.get("type").is_none(),
        "Codex format should not have 'type'"
    );
}

#[test]
fn roo_http_transport_uses_streamable_http_type() {
    let mut server = serde_json::Map::new();
    let url = "https://mcp.example.com/mcp";
    let transport = McpTransportConfig::Http {
        url: url.to_owned(),
        headers: BTreeMap::new(),
        headers_helper: None,
    };
    // Roo format: "type": "streamable-http" for Http, uses "headers"
    let type_str = match &transport {
        McpTransportConfig::Http { .. } => "streamable-http",
        _ => "sse",
    };
    server.insert("type".to_owned(), serde_json::json!(type_str));
    server.insert("url".to_owned(), serde_json::json!(url));
    let val = serde_json::Value::Object(server);
    assert_eq!(val["type"], "streamable-http");
    assert_eq!(val["url"], "https://mcp.example.com/mcp");
}

#[test]
fn roo_sse_transport_uses_sse_type() {
    let mut server = serde_json::Map::new();
    let url = "https://mcp.example.com/events";
    let transport = McpTransportConfig::Sse {
        url: url.to_owned(),
        headers: BTreeMap::new(),
        headers_helper: None,
    };
    let type_str = match &transport {
        McpTransportConfig::Http { .. } => "streamable-http",
        _ => "sse",
    };
    server.insert("type".to_owned(), serde_json::json!(type_str));
    server.insert("url".to_owned(), serde_json::json!(url));
    let val = serde_json::Value::Object(server);
    assert_eq!(val["type"], "sse");
}

#[test]
fn roo_websocket_transport_uses_sse_type() {
    let mut server = serde_json::Map::new();
    let url = "wss://mcp.example.com/ws";
    let transport = McpTransportConfig::WebSocket {
        url: url.to_owned(),
        headers: BTreeMap::new(),
        headers_helper: None,
    };
    let type_str = match &transport {
        McpTransportConfig::Http { .. } => "streamable-http",
        _ => "sse",
    };
    server.insert("type".to_owned(), serde_json::json!(type_str));
    server.insert("url".to_owned(), serde_json::json!(url));
    let val = serde_json::Value::Object(server);
    assert_eq!(val["type"], "sse");
}

#[test]
fn roo_url_transport_uses_headers_key() {
    let mut server = serde_json::Map::new();
    let url = "https://mcp.example.com/sse";
    let headers = BTreeMap::from([("X-Api-Key".to_owned(), "key123".to_owned())]);
    server.insert("type".to_owned(), serde_json::json!("sse"));
    server.insert("url".to_owned(), serde_json::json!(url));
    if !headers.is_empty() {
        server.insert("headers".to_owned(), serde_json::json!(headers));
    }
    let val = serde_json::Value::Object(server);
    assert_eq!(val["headers"]["X-Api-Key"], "key123");
    assert!(
        val.get("http_headers").is_none(),
        "Roo format uses 'headers' not 'http_headers'"
    );
}

#[test]
fn stdio_transport_extracts_command_and_args() {
    let config = sample_stdio_mcp_server("test-server", true, "npx");
    match &config.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &vec!["--serve".to_owned()]);
            assert!(cwd.is_none());
            assert!(env.is_empty());
        }
        other => panic!("expected Stdio transport, got {other:?}"),
    }
}
