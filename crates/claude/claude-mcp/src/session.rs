//! MCP session management for stdio, SSE, and HTTP transports.
//!
//! Handles spawning MCP server processes (stdio), connecting to remote MCP
//! servers via SSE (Server-Sent Events) and HTTP Streamable transports,
//! performing the initialization handshake, listing tools, and invoking tools
//! over JSON-RPC.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite};
use walkdir::WalkDir;

use crate::config::{McpConfig, McpServerConfig};
use crate::error::{McpConfigError, McpRuntimeError};
use crate::jsonrpc::{
    InitializeParams, JsonRpcEnvelope, JsonRpcNotification, JsonRpcRequest, McpInitializeResult,
    McpPromptGetRpcResult, McpPromptsListResult, McpResourceContent, McpResourceReadResult,
    McpResourcesListResult, McpToolsListResult, PromptGetParams, ResourceReadParams,
    ToolCallParams, rpc_id_matches,
};
use crate::resources::ServerResource;
use crate::transport::McpTransportConfig;
use crate::types::{
    McpClientInfo, McpPromptDescriptor, McpPromptGetResponse, McpServerInspection,
    McpToolCallResponse, McpToolCallResult,
};

/// Default MCP protocol version used during initialisation.
pub const DEFAULT_MCP_PROTOCOL_VERSION: &str = "2025-03-26";
/// Default timeout for MCP server startup in seconds.
pub const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 10;
/// Default timeout for individual MCP requests in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 15;
/// Default legacy MCP config file name.
pub const DEFAULT_MCP_CONFIG_FILE: &str = "mcp.toml";
/// Default Claude-compatible project MCP config file name.
pub const DEFAULT_PROJECT_MCP_CONFIG_FILE: &str = ".mcp.json";

/// An active stdio MCP session managing a child process.
pub(crate) struct StdioMcpSession {
    server_name: String,
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    initialized: McpInitializeResult,
    request_timeout_secs: u64,
}

/// Inspect an MCP server: initialize, list tools, and return the inspection result.
pub async fn inspect_server(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
) -> Result<McpServerInspection, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => inspect_stdio_server(server, command, args, cwd.as_deref(), env, client_info).await,
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.inspect_server().await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.inspect_server().await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.inspect_server().await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.inspect_server().await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.inspect_server().await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// Call a tool on an MCP server.
pub async fn call_tool(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
    tool_name: &str,
    arguments: Value,
) -> Result<McpToolCallResponse, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut session =
                StdioMcpSession::connect(server, command, args, cwd.as_deref(), env, client_info)
                    .await?;
            let result = session.call_tool(tool_name, arguments).await;
            session.shutdown().await;
            result
        }
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.call_tool(tool_name, arguments).await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.call_tool(tool_name, arguments).await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.call_tool(tool_name, arguments).await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.call_tool(tool_name, arguments).await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.call_tool(tool_name, arguments).await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// List resources exposed by an MCP server.
///
/// Connects to the server via stdio, sends `resources/list`, and returns
/// the available resources.
pub async fn list_resources(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
) -> Result<Vec<ServerResource>, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut session =
                StdioMcpSession::connect(server, command, args, cwd.as_deref(), env, client_info)
                    .await?;
            let result = session.list_resources().await;
            session.shutdown().await;
            result
        }
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.list_resources().await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.list_resources().await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.list_resources().await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.list_resources().await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.list_resources().await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// List prompts exposed by an MCP server.
pub async fn list_prompts(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
) -> Result<Vec<McpPromptDescriptor>, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut session =
                StdioMcpSession::connect(server, command, args, cwd.as_deref(), env, client_info)
                    .await?;
            let result = session.list_prompts().await;
            session.shutdown().await;
            result
        }
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.list_prompts().await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.list_prompts().await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.list_prompts().await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.list_prompts().await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.list_prompts().await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// Get a prompt from an MCP server.
pub async fn get_prompt(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
    prompt_name: &str,
    arguments: Value,
) -> Result<McpPromptGetResponse, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut session =
                StdioMcpSession::connect(server, command, args, cwd.as_deref(), env, client_info)
                    .await?;
            let result = session.get_prompt(prompt_name, arguments).await;
            session.shutdown().await;
            result
        }
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.get_prompt(prompt_name, arguments).await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.get_prompt(prompt_name, arguments).await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.get_prompt(prompt_name, arguments).await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.get_prompt(prompt_name, arguments).await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.get_prompt(prompt_name, arguments).await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// Read a resource from an MCP server.
///
/// Connects to the server, sends `resources/read`, and returns
/// the resource content.
pub async fn read_resource(
    server: &McpServerConfig,
    client_info: &McpClientInfo,
    uri: &str,
) -> Result<Vec<McpResourceContent>, McpRuntimeError> {
    match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            let mut session =
                StdioMcpSession::connect(server, command, args, cwd.as_deref(), env, client_info)
                    .await?;
            let result = session.read_resource(uri).await;
            session.shutdown().await;
            result
        }
        McpTransportConfig::Sse { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, headers, client_info).await?;
            session.read_resource(uri).await
        }
        McpTransportConfig::Http { url, headers, .. } => {
            let mut session =
                RemoteMcpSession::connect_http(server, url, headers, client_info).await?;
            session.read_resource(uri).await
        }
        McpTransportConfig::WebSocket { url, headers, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, headers, client_info).await?;
            session.read_resource(uri).await
        }
        McpTransportConfig::SseIde { url, .. } => {
            let mut session =
                RemoteMcpSession::connect_sse(server, url, &BTreeMap::new(), client_info).await?;
            session.read_resource(uri).await
        }
        McpTransportConfig::WsIde { url, .. } => {
            let mut session =
                WebSocketMcpSession::connect(server, url, &BTreeMap::new(), client_info).await?;
            session.read_resource(uri).await
        }
        McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
            Err(McpRuntimeError::UnsupportedTransport {
                server: server.name.clone(),
                transport: server.transport.kind(),
            })
        }
    }
}

/// Discover MCP configuration files under a root directory.
pub fn discover_mcp_configs(root: &Path) -> Vec<PathBuf> {
    let mut configs = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry.file_name() == DEFAULT_MCP_CONFIG_FILE
                || entry.file_name() == DEFAULT_PROJECT_MCP_CONFIG_FILE
        })
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    configs.sort();
    configs
}

/// Discover and load all MCP configuration files under a root directory.
pub fn load_discovered_mcp_configs(
    root: &Path,
) -> Result<Vec<crate::config::DiscoveredMcpConfig>, McpConfigError> {
    discover_mcp_configs(root)
        .into_iter()
        .map(|path| {
            let config = McpConfig::load(&path)?;
            Ok(crate::config::DiscoveredMcpConfig { path, config })
        })
        .collect()
}

async fn inspect_stdio_server(
    server: &McpServerConfig,
    command: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
    client_info: &McpClientInfo,
) -> Result<McpServerInspection, McpRuntimeError> {
    let mut session =
        StdioMcpSession::connect(server, command, args, cwd, env, client_info).await?;
    let result = session.inspect_server().await;
    session.shutdown().await;
    result
}

