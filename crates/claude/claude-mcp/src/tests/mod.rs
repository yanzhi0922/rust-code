//! Integration tests for the rc-mcp crate.
//!
//! Migrated from the original `lib.rs` inline tests, plus new tests for
//! the additional modules introduced during the Phase 5a refactoring.

use std::collections::BTreeMap;
use std::fs;

use std::process::Command as ProcessCommand;

use tempfile::tempdir;

use crate::config::{McpCapabilityMatrix, McpConfig, McpServerConfig};
use crate::error::{McpConfigError, McpRuntimeError};
use crate::session::{
    DEFAULT_MCP_CONFIG_FILE, call_tool, discover_mcp_configs, get_prompt, inspect_server,
    load_discovered_mcp_configs,
};
use crate::transport::{McpTransport, McpTransportConfig};
use crate::types::McpClientInfo;

fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("unexpected error: {error}"),
    }
}

// ── Original tests (migrated from lib.rs) ───────────────────────────────────

#[test]
fn parses_stdio_and_http_servers() {
    let config = ok(McpConfig::from_toml_str(
        r#"
            [mcp_servers.brave]
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-brave-search"]
            startup_timeout_secs = 5

            [mcp_servers.brave.env]
            BRAVE_API_KEY = "secret"

            [mcp_servers.context7]
            url = "https://mcp.context7.com/mcp"
            enabled = false
            request_timeout_secs = 15

            [mcp_servers.context7.http_headers]
            Authorization = "Bearer test"

            [mcp_servers.context7.capabilities]
            tools = true
            resources = true
        "#,
    ));

    let brave = match config.servers.get("brave") {
        Some(server) => server,
        None => panic!("missing brave server"),
    };
    assert!(brave.enabled);
    assert_eq!(brave.transport.kind(), McpTransport::Stdio);
    assert_eq!(brave.startup_timeout_secs, Some(5));

    let context7 = match config.servers.get("context7") {
        Some(server) => server,
        None => panic!("missing context7 server"),
    };
    assert!(!context7.enabled);
    assert_eq!(context7.transport.kind(), McpTransport::Http);
    assert!(context7.capabilities.supports_tools);
    assert!(context7.capabilities.supports_resources);
}

#[test]
fn parses_websocket_server() {
    let config = ok(McpConfig::from_toml_str(
        r#"
            [mcp_servers.relay]
            url = "wss://example.com/mcp"
        "#,
    ));

    let relay = match config.servers.get("relay") {
        Some(server) => server,
        None => panic!("missing relay server"),
    };
    assert_eq!(relay.transport.kind(), McpTransport::WebSocket);
}

#[test]
fn rejects_server_without_transport() {
    let error = McpConfig::from_toml_str(
        r"
            [mcp_servers.invalid]
            enabled = true
        ",
    )
    .expect_err("server without transport should fail");

    assert!(matches!(
        error,
        McpConfigError::MissingTransport { ref name } if name == "invalid"
    ));
}

#[test]
fn discovers_and_loads_configs() {
    let temp = ok(tempdir());
    let root = temp.path();
    let nested = root.join("plugins").join("example");
    ok(fs::create_dir_all(&nested));
    ok(fs::write(
        root.join(DEFAULT_MCP_CONFIG_FILE),
        "[mcp_servers.one]\ncommand = \"uvx\"\n",
    ));
    ok(fs::write(
        nested.join(DEFAULT_MCP_CONFIG_FILE),
        "[mcp_servers.two]\nurl = \"https://example.com/mcp\"\n",
    ));

    let discovered = discover_mcp_configs(root);
    assert_eq!(discovered.len(), 2);

    let loaded = ok(load_discovered_mcp_configs(root));
    assert_eq!(loaded.len(), 2);
    assert!(
        loaded
            .iter()
            .any(|entry| entry.config.servers.contains_key("one"))
    );
    assert!(
        loaded
            .iter()
            .any(|entry| entry.config.servers.contains_key("two"))
    );
}

