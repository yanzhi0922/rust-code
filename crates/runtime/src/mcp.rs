use anyhow::Context;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub connected: bool,
    pub tool_count: usize,
    pub resource_count: usize,
}

#[async_trait]
pub trait McpTransport: Send {
    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value>;
    async fn close(&mut self);
}

pub struct StdioTransport {
    child: Option<Child>,
    next_id: u64,
}

impl StdioTransport {
    pub fn new() -> Self {
        Self {
            child: None,
            next_id: 1,
        }
    }

    pub async fn spawn(
        &mut self,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (key, value) in env {
            cmd.env(key, value);
        }

        let child = cmd.spawn()?;
        self.child = Some(child);
        Ok(())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let child = self.child.as_mut().context("MCP server not connected")?;

        let id = self.next_id;
        self.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let stdin = child.stdin.as_mut().context("stdin not available")?;
        let msg = format!("{}\n", serde_json::to_string(&request)?);
        stdin.write_all(msg.as_bytes()).await?;
        stdin.flush().await?;

        let stdout = child.stdout.as_mut().context("stdout not available")?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let response: serde_json::Value = serde_json::from_str(&line)?;
        if let Some(error) = response.get("error") {
            anyhow::bail!("MCP error: {}", error);
        }

        Ok(response)
    }

    async fn close(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill().await;
        }
        self.child = None;
    }
}

pub struct SseTransport {
    url: String,
    message_endpoint: Option<String>,
    http_client: reqwest::Client,
    response_tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    response_rx: Option<tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>>,
    sse_task: Option<tokio::task::JoinHandle<()>>,
    next_id: u64,
}

fn resolve_endpoint(base_url: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    let Ok(base) = url::Url::parse(base_url) else {
        return format!("{}/{}", base_url.trim_end_matches('/'), endpoint.trim_start_matches('/'));
    };
    let scheme = base.scheme();
    let host = base.host_str().unwrap_or("localhost");
    let port_str = base.port().map_or(String::new(), |p| format!(":{}", p));
    let origin = format!("{}://{}{}", scheme, host, port_str);
    if endpoint.starts_with('/') {
        format!("{}{}", origin, endpoint)
    } else {
        format!("{}/{}", origin, endpoint)
    }
}

impl SseTransport {
    pub fn new(url: String) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            url,
            message_endpoint: None,
            http_client: reqwest::Client::new(),
            response_tx: tx,
            response_rx: Some(rx),
            sse_task: None,
            next_id: 1,
        }
    }

    pub async fn connect(&mut self) -> anyhow::Result<()> {
        let response = self
            .http_client
            .get(&self.url)
            .header("Accept", "text/event-stream")
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "SSE connection failed with status: {}",
                response.status()
            );
        }

        let (endpoint_tx, endpoint_rx) = tokio::sync::oneshot::channel::<String>();
        let response_tx = self.response_tx.clone();
        let mut endpoint_tx = Some(endpoint_tx);

        let task = tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut current_event_type = String::new();

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos]
                        .trim_end_matches('\r')
                        .to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        current_event_type.clear();
                        continue;
                    }

                    if line.starts_with(':') {
                        continue;
                    }

                    if let Some(evt) = line.strip_prefix("event: ") {
                        current_event_type = evt.trim().to_string();
                    } else if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if current_event_type == "endpoint" {
                            if let Some(tx) = endpoint_tx.take() {
                                let _ = tx.send(data.to_string());
                            }
                        } else if !data.is_empty() {
                            if let Ok(json) =
                                serde_json::from_str::<serde_json::Value>(data)
                            {
                                let _ = response_tx.send(json);
                            }
                        }
                    }
                }
            }
        });

        let endpoint_path = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            endpoint_rx,
        )
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for endpoint from SSE stream"))?
        .map_err(|_| anyhow::anyhow!("Failed to receive endpoint from SSE stream"))?;

        let endpoint = resolve_endpoint(&self.url, &endpoint_path);
        self.message_endpoint = Some(endpoint);
        self.sse_task = Some(task);

        Ok(())
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let endpoint = self
            .message_endpoint
            .as_ref()
            .context("SSE transport not connected - no message endpoint")?;

        let post_response = self.http_client.post(endpoint).json(&request).send().await?;

        if !post_response.status().is_success() {
            anyhow::bail!(
                "HTTP POST failed with status: {}",
                post_response.status()
            );
        }

        let rx = self
            .response_rx
            .as_mut()
            .context("SSE response channel not available")?;

        loop {
            let response = rx
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("SSE stream closed"))?;

            if response.get("id").and_then(|i| i.as_u64()) == Some(id) {
                if let Some(error) = response.get("error") {
                    anyhow::bail!("MCP error: {}", error);
                }
                return Ok(response);
            }
        }
    }

    async fn close(&mut self) {
        if let Some(task) = self.sse_task.take() {
            task.abort();
        }
        self.response_rx.take();
        self.message_endpoint.take();
    }
}