pub fn resolve_stdio_command(command: &str) -> String {
    #[cfg(windows)]
    {
        let path = Path::new(command);
        if path.extension().is_some() || path.components().count() > 1 {
            return command.to_owned();
        }

        for candidate in [
            format!("{command}.exe"),
            format!("{command}.cmd"),
            format!("{command}.bat"),
        ] {
            if let Ok(output) = std::process::Command::new("where.exe")
                .arg(&candidate)
                .output()
                && output.status.success()
                && let Some(first_match) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
            {
                return first_match.to_owned();
            }
        }
    }

    command.to_owned()
}

impl StdioMcpSession {
    async fn connect(
        server: &McpServerConfig,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: &BTreeMap<String, String>,
        client_info: &McpClientInfo,
    ) -> Result<Self, McpRuntimeError> {
        let resolved_command = resolve_stdio_command(command);
        let mut process = Command::new(&resolved_command);
        process
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()) // capture stderr for logging instead of discarding
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        if !env.is_empty() {
            process.envs(env);
        }

        let mut child = process.spawn().map_err(|source| McpRuntimeError::Spawn {
            server: server.name.clone(),
            command: resolved_command.clone(),
            source,
        })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpRuntimeError::MissingPipe {
                server: server.name.clone(),
                pipe: "stdin",
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpRuntimeError::MissingPipe {
                server: server.name.clone(),
                pipe: "stdout",
            })?;

        // Spawn a background task to log stderr from the MCP server process.
        // This ensures diagnostic output is captured rather than silently dropped.
        if let Some(stderr) = child.stderr.take() {
            let server_name = server.name.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(
                        target: "claude_mcp::session::stderr",
                        server = %server_name,
                        "MCP server stderr: {line}"
                    );
                }
            });
        }

        let mut lines = BufReader::new(stdout).lines();
        let startup_timeout = server
            .startup_timeout_secs
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECS);
        let request_timeout_secs = server
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);

        let initialize = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: InitializeParams {
                protocol_version: DEFAULT_MCP_PROTOCOL_VERSION,
                capabilities: serde_json::json!({}),
                client_info,
            },
        };
        write_message(&mut stdin, &server.name, "initialize request", &initialize).await?;
        let initialized: McpInitializeResult = wait_for_response(
            &mut lines,
            &server.name,
            1,
            "initialize response",
            startup_timeout,
        )
        .await?;

        let ready = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: serde_json::json!({}),
        };
        write_message(&mut stdin, &server.name, "initialized notification", &ready).await?;

        Ok(Self {
            server_name: server.name.clone(),
            child,
            stdin,
            lines,
            initialized,
            request_timeout_secs,
        })
    }

    async fn inspect_server(&mut self) -> Result<McpServerInspection, McpRuntimeError> {
        let tools_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "tools/list",
            params: serde_json::json!({}),
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "tools/list request",
            &tools_request,
        )
        .await?;
        let tools: McpToolsListResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            2,
            "tools/list response",
            self.request_timeout_secs,
        )
        .await?;
        let resources = if self.supports_resources() {
            match self.list_resources().await {
                Ok(resources) => resources,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };
        let prompts = if self.supports_prompts() {
            match self.list_prompts().await {
                Ok(prompts) => prompts,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };

        Ok(McpServerInspection {
            server_name: self.server_name.clone(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            capabilities: self.initialized.capabilities.clone(),
            instructions: self.initialized.instructions.clone(),
            tools: tools.tools,
            prompts,
            resources,
        })
    }

    async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResponse, McpRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "tools/call",
            params: ToolCallParams {
                name: tool_name,
                arguments,
            },
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "tools/call request",
            &request,
        )
        .await?;
        let mut result: McpToolCallResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            2,
            "tools/call response",
            self.request_timeout_secs,
        )
        .await?;

        // Truncate oversized tool results.
        crate::types::truncate_tool_call_result(&mut result);

        Ok(McpToolCallResponse {
            server_name: self.server_name.clone(),
            tool_name: tool_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// List prompts exposed by this MCP server.
    async fn list_prompts(&mut self) -> Result<Vec<McpPromptDescriptor>, McpRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 5,
            method: "prompts/list",
            params: serde_json::json!({}),
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "prompts/list request",
            &request,
        )
        .await?;
        let result: McpPromptsListResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            5,
            "prompts/list response",
            self.request_timeout_secs,
        )
        .await?;
        Ok(result.prompts)
    }

    /// Get a prompt from this MCP server.
    async fn get_prompt(
        &mut self,
        prompt_name: &str,
        arguments: Value,
    ) -> Result<McpPromptGetResponse, McpRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 6,
            method: "prompts/get",
            params: PromptGetParams {
                name: prompt_name.to_owned(),
                arguments,
            },
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "prompts/get request",
            &request,
        )
        .await?;
        let result: McpPromptGetRpcResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            6,
            "prompts/get response",
            self.request_timeout_secs,
        )
        .await?;

        Ok(McpPromptGetResponse {
            server_name: self.server_name.clone(),
            prompt_name: prompt_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// List resources exposed by this MCP server.
    async fn list_resources(&mut self) -> Result<Vec<ServerResource>, McpRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 3,
            method: "resources/list",
            params: serde_json::json!({}),
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "resources/list request",
            &request,
        )
        .await?;
        let result: McpResourcesListResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            3,
            "resources/list response",
            self.request_timeout_secs,
        )
        .await?;

        let resources = result
            .resources
            .into_iter()
            .map(|r| {
                let mut sr = ServerResource::new(r.uri, &self.server_name);
                sr.name = r.name;
                sr.description = r.description;
                sr.mime_type = r.mime_type;
                sr
            })
            .collect();
        Ok(resources)
    }

    /// Read a resource from this MCP server.
    async fn read_resource(
        &mut self,
        uri: &str,
    ) -> Result<Vec<McpResourceContent>, McpRuntimeError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 4,
            method: "resources/read",
            params: ResourceReadParams {
                uri: uri.to_owned(),
            },
        };
        write_message(
            &mut self.stdin,
            &self.server_name,
            "resources/read request",
            &request,
        )
        .await?;
        let result: McpResourceReadResult = wait_for_response(
            &mut self.lines,
            &self.server_name,
            4,
            "resources/read response",
            self.request_timeout_secs,
        )
        .await?;
        Ok(result.contents)
    }

    async fn shutdown(&mut self) {
        shutdown_child(&mut self.child).await;
    }

    fn supports_resources(&self) -> bool {
        self.initialized
            .capabilities
            .get("resources")
            .is_some_and(|resources| !resources.is_null() && resources != false)
    }

    fn supports_prompts(&self) -> bool {
        self.initialized
            .capabilities
            .get("prompts")
            .is_some_and(|prompts| !prompts.is_null() && prompts != false)
    }
}

fn is_unsupported_method_error(error: &McpRuntimeError) -> bool {
    matches!(error, McpRuntimeError::Rpc { code: -32601, .. })
}

