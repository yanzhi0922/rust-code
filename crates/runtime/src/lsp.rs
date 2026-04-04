use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: u32,
    pub range: LspRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<LspSymbol>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspMarkedString {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHover {
    pub contents: Vec<LspMarkedString>,
}

pub struct LspClient {
    process: Option<tokio::process::Child>,
    stdin_writer: Option<tokio::process::ChildStdin>,
    stdout_reader: Option<BufReader<tokio::process::ChildStdout>>,
    request_id: u32,
}

impl LspClient {
    pub fn new(command: &str, args: &[String]) -> Self {
        let child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match child {
            Ok(mut proc) => {
                let stdin_writer = proc.stdin.take();
                let stdout_reader = proc.stdout.take().map(BufReader::new);
                Self {
                    process: Some(proc),
                    stdin_writer,
                    stdout_reader,
                    request_id: 0,
                }
            }
            Err(_) => Self {
                process: None,
                stdin_writer: None,
                stdout_reader: None,
                request_id: 0,
            },
        }
    }

    pub fn is_connected(&self) -> bool {
        self.process.is_some()
            && self.stdin_writer.is_some()
            && self.stdout_reader.is_some()
    }

    pub async fn initialize(&mut self, root_uri: &str) -> Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "rootPath": root_uri,
            "capabilities": {
                "textDocument": {
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
                    "documentSymbol": { "dynamicRegistration": false, "hierarchicalDocumentSymbolSupport": true },
                    "implementation": { "dynamicRegistration": false }
                },
                "workspace": {
                    "symbol": { "dynamicRegistration": false }
                }
            }
        });

        let response = self.send_request("initialize", params).await?;
        let _result = response.get("result").cloned().unwrap_or(Value::Null);

        self.send_notification(
            "initialized",
            json!({ "capabilities": {} }),
        )
        .await?;

        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self
            .send_request("shutdown", Value::Null)
            .await
            .map_err(|e| anyhow::anyhow!("LSP shutdown request failed: {}", e));
        let _ = self
            .send_notification("exit", Value::Null)
            .await
            .map_err(|e| anyhow::anyhow!("LSP exit notification failed: {}", e));

        if let Some(ref mut proc) = self.process {
            let _ = proc.kill().await;
        }
        self.process = None;
        self.stdin_writer = None;
        self.stdout_reader = None;

        Ok(())
    }

    pub async fn goto_definition(
        &mut self,
        file_uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        let params = json!({
            "textDocument": { "uri": file_uri },
            "position": position
        });

        let response = self.send_request("textDocument/definition", params).await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        match result {
            Value::Null | Value::Bool(false) => Ok(vec![]),
            Value::Array(arr) => {
                let locations: Vec<LspLocation> = arr
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                Ok(locations)
            }
            single => {
                let loc = serde_json::from_value(single).ok();
                Ok(loc.into_iter().collect())
            }
        }
    }

    pub async fn find_references(
        &mut self,
        file_uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        let params = json!({
            "textDocument": { "uri": file_uri },
            "position": position,
            "context": { "includeDeclaration": true }
        });

        let response = self
            .send_request("textDocument/references", params)
            .await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        match result {
            Value::Null => Ok(vec![]),
            Value::Array(arr) => {
                let locations: Vec<LspLocation> = arr
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                Ok(locations)
            }
            _ => Ok(vec![]),
        }
    }

    pub async fn hover(
        &mut self,
        file_uri: &str,
        position: LspPosition,
    ) -> Result<Option<LspHover>> {
        let params = json!({
            "textDocument": { "uri": file_uri },
            "position": position
        });

        let response = self.send_request("textDocument/hover", params).await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        if result.is_null() {
            return Ok(None);
        }

        let contents = result
            .get("contents")
            .and_then(|c| {
                if c.is_string() {
                    Some(vec![LspMarkedString {
                        language: None,
                        value: c.as_str().unwrap_or("").to_string(),
                    }])
                } else if let Some(kind) = c.get("kind") {
                    let lang = if kind.as_str() == Some("markdown") {
                        Some("markdown".to_string())
                    } else {
                        None
                    };
                    let value = c
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(vec![LspMarkedString {
                        language: lang,
                        value,
                    }])
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Ok(Some(LspHover { contents }))
    }

    pub async fn document_symbols(&mut self, file_uri: &str) -> Result<Vec<LspSymbol>> {
        let params = json!({
            "textDocument": { "uri": file_uri }
        });

        let response = self
            .send_request("textDocument/documentSymbol", params)
            .await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        match result {
            Value::Null => Ok(vec![]),
            Value::Array(arr) => {
                let symbols: Vec<LspSymbol> = arr
                    .iter()
                    .filter_map(|v| Self::parse_symbol(v))
                    .collect();
                Ok(symbols)
            }
            _ => Ok(vec![]),
        }
    }

    pub async fn workspace_symbol(&mut self, query: &str) -> Result<Vec<LspSymbol>> {
        let params = json!({ "query": query });

        let response = self
            .send_request("workspace/symbol", params)
            .await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        match result {
            Value::Null => Ok(vec![]),
            Value::Array(arr) => {
                let symbols: Vec<LspSymbol> = arr
                    .iter()
                    .filter_map(|v| Self::parse_symbol_info(v))
                    .collect();
                Ok(symbols)
            }
            _ => Ok(vec![]),
        }
    }

    pub async fn goto_implementation(
        &mut self,
        file_uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        let params = json!({
            "textDocument": { "uri": file_uri },
            "position": position
        });

        let response = self
            .send_request("textDocument/implementation", params)
            .await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);

        match result {
            Value::Null | Value::Bool(false) => Ok(vec![]),
            Value::Array(arr) => {
                let locations: Vec<LspLocation> = arr
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                Ok(locations)
            }
            single => {
                let loc = serde_json::from_value(single).ok();
                Ok(loc.into_iter().collect())
            }
        }
    }

    pub async fn did_open(&mut self, file_uri: &str, language_id: &str, text: &str) -> Result<()> {
        let params = json!({
            "textDocument": {
                "uri": file_uri,
                "languageId": language_id,
                "version": 1,
                "text": text
            }
        });
        self.send_notification("textDocument/didOpen", params).await
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_id += 1;
        let id = self.request_id;

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.send_message(&message).await?;
        let response = self.read_response().await?;

        Ok(response)
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        self.send_message(&message).await
    }

    async fn send_message(&mut self, message: &Value) -> Result<()> {
        let writer = self
            .stdin_writer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("LSP stdin not available"))?;

        let content = serde_json::to_string(message)?;
        let header = format!("Content-Length: {}\r\n\r\n", content.len());

        writer.write_all(header.as_bytes()).await?;
        writer.write_all(content.as_bytes()).await?;
        writer.flush().await?;

        Ok(())
    }

    async fn read_response(&mut self) -> Result<Value> {
        let reader = self
            .stdout_reader
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("LSP stdout not available"))?;

        let mut header_line = String::new();
        let mut content_length: usize = 0;

        loop {
            header_line.clear();
            let bytes_read = reader.read_line(&mut header_line).await?;
            if bytes_read == 0 {
                anyhow::bail!("LSP connection closed");
            }

            let line = header_line.trim();
            if line.is_empty() {
                break;
            }

            if let Some(len_str) = line.strip_prefix("Content-Length:") {
                content_length = len_str.trim().parse::<usize>()?;
            }
        }

        if content_length == 0 {
            anyhow::bail!("No Content-Length header received");
        }

        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer).await?;
        let content_str = String::from_utf8(buffer)?;
        let response: Value = serde_json::from_str(&content_str)?;

        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("LSP error (code {}): {}", code, message);
        }

        Ok(response)
    }

    fn parse_symbol(value: &Value) -> Option<LspSymbol> {
        let name = value.get("name")?.as_str()?.to_string();
        let kind = value.get("kind")?.as_u64()? as u32;

        let range = if let Some(range_val) = value.get("range") {
            serde_json::from_value(range_val.clone()).ok()?
        } else {
            return None;
        };

        let children = value.get("children").and_then(|c| {
            let child_symbols: Vec<LspSymbol> =
                c.as_array()?.iter().filter_map(Self::parse_symbol).collect();
            if child_symbols.is_empty() {
                None
            } else {
                Some(child_symbols)
            }
        });

        Some(LspSymbol {
            name,
            kind,
            range,
            children,
        })
    }

    fn parse_symbol_info(value: &Value) -> Option<LspSymbol> {
        let name = value.get("name")?.as_str()?.to_string();
        let kind = value.get("kind")?.as_u64()? as u32;

        let range = if let Some(loc) = value.get("location") {
            serde_json::from_value(loc.get("range")?.clone()).ok()?
        } else {
            return None;
        };

        Some(LspSymbol {
            name,
            kind,
            range,
            children: None,
        })
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Some(ref mut proc) = self.process {
            let _ = proc.start_kill();
        }
    }
}