#[test]
fn saves_config_in_round_trip_format() {
    let temp = ok(tempdir());
    let path = temp.path().join(DEFAULT_MCP_CONFIG_FILE);
    let config = McpConfig {
        servers: BTreeMap::from([
            (
                "demo".to_owned(),
                McpServerConfig {
                    name: "demo".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "python".to_owned(),
                        args: vec!["server.py".to_owned()],
                        cwd: Some(temp.path().join("demo")),
                        env: BTreeMap::from([("TOKEN".to_owned(), "secret".to_owned())]),
                    },
                    capabilities: McpCapabilityMatrix {
                        supports_tools: true,
                        ..McpCapabilityMatrix::default()
                    },
                    startup_timeout_secs: Some(3),
                    request_timeout_secs: Some(5),
                    metadata: BTreeMap::from([("scope".to_owned(), "local".to_owned())]),
                    oauth: None,
                    tool_policy: crate::tool_policy::McpToolPolicy::default(),
                },
            ),
            (
                "remote".to_owned(),
                McpServerConfig {
                    name: "remote".to_owned(),
                    enabled: false,
                    transport: McpTransportConfig::Http {
                        url: "https://example.com/mcp".to_owned(),
                        headers: BTreeMap::from([(
                            "Authorization".to_owned(),
                            "Bearer token".to_owned(),
                        )]),
                        headers_helper: None,
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: BTreeMap::new(),
                    oauth: None,
                    tool_policy: crate::tool_policy::McpToolPolicy::default(),
                },
            ),
        ]),
    };

    config
        .save(&path)
        .unwrap_or_else(|error| panic!("save failed: {error}"));
    let loaded = McpConfig::load(&path).unwrap_or_else(|error| panic!("load failed: {error}"));
    assert_eq!(loaded.servers.len(), 2);
    assert_eq!(loaded.servers["demo"].transport.kind(), McpTransport::Stdio);
    assert_eq!(
        loaded.servers["remote"].transport.kind(),
        McpTransport::Http
    );
}

#[tokio::test]
async fn inspects_stdio_server_and_lists_tools() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP stdio inspection test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_mcp.py");
    ok(fs::write(
        &script,
        r#"
import json
import sys

def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

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
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.1.0"},
                "instructions": "Use mock tools"
            }
        })
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "tools": [
                    {
                        "name": "search",
                        "description": "Search indexed documentation",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {"type": "string"}
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "fetch",
                        "description": "Fetch a resource by URL",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "url": {"type": "string"}
                            }
                        }
                    }
                ]
            }
        })
    else:
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {"code": -32601, "message": "unknown method"}
        })
"#,
    ));
    prefix_args.push(script.to_string_lossy().into_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let inspection = inspect_server(&server, &McpClientInfo::new("remote-code-rust", "test"))
        .await
        .unwrap_or_else(|error| panic!("inspection failed: {error}"));

    assert_eq!(inspection.server_name, "mock");
    assert_eq!(inspection.protocol_version, "2025-03-26");
    assert_eq!(
        inspection
            .server_info
            .as_ref()
            .map(|info| info.name.as_str()),
        Some("mock-mcp")
    );
    assert_eq!(inspection.tools.len(), 2);
    assert_eq!(inspection.tools[0].name, "search");
    assert_eq!(
        inspection.tools[0].description.as_deref(),
        Some("Search indexed documentation")
    );
    assert!(inspection.resources.is_empty());
    assert!(inspection.prompts.is_empty());
}

#[tokio::test]
async fn inspection_lists_prompts_and_get_prompt_returns_messages() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP stdio prompt inspection test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_mcp_prompts.py");
    ok(fs::write(&script, mock_prompt_server_script()));
    prefix_args.push(script.to_string_lossy().into_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let inspection = inspect_server(&server, &McpClientInfo::new("remote-code-rust", "test"))
        .await
        .unwrap_or_else(|error| panic!("inspection failed: {error}"));

    assert_eq!(inspection.prompts.len(), 1);
    assert_eq!(inspection.prompts[0].name, "plan");
    assert_eq!(inspection.prompts[0].arguments.len(), 1);
    assert_eq!(inspection.prompts[0].arguments[0].name, "topic");
    assert!(inspection.prompts[0].arguments[0].required);

    let response = get_prompt(
        &server,
        &McpClientInfo::new("remote-code-rust", "test"),
        "plan",
        serde_json::json!({"topic": "MCP"}),
    )
    .await
    .unwrap_or_else(|error| panic!("get prompt failed: {error}"));

    assert_eq!(response.prompt_name, "plan");
    assert_eq!(
        response.result.description.as_deref(),
        Some("Planning prompt")
    );
    assert_eq!(response.result.messages.len(), 1);
    assert_eq!(response.result.messages[0].role, "user");
    assert_eq!(response.result.messages[0].content["text"], "Plan MCP");
}