async fn write_message<T: Serialize>(
    stdin: &mut ChildStdin,
    server: &str,
    phase: &'static str,
    payload: &T,
) -> Result<(), McpRuntimeError> {
    let mut body = serde_json::to_vec(payload).map_err(|source| McpRuntimeError::Serialize {
        server: server.to_owned(),
        phase,
        source,
    })?;
    body.push(b'\n');
    stdin
        .write_all(&body)
        .await
        .map_err(|source| McpRuntimeError::Write {
            server: server.to_owned(),
            phase,
            source,
        })?;
    stdin
        .flush()
        .await
        .map_err(|source| McpRuntimeError::Write {
            server: server.to_owned(),
            phase,
            source,
        })
}

async fn wait_for_response<T: DeserializeOwned>(
    lines: &mut Lines<BufReader<ChildStdout>>,
    server: &str,
    request_id: u64,
    phase: &'static str,
    timeout_secs: u64,
) -> Result<T, McpRuntimeError> {
    timeout(Duration::from_secs(timeout_secs), async {
        loop {
            let line = lines
                .next_line()
                .await
                .map_err(|source| McpRuntimeError::Read {
                    server: server.to_owned(),
                    phase,
                    source,
                })?;
            let Some(line) = line else {
                return Err(McpRuntimeError::Closed {
                    server: server.to_owned(),
                    phase,
                });
            };
            if line.trim().is_empty() {
                continue;
            }
            let envelope: JsonRpcEnvelope =
                serde_json::from_str(&line).map_err(|source| McpRuntimeError::Decode {
                    server: server.to_owned(),
                    phase,
                    source,
                })?;
            let Some(id) = envelope.id.as_ref() else {
                continue;
            };
            if !rpc_id_matches(id, request_id) {
                continue;
            }
            if let Some(error) = envelope.error {
                return Err(McpRuntimeError::Rpc {
                    server: server.to_owned(),
                    code: error.code,
                    message: error.message,
                });
            }
            let result = envelope.result.ok_or_else(|| McpRuntimeError::Protocol {
                server: server.to_owned(),
                phase,
                message: "response did not include a result payload".to_owned(),
            })?;
            return serde_json::from_value(result).map_err(|source| McpRuntimeError::Decode {
                server: server.to_owned(),
                phase,
                source,
            });
        }
    })
    .await
    .map_err(|_| McpRuntimeError::Timeout {
        server: server.to_owned(),
        phase,
        timeout_secs,
    })?
}

async fn shutdown_child(child: &mut Child) {
    kill_child_process_tree(child).await;
    let _ = timeout(Duration::from_secs(3), child.wait()).await;
}

async fn kill_child_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            return;
        }
    }

    let _ = child.start_kill();
}

// ---------------------------------------------------------------------------
// Persistent MCP client (session reuse)
// ---------------------------------------------------------------------------

/// Inner session state for [`McpClient`].
enum McpClientSession {
    /// An active stdio session (child process kept alive between calls).
    Stdio(Box<StdioMcpSession>),
    /// An active HTTP/SSE session (HTTP client reused between calls).
    Http(Box<RemoteMcpSession>),
    /// An active WebSocket session (persistent WebSocket connection).
    WebSocket(Box<WebSocketMcpSession>),
}

/// A persistent MCP client that reuses connections across multiple calls.
///
/// Unlike the top-level functions ([`call_tool`], [`inspect_server`], etc.)
/// which create a new process/connection for each invocation, `McpClient`
/// maintains a single active session and reuses it for all operations.
///
/// # Example
///
/// ```ignore
/// use claude_mcp::{McpClient, McpClientInfo, McpServerConfig};
///
/// let config: McpServerConfig = /* ... */;
/// let client_info = McpClientInfo::new("my-app", "1.0");
///
/// let mut client = McpClient::connect(&config, &client_info).await?;
/// // First call reuses the same connection
/// let result1 = client.call_tool("search", json!({"q": "rust"})).await?;
/// // Second call reuses the same connection — no new process spawned
/// let result2 = client.call_tool("search", json!({"q": "tokio"})).await?;
/// // Clean up when done
/// client.shutdown().await;
/// ```
pub struct McpClient {
    session: Option<McpClientSession>,
    config: McpServerConfig,
    /// Stored for potential reconnection if the session drops.
    #[allow(dead_code)]
    client_info: McpClientInfo,
}