pub struct McpClient {
    config: McpServerConfig,
    transport: Option<Box<dyn McpTransport>>,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            transport: None,
            tools: Vec::new(),
            resources: Vec::new(),
        }
    }

    pub async fn connect(&mut self) -> anyhow::Result<()> {
        if let Some(ref command) = self.config.command {
            let mut transport = StdioTransport::new();
            transport
                .spawn(command, &self.config.args, &self.config.env)
                .await?;
            self.transport = Some(Box::new(transport));
        } else if let Some(ref url) = self.config.url {
            let mut transport = SseTransport::new(url.clone());
            transport.connect().await?;
            self.transport = Some(Box::new(transport));
        } else {
            anyhow::bail!("No command or URL specified for MCP server");
        }

        self.send_initialize().await?;
        self.discover_tools().await?;
        self.discover_resources().await?;

        Ok(())
    }

    async fn send_initialize(&mut self) -> anyhow::Result<()> {
        let transport = self
            .transport
            .as_mut()
            .context("MCP transport not connected")?;

        transport
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "claude-code-rs",
                        "version": "0.1.0"
                    }
                }),
            )
            .await?;

        Ok(())
    }

    async fn discover_tools(&mut self) -> anyhow::Result<()> {
        let response = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;
        if let Some(tools) = response.get("result").and_then(|r| r.get("tools")) {
            if let Some(tools_array) = tools.as_array() {
                self.tools = tools_array
                    .iter()
                    .filter_map(|t| {
                        Some(McpTool {
                            name: t.get("name")?.as_str()?.to_string(),
                            description: t
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input_schema: t.get("inputSchema")?.clone(),
                        })
                    })
                    .collect();
            }
        }
        Ok(())
    }

    async fn discover_resources(&mut self) -> anyhow::Result<()> {
        let response = self
            .send_request("resources/list", serde_json::json!({}))
            .await?;
        if let Some(resources) = response
            .get("result")
            .and_then(|r| r.get("resources"))
        {
            if let Some(resources_array) = resources.as_array() {
                self.resources = resources_array
                    .iter()
                    .filter_map(|r| {
                        Some(McpResource {
                            uri: r.get("uri")?.as_str()?.to_string(),
                            name: r.get("name")?.as_str()?.to_string(),
                            description: r
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(String::from),
                            mime_type: r
                                .get("mimeType")
                                .and_then(|m| m.as_str())
                                .map(String::from),
                        })
                    })
                    .collect();
            }
        }
        Ok(())
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let transport = self
            .transport
            .as_mut()
            .context("MCP transport not connected")?;
        transport.send_request(method, params).await
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let response = self
            .send_request(
                "tools/call",
                serde_json::json!({ "name": name, "arguments": args }),
            )
            .await?;

        let content = response
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.get("text")
                            .and_then(|t| t.as_str())
                            .map(String::from)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        Ok(content)
    }

    pub async fn read_resource(&mut self, uri: &str) -> anyhow::Result<String> {
        let response = self
            .send_request("resources/read", serde_json::json!({ "uri": uri }))
            .await?;

        let content = response
            .get("result")
            .and_then(|r| r.get("contents"))
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.get("text")
                            .and_then(|t| t.as_str())
                            .map(String::from)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        Ok(content)
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    pub async fn disconnect(&mut self) -> anyhow::Result<()> {
        if let Some(mut transport) = self.transport.take() {
            transport.close().await;
        }
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_some()
    }
}