#[tokio::test]
async fn inspection_lists_advertised_resources() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP stdio resource inspection test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_mcp_resources.py");
    ok(fs::write(&script, mock_resource_server_script("list")));
    prefix_args.push(script.to_string_lossy().into_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let inspection = inspect_server(&server, &McpClientInfo::new("remote-code-rust", "test"))
        .await
        .unwrap_or_else(|error| panic!("inspection failed: {error}"));

    assert_eq!(inspection.tools.len(), 1);
    assert_eq!(inspection.resources.len(), 2);
    assert_eq!(inspection.resources[0].uri, "file:///workspace/README.md");
    assert_eq!(inspection.resources[0].server, "mock");
    assert_eq!(inspection.resources[0].name.as_deref(), Some("Readme"));
    assert_eq!(
        inspection.resources[0].mime_type.as_deref(),
        Some("text/markdown")
    );
    assert_eq!(
        inspection.resources[1].description.as_deref(),
        Some("Project metadata")
    );
}

#[tokio::test]
async fn inspection_tolerates_unsupported_advertised_resources() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP stdio unsupported resources test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_mcp_resources.py");
    ok(fs::write(
        &script,
        mock_resource_server_script("unsupported"),
    ));
    prefix_args.push(script.to_string_lossy().into_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let inspection = inspect_server(&server, &McpClientInfo::new("remote-code-rust", "test"))
        .await
        .unwrap_or_else(|error| {
            panic!("inspection should tolerate unsupported resources: {error}")
        });

    assert_eq!(inspection.tools.len(), 1);
    assert!(inspection.resources.is_empty());
}

#[tokio::test]
async fn calls_stdio_tool_and_returns_typed_result() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP stdio tool call test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_tool_call.py");
    ok(fs::write(&script, mock_tool_call_server_script()));
    prefix_args.push(script.to_string_lossy().into_owned());
    prefix_args.push("success".to_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let response = call_tool(
        &server,
        &McpClientInfo::new("remote-code-rust", "test"),
        "echo",
        serde_json::json!({"text": "hello"}),
    )
    .await
    .unwrap_or_else(|error| panic!("tool call failed: {error}"));

    assert_eq!(response.server_name, "mock");
    assert_eq!(response.tool_name, "echo");
    assert_eq!(response.protocol_version, "2025-03-26");
    assert_eq!(
        response.server_info.as_ref().map(|info| info.name.as_str()),
        Some("mock-mcp")
    );
    assert!(!response.result.is_error);
    assert_eq!(response.result.content.len(), 1);
    assert_eq!(response.result.content[0].kind, "text");
    assert_eq!(
        response.result.content[0]
            .fields
            .get("text")
            .and_then(serde_json::Value::as_str),
        Some("echo: hello")
    );
    assert_eq!(
        response.result.structured_content,
        Some(serde_json::json!({"echoed": "hello"}))
    );
}

#[tokio::test]
async fn preserves_tool_error_payloads() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP tool error payload test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_tool_call.py");
    ok(fs::write(&script, mock_tool_call_server_script()));
    prefix_args.push(script.to_string_lossy().into_owned());
    prefix_args.push("tool_error".to_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let response = call_tool(
        &server,
        &McpClientInfo::new("remote-code-rust", "test"),
        "echo",
        serde_json::json!({"text": "boom"}),
    )
    .await
    .unwrap_or_else(|error| panic!("tool error payload should remain typed: {error}"));

    assert!(response.result.is_error);
    assert_eq!(
        response.result.content[0]
            .fields
            .get("text")
            .and_then(serde_json::Value::as_str),
        Some("tool execution failed")
    );
}

