//! MCP error types for configuration and runtime failures.

use std::path::PathBuf;

use thiserror::Error;

use crate::transport::McpTransport;

/// Errors that can occur while loading, parsing, or saving MCP configuration.
#[derive(Debug, Error)]
pub enum McpConfigError {
    #[error("failed to read MCP config at `{path}`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse MCP config TOML at `{path}`")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to parse MCP config JSON at `{path}`")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("MCP server `{name}` must define either `command` or `url`")]
    MissingTransport { name: String },
    #[error("MCP server `{name}` cannot define both `command` and `url`")]
    AmbiguousTransport { name: String },
    #[error("MCP server `{name}` uses unsupported url scheme `{scheme}`")]
    UnsupportedUrlScheme { name: String, scheme: String },
    #[error("failed to serialize MCP config")]
    Serialize {
        #[source]
        source: toml::ser::Error,
    },
    #[error("failed to write MCP config at `{path}`")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Errors that can occur during MCP server communication.
#[derive(Debug, Error)]
pub enum McpRuntimeError {
    #[error("MCP server `{server}` uses unsupported runtime transport `{transport:?}`")]
    UnsupportedTransport {
        server: String,
        transport: McpTransport,
    },
    #[error("{0}")]
    Transport(String),
    #[error("failed to spawn MCP server `{server}` using `{command}`")]
    Spawn {
        server: String,
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("MCP server `{server}` did not expose {pipe}")]
    MissingPipe { server: String, pipe: &'static str },
    #[error("failed to serialize JSON-RPC payload for MCP server `{server}` during {phase}")]
    Serialize {
        server: String,
        phase: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to write to MCP server `{server}` during {phase}")]
    Write {
        server: String,
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read from MCP server `{server}` during {phase}")]
    Read {
        server: String,
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for MCP server `{server}` during {phase} after {timeout_secs}s")]
    Timeout {
        server: String,
        phase: &'static str,
        timeout_secs: u64,
    },
    #[error("MCP server `{server}` closed stdout while waiting for {phase}")]
    Closed { server: String, phase: &'static str },
    #[error("failed to decode JSON from MCP server `{server}` during {phase}")]
    Decode {
        server: String,
        phase: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("MCP server `{server}` returned an invalid response during {phase}: {message}")]
    Protocol {
        server: String,
        phase: &'static str,
        message: String,
    },
    #[error("MCP server `{server}` returned JSON-RPC error {code}: {message}")]
    Rpc {
        server: String,
        code: i64,
        message: String,
    },
    #[error("HTTP request to MCP server `{server}` failed during {phase}")]
    Http {
        server: String,
        phase: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("MCP server `{server}` returned HTTP {status} during {phase}: {message}")]
    HttpError {
        server: String,
        phase: &'static str,
        status: u16,
        message: String,
    },
    #[error("MCP server `{server}` returned JSON-RPC error {code} during {phase}: {message}")]
    JsonRpc {
        server: String,
        phase: &'static str,
        code: i64,
        message: String,
    },
    // ── OAuth errors ───────────────────────────────────────────────────────
    #[error("OAuth error for `{server}`: {message}")]
    OAuth { server: String, message: String },
    #[error("token store I/O at `{path}`")]
    TokenStoreIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("token store serialization")]
    TokenStoreSerialize {
        #[source]
        source: serde_json::Error,
    },
    // ── Proxy errors ───────────────────────────────────────────────────────
    #[error("proxy error: {message}")]
    Proxy { message: String },
}

// ── Session expiry detection ─────────────────────────────────────────────────

/// JSON-RPC error code indicating the MCP session has expired.
///
/// When the server returns this code (typically accompanied by HTTP 404),
/// the client should tear down the current session and reconnect.
pub const MCP_SESSION_EXPIRED_CODE: i64 = -32001;

/// Check whether an error indicates that the MCP session has expired and
/// the client should reconnect.
///
/// Session expiry is detected by either:
/// - An HTTP 404 response
/// - A JSON-RPC error with code `-32001`
pub fn is_session_expired_error(error: &McpRuntimeError) -> bool {
    match error {
        McpRuntimeError::HttpError { status, .. } => *status == 404,
        McpRuntimeError::Rpc { code, .. } | McpRuntimeError::JsonRpc { code, .. } => {
            *code == MCP_SESSION_EXPIRED_CODE
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_read_formats_path() {
        let path = PathBuf::from("/tmp/mcp.toml");
        let err = McpConfigError::Read {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("/tmp/mcp.toml"),
            "error message should contain the path: {msg}"
        );
    }

    #[test]
    fn config_error_missing_transport_formats_name() {
        let err = McpConfigError::MissingTransport {
            name: "my-server".to_owned(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("my-server"),
            "error message should contain the server name: {msg}"
        );
    }

    #[test]
    fn config_error_ambiguous_transport_formats_name() {
        let err = McpConfigError::AmbiguousTransport {
            name: "dup-server".to_owned(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("dup-server"),
            "error message should contain the server name: {msg}"
        );
    }

    #[test]
    fn runtime_error_unsupported_transport() {
        let err = McpRuntimeError::UnsupportedTransport {
            server: "test".to_owned(),
            transport: McpTransport::Http,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("test") && msg.contains("Http"),
            "error message should contain server and transport: {msg}"
        );
    }

    #[test]
    fn runtime_error_rpc_formats_code_and_message() {
        let err = McpRuntimeError::Rpc {
            server: "svc".to_owned(),
            code: -32600,
            message: "invalid request".to_owned(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("-32600") && msg.contains("invalid request"),
            "error message should contain code and message: {msg}"
        );
    }

    #[test]
    fn runtime_error_timeout_formats_secs() {
        let err = McpRuntimeError::Timeout {
            server: "slow".to_owned(),
            phase: "initialize response",
            timeout_secs: 30,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("30s"),
            "error message should contain timeout: {msg}"
        );
    }
}