pub struct LspManager {
    clients: HashMap<String, LspClient>,
    file_extensions: HashMap<String, String>,
}

impl LspManager {
    pub fn new() -> Self {
        let mut file_extensions = HashMap::new();
        file_extensions.insert(".rs".to_string(), "rust-analyzer".to_string());
        file_extensions.insert(".ts".to_string(), "typescript-language-server".to_string());
        file_extensions.insert(".tsx".to_string(), "typescript-language-server".to_string());
        file_extensions.insert(".js".to_string(), "typescript-language-server".to_string());
        file_extensions.insert(".jsx".to_string(), "typescript-language-server".to_string());
        file_extensions.insert(".py".to_string(), "pylsp".to_string());
        file_extensions.insert(".go".to_string(), "gopls".to_string());
        file_extensions.insert(".c".to_string(), "clangd".to_string());
        file_extensions.insert(".cpp".to_string(), "clangd".to_string());
        file_extensions.insert(".h".to_string(), "clangd".to_string());
        file_extensions.insert(".java".to_string(), "jdtls".to_string());
        file_extensions.insert(".rb".to_string(), "solargraph".to_string());
        file_extensions.insert(".swift".to_string(), "sourcekit-lsp".to_string());
        file_extensions.insert(".kt".to_string(), "kotlin-language-server".to_string());
        file_extensions.insert(".cs".to_string(), "omnisharp".to_string());

        Self {
            clients: HashMap::new(),
            file_extensions,
        }
    }