impl McpClient {
    /// Connect to an MCP server and return a persistent client.
    ///
    /// For stdio transport, this spawns the child process and performs the
    /// initialization handshake. For HTTP transport, this creates an HTTP
    /// client and performs the handshake. The connection is kept alive for
    /// reuse across subsequent calls.
    pub async fn connect(
        config: &McpServerConfig,
        client_info: &McpClientInfo,
    ) -> Result<Self, McpRuntimeError> {
        let session = match &config.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                cwd,
                env,
            } => {
                let session = StdioMcpSession::connect(
                    config,
                    command,
                    args,
                    cwd.as_deref(),
                    env,
                    client_info,
                )
                .await?;
                McpClientSession::Stdio(Box::new(session))
            }
            McpTransportConfig::Sse { url, headers, .. } => {
                let session =
                    RemoteMcpSession::connect_sse(config, url, headers, client_info).await?;
                McpClientSession::Http(Box::new(session))
            }
            McpTransportConfig::Http { url, headers, .. } => {
                let session =
                    RemoteMcpSession::connect_http(config, url, headers, client_info).await?;
                McpClientSession::Http(Box::new(session))
            }
            McpTransportConfig::WebSocket { url, headers, .. } => {
                let session =
                    WebSocketMcpSession::connect(config, url, headers, client_info).await?;
                McpClientSession::WebSocket(Box::new(session))
            }
            McpTransportConfig::SseIde { url, .. } => {
                let session =
                    RemoteMcpSession::connect_sse(config, url, &BTreeMap::new(), client_info)
                        .await?;
                McpClientSession::Http(Box::new(session))
            }
            McpTransportConfig::WsIde { url, .. } => {
                let session =
                    WebSocketMcpSession::connect(config, url, &BTreeMap::new(), client_info)
                        .await?;
                McpClientSession::WebSocket(Box::new(session))
            }
            McpTransportConfig::Sdk { .. } | McpTransportConfig::ClaudeAiProxy { .. } => {
                return Err(McpRuntimeError::UnsupportedTransport {
                    server: config.name.clone(),
                    transport: config.transport.kind(),
                });
            }
        };

        Ok(Self {
            session: Some(session),
            config: config.clone(),
            client_info: client_info.clone(),
        })
    }

    /// Call a tool on the MCP server using the persistent connection.
    ///
    /// If the session has expired (HTTP 404 or JSON-RPC error code -32001),
    /// automatically reconnects and retries the call once.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResponse, McpRuntimeError> {
        let arguments_clone = arguments.clone();
        let result = match self.session.as_mut() {
            Some(McpClientSession::Stdio(session)) => session.call_tool(tool_name, arguments).await,
            Some(McpClientSession::Http(session)) => session.call_tool(tool_name, arguments).await,
            Some(McpClientSession::WebSocket(session)) => {
                session.call_tool(tool_name, arguments).await
            }
            None => Err(McpRuntimeError::Protocol {
                server: self.config.name.clone(),
                phase: "call_tool",
                message: "session is not connected".to_owned(),
            }),
        };

        // If the session has expired, reconnect and retry once.
        if let Err(ref error) = result
            && crate::error::is_session_expired_error(error)
        {
            tracing::info!(
                server = %self.config.name,
                "MCP session expired, reconnecting"
            );
            self.reconnect().await?;
            return match self.session.as_mut() {
                Some(McpClientSession::Stdio(session)) => {
                    session.call_tool(tool_name, arguments_clone).await
                }
                Some(McpClientSession::Http(session)) => {
                    session.call_tool(tool_name, arguments_clone).await
                }
                Some(McpClientSession::WebSocket(session)) => {
                    session.call_tool(tool_name, arguments_clone).await
                }
                None => Err(McpRuntimeError::Protocol {
                    server: self.config.name.clone(),
                    phase: "call_tool",
                    message: "session is not connected after reconnection".to_owned(),
                }),
            };
        }

        result
    }

    /// List tools available on the MCP server.
    pub async fn list_tools(
        &mut self,
    ) -> Result<Vec<crate::types::McpToolDescriptor>, McpRuntimeError> {
        match self.session.as_mut() {
            Some(McpClientSession::Stdio(session)) => {
                let request = JsonRpcRequest {
                    jsonrpc: "2.0",
                    id: 10,
                    method: "tools/list",
                    params: serde_json::json!({}),
                };
                write_message(
                    &mut session.stdin,
                    &session.server_name,
                    "tools/list request",
                    &request,
                )
                .await?;
                let result: McpToolsListResult = wait_for_response(
                    &mut session.lines,
                    &session.server_name,
                    10,
                    "tools/list response",
                    session.request_timeout_secs,
                )
                .await?;
                Ok(result.tools)
            }
            Some(McpClientSession::Http(_)) | Some(McpClientSession::WebSocket(_)) => {
                // For HTTP/WebSocket, we can do a fresh tools/list request
                let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
                let request = JsonRpcRequest {
                    jsonrpc: "2.0",
                    id: rpc_id,
                    method: "tools/list",
                    params: serde_json::json!({}),
                };
                let response = match self.session.as_mut() {
                    Some(McpClientSession::Http(session)) => {
                        send_http_request(
                            &session.http,
                            &session.url,
                            &session.headers,
                            &request,
                            session.request_timeout_secs,
                            &session.server_name,
                            "tools/list",
                        )
                        .await?
                    }
                    _ => unreachable!(),
                };
                let server_name = match self.session.as_ref() {
                    Some(McpClientSession::Http(session)) => session.server_name.clone(),
                    _ => unreachable!(),
                };
                let result: McpToolsListResult =
                    parse_jsonrpc_result(&response, &server_name, "tools/list")?;
                Ok(result.tools)
            }
            None => Err(McpRuntimeError::Protocol {
                server: self.config.name.clone(),
                phase: "list_tools",
                message: "session is not connected".to_owned(),
            }),
        }
    }

    /// List resources exposed by the MCP server.
    pub async fn list_resources(&mut self) -> Result<Vec<ServerResource>, McpRuntimeError> {
        match self.session.as_mut() {
            Some(McpClientSession::Stdio(session)) => session.list_resources().await,
            Some(McpClientSession::Http(session)) => session.list_resources().await,
            Some(McpClientSession::WebSocket(session)) => session.list_resources().await,
            None => Err(McpRuntimeError::Protocol {
                server: self.config.name.clone(),
                phase: "list_resources",
                message: "session is not connected".to_owned(),
            }),
        }
    }

    /// List prompts exposed by the MCP server.
    pub async fn list_prompts(&mut self) -> Result<Vec<McpPromptDescriptor>, McpRuntimeError> {
        match self.session.as_mut() {
            Some(McpClientSession::Stdio(session)) => session.list_prompts().await,
            Some(McpClientSession::Http(session)) => session.list_prompts().await,
            Some(McpClientSession::WebSocket(session)) => session.list_prompts().await,
            None => Err(McpRuntimeError::Protocol {
                server: self.config.name.clone(),
                phase: "list_prompts",
                message: "session is not connected".to_owned(),
            }),
        }
    }

    /// Inspect the MCP server (tools, resources, prompts).
    pub async fn inspect(&mut self) -> Result<McpServerInspection, McpRuntimeError> {
        match self.session.as_mut() {
            Some(McpClientSession::Stdio(session)) => session.inspect_server().await,
            Some(McpClientSession::Http(session)) => session.inspect_server().await,
            Some(McpClientSession::WebSocket(session)) => session.inspect_server().await,
            None => Err(McpRuntimeError::Protocol {
                server: self.config.name.clone(),
                phase: "inspect",
                message: "session is not connected".to_owned(),
            }),
        }
    }

    /// Reconnect to the MCP server by shutting down the current session
    /// and establishing a new one.
    pub async fn reconnect(&mut self) -> Result<(), McpRuntimeError> {
        self.shutdown().await;
        let mut new_client = McpClient::connect(&self.config, &self.client_info).await?;
        self.session = std::mem::take(&mut new_client.session);
        Ok(())
    }

    /// Check if the client has an active session.
    pub fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    /// Shut down the persistent connection.
    ///
    /// For stdio transport, this kills the child process. For HTTP transport,
    /// this is a no-op (the HTTP client is dropped when `McpClient` is dropped).
    pub async fn shutdown(&mut self) {
        if let Some(session) = self.session.take() {
            match session {
                McpClientSession::Stdio(mut s) => {
                    s.shutdown().await;
                }
                McpClientSession::Http(_) | McpClientSession::WebSocket(_) => {
                    // HTTP/WebSocket clients are dropped automatically
                }
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort cleanup: if the session hasn't been shut down, try to
        // kill the child process synchronously. For full async cleanup,
        // callers should call `shutdown()` before dropping.
        if let Some(McpClientSession::Stdio(mut session)) = self.session.take() {
            let _ = session.child.start_kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Remote MCP session (SSE / HTTP Streamable)
// ---------------------------------------------------------------------------

/// Global JSON-RPC request ID counter for remote sessions.
static REMOTE_RPC_ID: AtomicU64 = AtomicU64::new(1);

/// An MCP session over HTTP (Streamable) or SSE transport.
///
/// For HTTP Streamable: each request is a POST, and the response is returned
/// directly. For SSE: a persistent GET connection receives server-pushed
/// events, while POST is used for sending requests.
pub(crate) struct RemoteMcpSession {
    /// Server name (for error messages).
    server_name: String,
    /// Base URL for the MCP server endpoint.
    url: String,
    /// Additional HTTP headers.
    headers: BTreeMap<String, String>,
    /// HTTP client.
    http: reqwest::Client,
    /// Result of the initialization handshake.
    initialized: McpInitializeResult,
    /// Request timeout in seconds.
    request_timeout_secs: u64,
}

impl RemoteMcpSession {
    /// Connect to a remote MCP server via HTTP Streamable transport.
    ///
    /// Performs the initialization handshake and returns a ready-to-use
    /// session.
    pub async fn connect_http(
        server: &McpServerConfig,
        url: &str,
        headers: &BTreeMap<String, String>,
        client_info: &McpClientInfo,
    ) -> Result<Self, McpRuntimeError> {
        let http = reqwest::Client::new();
        let request_timeout_secs = server
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);

        // Resolve headers dynamically if a headers_helper is configured.
        let _resolved_headers = match &server.transport {
            McpTransportConfig::Http {
                headers_helper: Some(_helper),
                ..
            } => resolve_headers_with_helper(&server.name, None, headers).await,
            _ => headers.clone(),
        };

        // Build the initialization request using the generic JsonRpcRequest<T>.
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let init_params = InitializeParams {
            protocol_version: DEFAULT_MCP_PROTOCOL_VERSION,
            capabilities: serde_json::json!({}),
            client_info,
        };
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "initialize",
            params: init_params,
        };

        let response = send_http_request(
            &http,
            url,
            headers,
            &init_request,
            request_timeout_secs,
            &server.name,
            "initialize",
        )
        .await?;

        let init_result: McpInitializeResult =
            parse_jsonrpc_result(&response, &server.name, "initialize")?;

        // Send initialized notification (fire-and-forget).
        let initialized_notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: serde_json::json!({}),
        };
        let _ =
            send_http_notification(&http, url, headers, &initialized_notification, &server.name)
                .await;

        Ok(Self {
            server_name: server.name.clone(),
            url: url.to_owned(),
            headers: headers.clone(),
            http,
            initialized: init_result,
            request_timeout_secs,
        })
    }

    /// Connect to a remote MCP server via SSE (Server-Sent Events) transport.
    ///
    /// SSE transport connects to an HTTP endpoint, receives JSON-RPC responses
    /// as SSE `data:` events on a persistent GET connection, and sends
    /// JSON-RPC requests via HTTP POST to the same endpoint.
    pub async fn connect_sse(
        server: &McpServerConfig,
        url: &str,
        headers: &BTreeMap<String, String>,
        client_info: &McpClientInfo,
    ) -> Result<Self, McpRuntimeError> {
        let http = reqwest::Client::new();
        let request_timeout_secs = server
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);

        // Build the initialization request.
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let init_params = InitializeParams {
            protocol_version: DEFAULT_MCP_PROTOCOL_VERSION,
            capabilities: serde_json::json!({}),
            client_info,
        };
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "initialize",
            params: init_params,
        };

        // Send initialize via POST and parse the SSE response.
        let response = send_sse_request(
            &http,
            url,
            headers,
            &init_request,
            request_timeout_secs,
            &server.name,
            "initialize",
        )
        .await?;

        let init_result: McpInitializeResult =
            parse_jsonrpc_result(&response, &server.name, "initialize")?;

        // Send initialized notification (fire-and-forget).
        let initialized_notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: serde_json::json!({}),
        };
        let _ =
            send_http_notification(&http, url, headers, &initialized_notification, &server.name)
                .await;

        Ok(Self {
            server_name: server.name.clone(),
            url: url.to_owned(),
            headers: headers.clone(),
            http,
            initialized: init_result,
            request_timeout_secs,
        })
    }

    /// Inspect the server: return initialization result, tool list, and
    /// optionally prompts and resources.
    async fn inspect_server(&mut self) -> Result<McpServerInspection, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let tools_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "tools/list",
            params: serde_json::json!({}),
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &tools_request,
            self.request_timeout_secs,
            &self.server_name,
            "tools/list",
        )
        .await?;

        let tools_result: McpToolsListResult =
            parse_jsonrpc_result(&response, &self.server_name, "tools/list")?;

        // Fetch resources if the server declares the capability
        let resources = if self.supports_resources() {
            match self.list_resources().await {
                Ok(resources) => resources,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };

        // Fetch prompts if the server declares the capability
        let prompts = if self.supports_prompts() {
            match self.list_prompts().await {
                Ok(prompts) => prompts,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };

        Ok(McpServerInspection {
            server_name: self.server_name.clone(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            capabilities: self.initialized.capabilities.clone(),
            instructions: self.initialized.instructions.clone(),
            tools: tools_result.tools,
            prompts,
            resources,
        })
    }

    /// Call a tool on the remote MCP server.
    async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResponse, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "tools/call",
            params: ToolCallParams {
                name: tool_name,
                arguments,
            },
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &request,
            self.request_timeout_secs,
            &self.server_name,
            "tools/call",
        )
        .await?;

        let mut result: McpToolCallResult =
            parse_jsonrpc_result(&response, &self.server_name, "tools/call")?;

        // Truncate oversized tool results.
        crate::types::truncate_tool_call_result(&mut result);

        Ok(McpToolCallResponse {
            server_name: self.server_name.clone(),
            tool_name: tool_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// List resources from the remote MCP server.
    async fn list_resources(&mut self) -> Result<Vec<ServerResource>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "resources/list",
            params: serde_json::json!({}),
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &request,
            self.request_timeout_secs,
            &self.server_name,
            "resources/list",
        )
        .await?;

        let result: McpResourcesListResult =
            parse_jsonrpc_result(&response, &self.server_name, "resources/list")?;

        Ok(result
            .resources
            .into_iter()
            .map(|r| ServerResource {
                uri: r.uri,
                name: r.name,
                description: r.description,
                mime_type: r.mime_type,
                server: self.server_name.clone(),
            })
            .collect())
    }

    /// List prompts from the remote MCP server.
    async fn list_prompts(&mut self) -> Result<Vec<McpPromptDescriptor>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "prompts/list",
            params: serde_json::json!({}),
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &request,
            self.request_timeout_secs,
            &self.server_name,
            "prompts/list",
        )
        .await?;

        let result: McpPromptsListResult =
            parse_jsonrpc_result(&response, &self.server_name, "prompts/list")?;

        Ok(result
            .prompts
            .into_iter()
            .map(|p| McpPromptDescriptor {
                name: p.name,
                title: p.title,
                description: p.description,
                arguments: p.arguments,
            })
            .collect())
    }

    /// Get a prompt from the remote MCP server.
    async fn get_prompt(
        &mut self,
        prompt_name: &str,
        arguments: Value,
    ) -> Result<McpPromptGetResponse, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "prompts/get",
            params: PromptGetParams {
                name: prompt_name.to_owned(),
                arguments,
            },
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &request,
            self.request_timeout_secs,
            &self.server_name,
            "prompts/get",
        )
        .await?;

        let result: McpPromptGetRpcResult =
            parse_jsonrpc_result(&response, &self.server_name, "prompts/get")?;

        Ok(McpPromptGetResponse {
            server_name: self.server_name.clone(),
            prompt_name: prompt_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// Read a resource from the remote MCP server.
    async fn read_resource(
        &mut self,
        uri: &str,
    ) -> Result<Vec<McpResourceContent>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "resources/read",
            params: ResourceReadParams {
                uri: uri.to_owned(),
            },
        };

        let response = send_http_request(
            &self.http,
            &self.url,
            &self.headers,
            &request,
            self.request_timeout_secs,
            &self.server_name,
            "resources/read",
        )
        .await?;

        let result: McpResourceReadResult =
            parse_jsonrpc_result(&response, &self.server_name, "resources/read")?;

        Ok(result.contents)
    }

    fn supports_resources(&self) -> bool {
        self.initialized
            .capabilities
            .get("resources")
            .is_some_and(|resources| !resources.is_null() && resources != false)
    }

    fn supports_prompts(&self) -> bool {
        self.initialized
            .capabilities
            .get("prompts")
            .is_some_and(|prompts| !prompts.is_null() && prompts != false)
    }
}