pub struct McpManager {
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    pub async fn add_server(&mut self, config: McpServerConfig) -> anyhow::Result<()> {
        let name = config.name.clone();
        let mut client = McpClient::new(config);
        if client.config.enabled {
            client.connect().await?;
        }
        self.clients.insert(name, client);
        Ok(())
    }

    pub fn get_client(&self, name: &str) -> Option<&McpClient> {
        self.clients.get(name)
    }

    pub fn get_client_mut(&mut self, name: &str) -> Option<&mut McpClient> {
        self.clients.get_mut(name)
    }

    pub fn all_tools(&self) -> Vec<(&str, &McpTool)> {
        self.clients
            .iter()
            .flat_map(|(name, client): (&String, &McpClient)| {
                client.tools().iter().map(move |t| (name.as_str(), t))
            })
            .collect()
    }

    pub async fn disconnect_all(&mut self) -> anyhow::Result<()> {
        for (_name, client) in self.clients.iter_mut() {
            client.disconnect().await?;
        }
        Ok(())
    }

    pub async fn remove_server(&mut self, name: &str) -> anyhow::Result<()> {
        if let Some(mut client) = self.clients.remove(name) {
            client.disconnect().await?;
        }
        Ok(())
    }

    pub fn list_servers(&self) -> Vec<&str> {
        self.clients.keys().map(|s| s.as_str()).collect()
    }

    pub async fn call_tool_any(
        &mut self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        for client in self.clients.values_mut() {
            if client.tools().iter().any(|t| t.name == tool_name) {
                return client.call_tool(tool_name, args).await;
            }
        }
        anyhow::bail!("No server has tool: {}", tool_name)
    }

    pub async fn read_resource_any(&mut self, uri: &str) -> anyhow::Result<String> {
        for client in self.clients.values_mut() {
            if client.resources().iter().any(|r| r.uri == uri) {
                return client.read_resource(uri).await;
            }
        }
        anyhow::bail!("No server has resource: {}", uri)
    }

    pub fn get_server_status(&self, name: &str) -> Option<McpServerStatus> {
        self.clients.get(name).map(|client| McpServerStatus {
            name: name.to_string(),
            connected: client.is_connected(),
            tool_count: client.tools().len(),
            resource_count: client.resources().len(),
        })
    }