#[tokio::test]
async fn surfaces_json_rpc_errors_from_tool_call() {
    let Some((python, mut prefix_args)) = python_command() else {
        eprintln!("Skipping MCP JSON-RPC error test because Python is unavailable.");
        return;
    };

    let temp = ok(tempdir());
    let script = temp.path().join("mock_tool_call.py");
    ok(fs::write(&script, mock_tool_call_server_script()));
    prefix_args.push(script.to_string_lossy().into_owned());
    prefix_args.push("rpc_error".to_owned());

    let server = McpServerConfig {
        name: "mock".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: python,
            args: prefix_args,
            cwd: Some(temp.path().to_path_buf()),
            env: BTreeMap::new(),
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: Some(3),
        request_timeout_secs: Some(3),
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let error = call_tool(
        &server,
        &McpClientInfo::new("remote-code-rust", "test"),
        "echo",
        serde_json::json!({"text": "boom"}),
    )
    .await
    .expect_err("JSON-RPC tool call failure should surface as runtime error");

    assert!(matches!(
        error,
        McpRuntimeError::Rpc {
            code: -32001,
            ref message,
            ..
        } if message == "tool call failed"
    ));
}

#[tokio::test]
async fn http_transport_attempts_connection() {
    // HTTP transport is now supported — connecting to a non-existent server
    // should produce an HTTP-level error (connection refused / DNS failure),
    // not UnsupportedTransport.
    let server = McpServerConfig {
        name: "relay".to_owned(),
        enabled: true,
        transport: McpTransportConfig::Http {
            url: "https://example.com/mcp".to_owned(),
            headers: BTreeMap::new(),
            headers_helper: None,
        },
        capabilities: McpCapabilityMatrix::default(),
        startup_timeout_secs: None,
        request_timeout_secs: None,
        metadata: BTreeMap::new(),
        oauth: None,
        tool_policy: crate::tool_policy::McpToolPolicy::default(),
    };

    let error = inspect_server(&server, &McpClientInfo::default())
        .await
        .expect_err("connecting to a fake URL should fail");
    // The error should be an HTTP or JSON-RPC error, not UnsupportedTransport.
    assert!(
        matches!(
            error,
            McpRuntimeError::Http { .. }
                | McpRuntimeError::HttpError { .. }
                | McpRuntimeError::JsonRpc { .. }
        ),
        "expected HTTP/JSON-RPC error for fake URL, got: {error:?}"
    );
}

// ── New integration tests for additional modules ────────────────────────────

#[test]
fn connection_states_have_correct_type_tags() {
    use crate::connection::{
        ConnectedServer, DisabledServer, FailedServer, McpServerConnection, NeedsAuthServer,
        PendingServer,
    };
    use crate::scope::{ConfigScope, ScopedMcpServerConfig};

    let scoped = ScopedMcpServerConfig::new(
        McpServerConfig {
            name: "test".to_owned(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "echo".to_owned(),
                args: vec![],
                cwd: None,
                env: BTreeMap::new(),
            },
            capabilities: McpCapabilityMatrix::default(),
            startup_timeout_secs: None,
            request_timeout_secs: None,
            metadata: BTreeMap::new(),
            oauth: None,
            tool_policy: crate::tool_policy::McpToolPolicy::default(),
        },
        ConfigScope::Local,
    );

    let states: Vec<McpServerConnection> = vec![
        McpServerConnection::Connected(ConnectedServer {
            name: "a".to_owned(),
            capabilities: McpCapabilityMatrix::default(),
            server_info: None,
            instructions: None,
            config: scoped.clone(),
        }),
        McpServerConnection::Failed(FailedServer {
            name: "b".to_owned(),
            config: scoped.clone(),
            error: None,
        }),
        McpServerConnection::NeedsAuth(NeedsAuthServer {
            name: "c".to_owned(),
            config: scoped.clone(),
        }),
        McpServerConnection::Pending(PendingServer {
            name: "d".to_owned(),
            config: scoped.clone(),
            reconnect_attempt: None,
            max_reconnect_attempts: None,
        }),
        McpServerConnection::Disabled(DisabledServer {
            name: "e".to_owned(),
            config: scoped,
        }),
    ];

    let types: Vec<&str> = states
        .iter()
        .map(|s: &McpServerConnection| s.connection_type())
        .collect();
    assert_eq!(
        types,
        vec!["connected", "failed", "needs-auth", "pending", "disabled"]
    );
}

#[test]
fn normalization_roundtrip_for_tool_names() {
    use crate::normalization::{build_mcp_tool_name, mcp_info_from_string, normalize_name_for_mcp};

    let server = normalize_name_for_mcp("my server/v2");
    let tool = normalize_name_for_mcp("search & fetch");
    let full = build_mcp_tool_name(&server, &tool);
    let info = mcp_info_from_string(&full).expect("should parse");
    assert_eq!(info.server_name, "my_server_v2");
    assert_eq!(info.tool_name, "search___fetch");
}

#[test]
fn env_expansion_with_defaults() {
    use crate::env_expansion::expand_env_vars;

    let result = expand_env_vars("${RC_MCP_NONEXISTENT_VAR_XYZ:-default_value}");
    assert_eq!(result.expanded, "default_value");
    assert!(result.missing_vars.is_empty());
}

#[test]
fn scope_ordering() {
    use crate::scope::ConfigScope;

    assert!(ConfigScope::Managed.precedence() > ConfigScope::Enterprise.precedence());
    assert!(ConfigScope::Enterprise.precedence() > ConfigScope::Local.precedence());
}

#[test]
fn transport_kind_covers_all_variants() {
    use crate::transport::McpTransportKind;

    let all = [
        McpTransportKind::Stdio,
        McpTransportKind::Sse,
        McpTransportKind::SseIde,
        McpTransportKind::Http,
        McpTransportKind::WebSocket,
        McpTransportKind::WsIde,
        McpTransportKind::Sdk,
        McpTransportKind::ClaudeAiProxy,
    ];

    for kind in all {
        let s = serde_json::to_string(&kind).expect("serialize");
        let back: McpTransportKind = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(kind, back, "roundtrip failed for {kind:?}");
    }
}

#[test]
fn serialized_cli_state_json_output() {
    use crate::serialization::{McpCliState, SerializedClient, SerializedTool};

    let state = McpCliState {
        clients: vec![SerializedClient {
            name: "test-server".to_owned(),
            connection_type: "connected".to_owned(),
            capabilities: Some(McpCapabilityMatrix {
                supports_tools: true,
                ..McpCapabilityMatrix::default()
            }),
        }],
        tools: vec![SerializedTool {
            name: "search".to_owned(),
            description: "Search".to_owned(),
            input_json_schema: None,
            is_mcp: Some(true),
            original_tool_name: None,
        }],
        ..McpCliState::default()
    };

    let json = serde_json::to_string_pretty(&state).expect("serialize");
    assert!(json.contains("\"test-server\""));
    assert!(json.contains("\"connected\""));
    assert!(json.contains("\"search\""));
}

#[test]
fn server_resource_builder() {
    use crate::resources::ServerResource;

    let res = ServerResource::new("file:///data.csv", "my-server")
        .with_name("Data")
        .with_description("CSV data file")
        .with_mime_type("text/csv");

    assert_eq!(res.uri, "file:///data.csv");
    assert_eq!(res.name.as_deref(), Some("Data"));
    assert_eq!(res.mime_type.as_deref(), Some("text/csv"));
}

// ── Helpers ─────────────────────────────────────────────────────────────────

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

mode = sys.argv[1]

def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

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
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.1.0"}
            }
        })
    elif method == "notifications/initialized":
        continue
    elif method == "tools/call":
        if mode == "rpc_error":
            send({
                "jsonrpc": "2.0",
                "id": message_id,
                "error": {"code": -32001, "message": "tool call failed"}
            })
        elif mode == "tool_error":
            send({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "content": [
                        {"type": "text", "text": "tool execution failed"}
                    ],
                    "isError": True
                }
            })
        else:
            text = message.get("params", {}).get("arguments", {}).get("text", "")
            send({
                "jsonrpc": "2.0",
                "id": message_id,
                "result": {
                    "content": [
                        {"type": "text", "text": f"echo: {text}"}
                    ],
                    "structuredContent": {
                        "echoed": text
                    },
                    "isError": False
                }
            })