/// Send a JSON-RPC request via HTTP POST and return the response body.
async fn send_http_request<T: Serialize>(
    http: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    request: &T,
    timeout_secs: u64,
    server_name: &str,
    phase: &'static str,
) -> Result<String, McpRuntimeError> {
    let body = serde_json::to_vec(request).map_err(|source| McpRuntimeError::Serialize {
        server: server_name.to_owned(),
        phase,
        source,
    })?;

    let mut builder = http
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(timeout_secs))
        .body(body);

    for (key, value) in headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    let response = timeout(Duration::from_secs(timeout_secs), builder.send())
        .await
        .map_err(|_| McpRuntimeError::Timeout {
            server: server_name.to_owned(),
            phase,
            timeout_secs,
        })?
        .map_err(|source| McpRuntimeError::Http {
            server: server_name.to_owned(),
            phase,
            source,
        })?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(McpRuntimeError::HttpError {
            server: server_name.to_owned(),
            phase,
            status: status.as_u16(),
            message: text,
        });
    }

    response
        .text()
        .await
        .map_err(|source| McpRuntimeError::Http {
            server: server_name.to_owned(),
            phase,
            source,
        })
}

/// Send a JSON-RPC notification via HTTP POST (fire-and-forget).
async fn send_http_notification<T: Serialize>(
    http: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    notification: &T,
    server_name: &str,
) -> Result<(), McpRuntimeError> {
    let body = serde_json::to_vec(notification).map_err(|source| McpRuntimeError::Serialize {
        server: server_name.to_owned(),
        phase: "notification",
        source,
    })?;

    let mut builder = http
        .post(url)
        .header("Content-Type", "application/json")
        .body(body);

    for (key, value) in headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    let _ = builder.send().await;
    Ok(())
}