    pub async fn reconnect(&mut self, name: &str) -> anyhow::Result<()> {
        let config = self
            .clients
            .get(name)
            .map(|c| c.config.clone())
            .ok_or_else(|| anyhow::anyhow!("Server not found: {}", name))?;

        if let Some(mut client) = self.clients.remove(name) {
            let _ = client.disconnect().await;
        }

        let mut new_client = McpClient::new(config);
        new_client.connect().await?;
        self.clients.insert(name.to_string(), new_client);

        Ok(())
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disabled_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            command: None,
            args: vec![],
            env: HashMap::new(),
            url: None,
            enabled: false,
        }
    }

    #[test]
    fn test_config_deserialize_command() {
        let json = r#"{"name":"test","command":"node","args":["server.js"],"enabled":true}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "test");
        assert_eq!(config.command, Some("node".to_string()));
        assert_eq!(config.args, vec!["server.js"]);
        assert!(config.enabled);
        assert!(config.url.is_none());
    }

    #[test]
    fn test_config_deserialize_url() {
        let json = r#"{"name":"test","url":"http://localhost:3000/sse","enabled":true}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.url,
            Some("http://localhost:3000/sse".to_string())
        );
        assert!(config.command.is_none());
    }

    #[test]
    fn test_config_defaults() {
        let json = r#"{"name":"test"}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert!(config.command.is_none());
        assert!(config.url.is_none());
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
        assert!(!config.enabled);
    }

    #[tokio::test]
    async fn test_manager_list_servers() {
        let mut manager = McpManager::new();
        assert!(manager.list_servers().is_empty());

        manager
            .add_server(disabled_config("server1"))
            .await
            .unwrap();
        manager
            .add_server(disabled_config("server2"))
            .await
            .unwrap();

        let mut names = manager.list_servers();
        names.sort();
        assert_eq!(names, vec!["server1", "server2"]);
    }

    #[tokio::test]
    async fn test_manager_remove_server() {
        let mut manager = McpManager::new();
        manager
            .add_server(disabled_config("server1"))
            .await
            .unwrap();
        manager
            .add_server(disabled_config("server2"))
            .await
            .unwrap();

        manager.remove_server("server1").await.unwrap();
        assert_eq!(manager.list_servers(), vec!["server2"]);

        manager.remove_server("nonexistent").await.unwrap();
        assert_eq!(manager.list_servers(), vec!["server2"]);
    }

    #[tokio::test]
    async fn test_server_status_disconnected() {
        let mut manager = McpManager::new();
        manager
            .add_server(disabled_config("test"))
            .await
            .unwrap();

        let status = manager.get_server_status("test").unwrap();
        assert_eq!(status.name, "test");
        assert!(!status.connected);
        assert_eq!(status.tool_count, 0);
        assert_eq!(status.resource_count, 0);
    }

    #[test]
    fn test_server_status_struct() {
        let status = McpServerStatus {
            name: "myserver".to_string(),
            connected: true,
            tool_count: 5,
            resource_count: 3,
        };
        assert_eq!(status.name, "myserver");
        assert!(status.connected);
        assert_eq!(status.tool_count, 5);
        assert_eq!(status.resource_count, 3);
    }

    #[test]
    fn test_resolve_endpoint_relative() {
        assert_eq!(
            resolve_endpoint("http://localhost:3000/sse", "/message?sid=123"),
            "http://localhost:3000/message?sid=123"
        );
    }

    #[test]
    fn test_resolve_endpoint_absolute() {
        assert_eq!(
            resolve_endpoint(
                "http://localhost:3000/sse",
                "http://other:4000/message"
            ),
            "http://other:4000/message"
        );
    }

    #[test]
    fn test_resolve_endpoint_with_port() {
        assert_eq!(
            resolve_endpoint("http://localhost:8080/sse", "/msg"),
            "http://localhost:8080/msg"
        );
    }

    #[test]
    fn test_resolve_endpoint_https() {
        assert_eq!(
            resolve_endpoint("https://example.com/sse", "/message"),
            "https://example.com/message"
        );
    }

    #[tokio::test]
    async fn test_call_tool_any_not_found() {
        let mut manager = McpManager::new();
        manager
            .add_server(disabled_config("test"))
            .await
            .unwrap();

        let result = manager
            .call_tool_any("nonexistent", serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_resource_any_not_found() {
        let mut manager = McpManager::new();
        manager
            .add_server(disabled_config("test"))
            .await
            .unwrap();

        let result = manager.read_resource_any("file:///x").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reconnect_nonexistent() {
        let mut manager = McpManager::new();
        let result = manager.reconnect("nope").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_transport_detection() {
        let mut config = disabled_config("test");
        config.command = Some("node".into());
        assert!(config.command.is_some());

        let mut config = disabled_config("test");
        config.url = Some("http://localhost/sse".into());
        assert!(config.url.is_some());
    }
}