    pub async fn register_server(
        &mut self,
        name: &str,
        command: &str,
        args: &[String],
        root_uri: &str,
    ) -> Result<()> {
        let mut client = LspClient::new(command, args);

        if !client.is_connected() {
            anyhow::bail!(
                "Failed to start LSP server '{}' with command: {}",
                name,
                command
            );
        }

        client.initialize(root_uri).await?;
        self.clients.insert(name.to_string(), client);

        Ok(())
    }

    pub fn get_client_for_file(&mut self, file_path: &str) -> Option<&mut LspClient> {
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))?;

        let server_name = self.file_extensions.get(&ext)?;
        self.clients.get_mut(server_name)
    }

    pub async fn shutdown_all(&mut self) -> Result<()> {
        let names: Vec<String> = self.clients.keys().cloned().collect();
        for name in names {
            if let Some(mut client) = self.clients.remove(&name) {
                let _ = client.shutdown().await;
            }
        }
        Ok(())
    }

    pub fn is_server_registered(&self, name: &str) -> bool {
        self.clients.contains_key(name)
    }

    pub fn registered_servers(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    pub fn file_extensions(&self) -> &HashMap<String, String> {
        &self.file_extensions
    }

    pub fn get_extension_for_file(file_path: &str) -> Option<String> {
        std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn file_path_to_uri(path: &str) -> String {
    let absolute = if std::path::Path::new(path).is_absolute() {
        path.to_string()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path).to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string())
    };

    let normalized = absolute.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/');

    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
        format!("file:///{}", trimmed)
    } else if normalized.starts_with('/') {
        format!("file://{}", normalized)
    } else {
        format!("file:///{}", normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_path_to_uri() {
        let uri = file_path_to_uri("C:\\Users\\test\\project\\main.rs");
        assert!(uri.starts_with("file:///"));
        assert!(uri.contains("Users/test/project/main.rs"));
    }

    #[test]
    fn test_file_path_to_uri_forward_slash() {
        let uri = file_path_to_uri("C:/Users/test/project/main.rs");
        assert_eq!(uri, "file:///C:/Users/test/project/main.rs");
    }

    #[test]
    fn test_lsp_manager_creation() {
        let manager = LspManager::new();
        assert!(manager.clients.is_empty());
        assert!(manager.file_extensions.contains_key(".rs"));
        assert!(manager.file_extensions.contains_key(".ts"));
        assert!(manager.file_extensions.contains_key(".py"));
        assert!(manager.file_extensions.contains_key(".go"));
        assert_eq!(
            manager.file_extensions.get(".rs").unwrap(),
            "rust-analyzer"
        );
    }

    #[test]
    fn test_lsp_position_serialization() {
        let pos = LspPosition {
            line: 10,
            character: 5,
        };
        let json = serde_json::to_string(&pos).unwrap();
        assert!(json.contains("\"line\":10"));
        assert!(json.contains("\"character\":5"));

        let deserialized: LspPosition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.line, 10);
        assert_eq!(deserialized.character, 5);
    }

    #[test]
    fn test_lsp_range_serialization() {
        let range = LspRange {
            start: LspPosition {
                line: 1,
                character: 0,
            },
            end: LspPosition {
                line: 1,
                character: 10,
            },
        };
        let json = serde_json::to_string(&range).unwrap();
        let deserialized: LspRange = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.start.line, 1);
        assert_eq!(deserialized.end.character, 10);
    }

    #[test]
    fn test_lsp_location_serialization() {
        let loc = LspLocation {
            uri: "file:///test.rs".to_string(),
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 0,
                    character: 5,
                },
            },
        };
        let json = serde_json::to_string(&loc).unwrap();
        let deserialized: LspLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.uri, "file:///test.rs");
    }

    #[test]
    fn test_lsp_symbol_serialization() {
        let symbol = LspSymbol {
            name: "main".to_string(),
            kind: 12,
            range: LspRange {
                start: LspPosition {
                    line: 0,
                    character: 0,
                },
                end: LspPosition {
                    line: 10,
                    character: 1,
                },
            },
            children: None,
        };
        let json = serde_json::to_string(&symbol).unwrap();
        let deserialized: LspSymbol = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "main");
        assert_eq!(deserialized.kind, 12);
        assert!(deserialized.children.is_none());
    }

    #[test]
    fn test_lsp_hover_serialization() {
        let hover = LspHover {
            contents: vec![
                LspMarkedString {
                    language: Some("rust".to_string()),
                    value: "fn main()".to_string(),
                },
                LspMarkedString {
                    language: None,
                    value: "The entry point".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&hover).unwrap();
        let deserialized: LspHover = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.contents.len(), 2);
        assert_eq!(deserialized.contents[0].language.as_deref(), Some("rust"));
    }

    #[test]
    fn test_lsp_client_not_connected_when_spawn_fails() {
        let client = LspClient::new("nonexistent_command_xyz_12345", &[]);
        assert!(!client.is_connected());
    }

    #[test]
    fn test_lsp_manager_get_extension() {
        assert_eq!(
            LspManager::get_extension_for_file("src/main.rs"),
            Some(".rs".to_string())
        );
        assert_eq!(
            LspManager::get_extension_for_file("app.tsx"),
            Some(".tsx".to_string())
        );
        assert_eq!(LspManager::get_extension_for_file("Makefile"), None);
    }

    #[tokio::test]
    async fn test_lsp_manager_shutdown_all_empty() {
        let mut manager = LspManager::new();
        let result = manager.shutdown_all().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_lsp_manager_registered_servers_empty() {
        let manager = LspManager::new();
        assert!(manager.registered_servers().is_empty());
        assert!(!manager.is_server_registered("rust-analyzer"));
    }

    #[test]
    fn test_lsp_marked_string_no_language_serialization() {
        let ms = LspMarkedString {
            language: None,
            value: "hello".to_string(),
        };
        let json = serde_json::to_string(&ms).unwrap();
        assert!(!json.contains("language"));
    }
}