/// Send a JSON-RPC request via SSE transport (HTTP POST) and parse the SSE
/// response, extracting the JSON-RPC payload from `data:` lines.
///
/// SSE transport sends requests via HTTP POST and receives responses as
/// Server-Sent Events. Each SSE event has a `data:` line containing the
/// JSON-RPC response. This function:
/// 1. POSTs the JSON-RPC request to the endpoint
/// 2. Reads the response body as an SSE stream
/// 3. Extracts the first `data:` line that parses as a JSON-RPC response
async fn send_sse_request<T: Serialize>(
    http: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    request: &T,
    timeout_secs: u64,
    server_name: &str,
    phase: &'static str,
) -> Result<String, McpRuntimeError> {
    // Send the JSON-RPC request via HTTP POST.
    let response = send_http_request(
        http,
        url,
        headers,
        request,
        timeout_secs,
        server_name,
        phase,
    )
    .await?;

    // Check if the response is SSE (text/event-stream) or plain JSON.
    // If the response contains SSE `data:` lines, extract the first one.
    // Otherwise treat it as a plain JSON-RPC response.
    let payload = if response.contains("data:") {
        // Parse SSE events — extract the first data line with valid JSON-RPC content.
        let mut found = None;
        for line in response.lines() {
            let line = line.trim();
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() && data.starts_with('{') {
                    found = Some(data.to_owned());
                    break;
                }
            }
        }
        match found {
            Some(data) => data,
            None => {
                return Err(McpRuntimeError::Protocol {
                    server: server_name.to_owned(),
                    phase,
                    message: "SSE response did not contain a valid data event".to_owned(),
                });
            }
        }
    } else {
        // Plain JSON response — return as-is.
        response
    };

    // Check for session-expiry-indicating error codes in the response.
    // -32000 covers ConnectionClosed and RequestTimeout in MCP SSE transport.
    check_sse_session_errors(&payload, server_name, phase)?;

    Ok(payload)
}

/// JSON-RPC error code used by MCP SSE transport for connection-closed and
/// request-timeout conditions. When the server returns this code the client
/// should treat the session as expired and reconnect.
const MCP_SSE_CONNECTION_ERROR_CODE: i64 = -32000;

/// Inspect a raw SSE JSON-RPC payload for error codes that indicate the
/// session should be expired. Returns `Ok(())` if no such error is found.
fn check_sse_session_errors(
    payload: &str,
    server_name: &str,
    phase: &'static str,
) -> Result<(), McpRuntimeError> {
    let envelope: JsonRpcEnvelope = match serde_json::from_str(payload) {
        Ok(e) => e,
        // Not valid JSON — nothing to check.
        Err(_) => return Ok(()),
    };
    if let Some(error) = &envelope.error
        && error.code == MCP_SSE_CONNECTION_ERROR_CODE
    {
        let msg = error.message.to_lowercase();
        // Distinguish between connection-closed and request-timeout.
        if msg.contains("connection closed") || msg.contains("connectionclosed") {
            tracing::warn!(
                server = %server_name,
                "MCP SSE connection closed by server (code {})", error.code
            );
            return Err(McpRuntimeError::JsonRpc {
                server: server_name.to_owned(),
                phase,
                code: error.code,
                message: error.message.clone(),
            });
        }
        if msg.contains("request timeout") || msg.contains("requesttimeout") {
            tracing::warn!(
                server = %server_name,
                "MCP SSE request timeout from server (code {})", error.code
            );
            return Err(McpRuntimeError::JsonRpc {
                server: server_name.to_owned(),
                phase,
                code: error.code,
                message: error.message.clone(),
            });
        }
    }
    Ok(())
}

/// Parse a JSON-RPC response body into the expected result type.
fn parse_jsonrpc_result<T: DeserializeOwned>(
    body: &str,
    server_name: &str,
    phase: &'static str,
) -> Result<T, McpRuntimeError> {
    let envelope: JsonRpcEnvelope =
        serde_json::from_str(body).map_err(|source| McpRuntimeError::Decode {
            server: server_name.to_owned(),
            phase,
            source,
        })?;

    if let Some(error) = &envelope.error {
        return Err(McpRuntimeError::JsonRpc {
            server: server_name.to_owned(),
            phase,
            code: error.code,
            message: error.message.clone(),
        });
    }

    serde_json::from_value(envelope.result.unwrap_or_default()).map_err(|source| {
        McpRuntimeError::Decode {
            server: server_name.to_owned(),
            phase,
            source,
        }
    })
}

// ---------------------------------------------------------------------------
// WebSocket MCP session
// ---------------------------------------------------------------------------

/// Type alias for the WebSocket stream used by MCP sessions.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// An MCP session over a persistent WebSocket connection.
///
/// Connects via WebSocket to the MCP server endpoint, sends the MCP initialize
/// message as JSON, reads the response, and then uses the same connection for
/// all subsequent JSON-RPC requests.
#[allow(dead_code)]
pub(crate) struct WebSocketMcpSession {
    /// Server name (for error messages).
    server_name: String,
    /// WebSocket URL.
    url: String,
    /// Additional HTTP headers.
    headers: BTreeMap<String, String>,
    /// The WebSocket stream (split sink + stream combined).
    ws: WsStream,
    /// Result of the initialization handshake.
    initialized: McpInitializeResult,
    /// Request timeout in seconds.
    request_timeout_secs: u64,
}