"#
}

fn mock_resource_server_script(mode: &str) -> String {
    format!(
        r#"
import json
import sys

mode = {mode:?}

def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

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
        send({{
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {{
                "protocolVersion": "2025-03-26",
                "capabilities": {{"tools": {{}}, "resources": {{}}}},
                "serverInfo": {{"name": "mock-mcp", "version": "0.1.0"}}
            }}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({{
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {{
                "tools": [
                    {{"name": "fetch", "description": "Fetch", "inputSchema": {{}}}}
                ]
            }}
        }})
    elif method == "resources/list" and mode == "list":
        send({{
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {{
                "resources": [
                    {{
                        "uri": "file:///workspace/README.md",
                        "name": "Readme",
                        "description": "Workspace readme",
                        "mimeType": "text/markdown"
                    }},
                    {{
                        "uri": "file:///workspace/package.json",
                        "description": "Project metadata"
                    }}
                ]
            }}
        }})
    elif method == "resources/list":
        send({{
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {{"code": -32601, "message": "method not found"}}
        }})
    else:
        send({{
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {{"code": -32601, "message": "unknown method"}}
        }})
"#
    )
}

fn mock_prompt_server_script() -> &'static str {
    r#"
import json
import sys

def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

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
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}, "prompts": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.1.0"}
            }
        })
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {"tools": []}
        })
    elif method == "prompts/list":
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "prompts": [
                    {
                        "name": "plan",
                        "description": "Planning prompt",
                        "arguments": [
                            {"name": "topic", "required": True}
                        ]
                    }
                ]
            }
        })
    elif method == "prompts/get":
        topic = message.get("params", {}).get("arguments", {}).get("topic", "")
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "result": {
                "description": "Planning prompt",
                "messages": [
                    {"role": "user", "content": {"type": "text", "text": f"Plan {topic}"}}
                ]
            }
        })
    else:
        send({
            "jsonrpc": "2.0",
            "id": message_id,
            "error": {"code": -32601, "message": "unknown method"}
        })
"#
}