impl WebSocketMcpSession {
    /// Connect to a remote MCP server via WebSocket transport.
    ///
    /// Performs the initialization handshake and returns a ready-to-use
    /// session with the persistent WebSocket connection.
    pub async fn connect(
        server: &McpServerConfig,
        url: &str,
        headers: &BTreeMap<String, String>,
        client_info: &McpClientInfo,
    ) -> Result<Self, McpRuntimeError> {
        let request_timeout_secs = server
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);

        // Build the WebSocket request with optional extra headers.
        let mut request = tungstenite::http::Request::builder()
            .uri(url)
            .header("Sec-WebSocket-Protocol", "mcp");
        for (key, value) in headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let request = request
            .body(())
            .map_err(|source| McpRuntimeError::Protocol {
                server: server.name.clone(),
                phase: "ws-connect",
                message: format!("invalid WebSocket request: {source}"),
            })?;

        // Connect to the WebSocket server.
        let (mut ws, _response) = timeout(
            Duration::from_secs(request_timeout_secs),
            connect_async(request),
        )
        .await
        .map_err(|_| McpRuntimeError::Timeout {
            server: server.name.clone(),
            phase: "ws-connect",
            timeout_secs: request_timeout_secs,
        })?
        .map_err(|source| McpRuntimeError::Protocol {
            server: server.name.clone(),
            phase: "ws-connect",
            message: format!("WebSocket connection failed: {source}"),
        })?;

        // Build and send the initialize request.
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let init_params = InitializeParams {
            protocol_version: DEFAULT_MCP_PROTOCOL_VERSION,
            capabilities: serde_json::json!({}),
            client_info,
        };
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "initialize",
            params: init_params,
        };

        let init_result: McpInitializeResult = send_websocket_request_inner(
            &mut ws,
            &init_request,
            &server.name,
            "initialize",
            request_timeout_secs,
        )
        .await?;

        // Send the initialized notification (fire-and-forget).
        let initialized_notification = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: serde_json::json!({}),
        };
        let notification_str =
            serde_json::to_string(&initialized_notification).map_err(|source| {
                McpRuntimeError::Serialize {
                    server: server.name.clone(),
                    phase: "ws-initialized-notification",
                    source,
                }
            })?;
        let _ = ws
            .send(tungstenite::Message::Text(notification_str.into()))
            .await;

        Ok(Self {
            server_name: server.name.clone(),
            url: url.to_owned(),
            headers: headers.clone(),
            ws,
            initialized: init_result,
            request_timeout_secs,
        })
    }

    /// Inspect the server: return initialization result, tool list, and
    /// optionally prompts and resources.
    async fn inspect_server(&mut self) -> Result<McpServerInspection, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let tools_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "tools/list",
            params: serde_json::json!({}),
        };

        let tools_result: McpToolsListResult = send_websocket_request_inner(
            &mut self.ws,
            &tools_request,
            &self.server_name,
            "tools/list",
            self.request_timeout_secs,
        )
        .await?;

        let resources = if self.supports_resources() {
            match self.list_resources().await {
                Ok(resources) => resources,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };

        let prompts = if self.supports_prompts() {
            match self.list_prompts().await {
                Ok(prompts) => prompts,
                Err(error) if is_unsupported_method_error(&error) => Vec::new(),
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };

        Ok(McpServerInspection {
            server_name: self.server_name.clone(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            capabilities: self.initialized.capabilities.clone(),
            instructions: self.initialized.instructions.clone(),
            tools: tools_result.tools,
            prompts,
            resources,
        })
    }

    /// Call a tool on the remote MCP server via WebSocket.
    async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpToolCallResponse, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "tools/call",
            params: ToolCallParams {
                name: tool_name,
                arguments,
            },
        };

        let mut result: McpToolCallResult = send_websocket_request_inner(
            &mut self.ws,
            &request,
            &self.server_name,
            "tools/call",
            self.request_timeout_secs,
        )
        .await?;

        // Truncate oversized tool results.
        crate::types::truncate_tool_call_result(&mut result);

        Ok(McpToolCallResponse {
            server_name: self.server_name.clone(),
            tool_name: tool_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// List resources from the remote MCP server via WebSocket.
    async fn list_resources(&mut self) -> Result<Vec<ServerResource>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "resources/list",
            params: serde_json::json!({}),
        };

        let result: McpResourcesListResult = send_websocket_request_inner(
            &mut self.ws,
            &request,
            &self.server_name,
            "resources/list",
            self.request_timeout_secs,
        )
        .await?;

        Ok(result
            .resources
            .into_iter()
            .map(|r| ServerResource {
                uri: r.uri,
                name: r.name,
                description: r.description,
                mime_type: r.mime_type,
                server: self.server_name.clone(),
            })
            .collect())
    }

    /// List prompts from the remote MCP server via WebSocket.
    async fn list_prompts(&mut self) -> Result<Vec<McpPromptDescriptor>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "prompts/list",
            params: serde_json::json!({}),
        };

        let result: McpPromptsListResult = send_websocket_request_inner(
            &mut self.ws,
            &request,
            &self.server_name,
            "prompts/list",
            self.request_timeout_secs,
        )
        .await?;

        Ok(result
            .prompts
            .into_iter()
            .map(|p| McpPromptDescriptor {
                name: p.name,
                title: p.title,
                description: p.description,
                arguments: p.arguments,
            })
            .collect())
    }

    /// Get a prompt from the remote MCP server via WebSocket.
    async fn get_prompt(
        &mut self,
        prompt_name: &str,
        arguments: Value,
    ) -> Result<McpPromptGetResponse, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "prompts/get",
            params: PromptGetParams {
                name: prompt_name.to_owned(),
                arguments,
            },
        };

        let result: McpPromptGetRpcResult = send_websocket_request_inner(
            &mut self.ws,
            &request,
            &self.server_name,
            "prompts/get",
            self.request_timeout_secs,
        )
        .await?;

        Ok(McpPromptGetResponse {
            server_name: self.server_name.clone(),
            prompt_name: prompt_name.to_owned(),
            protocol_version: self.initialized.protocol_version.clone(),
            server_info: self.initialized.server_info.clone(),
            result,
        })
    }

    /// Read a resource from the remote MCP server via WebSocket.
    async fn read_resource(
        &mut self,
        uri: &str,
    ) -> Result<Vec<McpResourceContent>, McpRuntimeError> {
        let rpc_id = REMOTE_RPC_ID.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: rpc_id,
            method: "resources/read",
            params: ResourceReadParams {
                uri: uri.to_owned(),
            },
        };

        let result: McpResourceReadResult = send_websocket_request_inner(
            &mut self.ws,
            &request,
            &self.server_name,
            "resources/read",
            self.request_timeout_secs,
        )
        .await?;

        Ok(result.contents)
    }

    fn supports_resources(&self) -> bool {
        self.initialized
            .capabilities
            .get("resources")
            .is_some_and(|resources| !resources.is_null() && resources != false)
    }

    fn supports_prompts(&self) -> bool {
        self.initialized
            .capabilities
            .get("prompts")
            .is_some_and(|prompts| !prompts.is_null() && prompts != false)
    }
}

/// Send a JSON-RPC request over a WebSocket and read the response.
///
/// Serializes the request as JSON, sends it as a WebSocket text message,
/// then waits for a response message matching the request ID.
async fn send_websocket_request_inner<T: Serialize, R: DeserializeOwned>(
    ws: &mut WsStream,
    request: &T,
    server_name: &str,
    phase: &'static str,
    timeout_secs: u64,
) -> Result<R, McpRuntimeError> {
    // Serialize the request.
    let payload = serde_json::to_string(request).map_err(|source| McpRuntimeError::Serialize {
        server: server_name.to_owned(),
        phase,
        source,
    })?;

    // Extract the request ID for response matching.
    let request_value: Value =
        serde_json::from_str(&payload).map_err(|source| McpRuntimeError::Decode {
            server: server_name.to_owned(),
            phase,
            source,
        })?;
    let request_id = request_value.get("id").cloned();

    // Send the message.
    ws.send(tungstenite::Message::Text(payload.into()))
        .await
        .map_err(|source| McpRuntimeError::Write {
            server: server_name.to_owned(),
            phase,
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, source.to_string()),
        })?;

    // Read response, matching on the request ID.
    timeout(Duration::from_secs(timeout_secs), async {
        loop {
            let msg = ws
                .next()
                .await
                .ok_or_else(|| McpRuntimeError::Closed {
                    server: server_name.to_owned(),
                    phase,
                })?
                .map_err(|source| McpRuntimeError::Read {
                    server: server_name.to_owned(),
                    phase,
                    source: std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        source.to_string(),
                    ),
                })?;

            let text = match msg {
                tungstenite::Message::Text(text) => text,
                tungstenite::Message::Ping(data) => {
                    // Respond to pings with pong to keep the connection alive.
                    let _ = ws.send(tungstenite::Message::Pong(data)).await;
                    continue;
                }
                tungstenite::Message::Close(_) => {
                    return Err(McpRuntimeError::Closed {
                        server: server_name.to_owned(),
                        phase,
                    });
                }
                // Ignore binary, pong, and frame messages.
                _ => continue,
            };

            let text = text.trim();
            if text.is_empty() {
                continue;
            }

            let envelope: JsonRpcEnvelope =
                serde_json::from_str(text).map_err(|source| McpRuntimeError::Decode {
                    server: server_name.to_owned(),
                    phase,
                    source,
                })?;

            // Skip notifications (no id) and messages for other requests.
            if let Some(ref req_id) = request_id {
                if let Some(ref resp_id) = envelope.id {
                    if resp_id != req_id {
                        continue;
                    }
                } else {
                    continue;
                }
            } else if envelope.id.is_some() {
                continue;
            }

            if let Some(error) = envelope.error {
                return Err(McpRuntimeError::Rpc {
                    server: server_name.to_owned(),
                    code: error.code,
                    message: error.message,
                });
            }

            let result = envelope.result.ok_or_else(|| McpRuntimeError::Protocol {
                server: server_name.to_owned(),
                phase,
                message: "response did not include a result payload".to_owned(),
            })?;

            return serde_json::from_value(result).map_err(|source| McpRuntimeError::Decode {
                server: server_name.to_owned(),
                phase,
                source,
            });
        }
    })
    .await
    .map_err(|_| McpRuntimeError::Timeout {
        server: server_name.to_owned(),
        phase,
        timeout_secs,
    })?
}

// ---------------------------------------------------------------------------
// Headers helper resolution
// ---------------------------------------------------------------------------

/// Resolve headers dynamically using a `headers_helper` callback from
/// [`TransportConfig`], falling back to the static headers from
/// [`McpTransportConfig`].
///
/// This is used by `connect_http`, `connect_sse`, and the WebSocket session
/// to inject fresh headers (e.g. short-lived auth tokens) before each request.
async fn resolve_headers_with_helper(
    server_name: &str,
    transport_config: Option<&crate::transport::TransportConfig>,
    static_headers: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut resolved = static_headers.clone();

    if let Some(config) = transport_config {
        // Try to get a headers_helper from the extended TransportConfig.
        let helper_result =
            crate::headers::McpHeadersResolver::resolve_headers(server_name, config, |key| {
                std::env::var(key).ok()
            })
            .await;

        match helper_result {
            Ok(dynamic_headers) => {
                // Merge: static headers win on conflict.
                for (k, v) in dynamic_headers {
                    resolved.entry(k).or_insert(v);
                }
            }
            Err(err) => {
                tracing::warn!(
                    server = %server_name,
                    "Failed to resolve dynamic headers: {err}"
                );
            }
        }
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::resolve_stdio_command;

    #[test]
    fn resolve_stdio_command_preserves_explicit_extension() {
        assert_eq!(resolve_stdio_command("python.exe"), "python.exe");
    }

    #[test]
    fn resolve_stdio_command_preserves_relative_paths() {
        assert_eq!(
            resolve_stdio_command(".\\scripts\\server.cmd"),
            ".\\scripts\\server.cmd"
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_stdio_command_prefers_windows_wrappers_when_available() {
        let resolved = resolve_stdio_command("npx");
        assert!(
            resolved.eq_ignore_ascii_case("npx")
                || resolved.to_ascii_lowercase().ends_with("npx.cmd")
                || resolved.to_ascii_lowercase().ends_with("npx.exe"),
            "unexpected resolved command: {resolved}"
        );
    }

    #[test]
    fn check_sse_session_errors_no_error() {
        let payload = r#"{"id":1,"result":{"protocolVersion":"2025-03-26"}}"#;
        assert!(super::check_sse_session_errors(payload, "test", "init").is_ok());
    }

    #[test]
    fn check_sse_session_errors_connection_closed() {
        let payload = r#"{"id":1,"error":{"code":-32000,"message":"Connection closed"}}"#;
        let result = super::check_sse_session_errors(payload, "test", "init");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            super::McpRuntimeError::JsonRpc { code, .. } => assert_eq!(code, -32000),
            other => panic!("expected JsonRpc error, got: {other}"),
        }
    }

    #[test]
    fn check_sse_session_errors_request_timeout() {
        let payload = r#"{"id":1,"error":{"code":-32000,"message":"Request timeout"}}"#;
        let result = super::check_sse_session_errors(payload, "test", "init");
        assert!(result.is_err());
    }

    #[test]
    fn check_sse_session_errors_other_code_ignored() {
        // -32000 but with a different message — should not trigger.
        let payload = r#"{"id":1,"error":{"code":-32000,"message":"Some other error"}}"#;
        assert!(super::check_sse_session_errors(payload, "test", "init").is_ok());
    }

    #[test]
    fn check_sse_session_errors_invalid_json_ignored() {
        let payload = "not json";
        assert!(super::check_sse_session_errors(payload, "test", "init").is_ok());
    }
}
