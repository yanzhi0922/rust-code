//! MCP OAuth authentication with PKCE flow and token management.
//!
//! Implements the OAuth 2.0 Authorization Code flow with PKCE (Proof Key for
//! Code Exchange) for MCP servers that require authentication. Supports token
//! persistence, refresh, and automatic expiry detection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::McpServerConfig;
use crate::error::McpRuntimeError;
use crate::transport::{McpOAuthConfig, McpTransportConfig};

// ── PKCE parameters ─────────────────────────────────────────────────────────

/// PKCE (Proof Key for Code Exchange) parameters for OAuth flow.
#[derive(Debug, Clone)]
pub struct PkceParams {
    /// Random verifier string (43-128 chars, URL-safe base64).
    pub code_verifier: String,
    /// SHA-256 hash of the verifier, base64url-encoded.
    pub code_challenge: String,
    /// Challenge method (always "S256").
    pub code_challenge_method: String,
}

// ── OAuth tokens ────────────────────────────────────────────────────────────

/// OAuth token set returned by the authorization server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Access token for API calls.
    pub access_token: String,
    /// Refresh token for obtaining new access tokens.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Token expiry as Unix timestamp (seconds).
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Token type (typically "Bearer").
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// Granted scopes.
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_owned()
}

/// Generate the OAuth storage key used for an MCP server.
///
/// Claude Code keys remote MCP OAuth credentials by server name plus a stable
/// hash of the transport identity, so credentials are not accidentally reused
/// when a server is renamed in-place or a URL changes. Local transports fall
/// back to the server name because they do not participate in MCP OAuth.
#[must_use]
pub fn mcp_oauth_server_key(server_name: &str, server_config: &McpServerConfig) -> String {
    let Some(identity) = remote_transport_identity(server_config) else {
        return server_name.to_owned();
    };
    let config_json = serde_json::to_string(&identity).unwrap_or_else(|_| identity.to_string());
    let mut hasher = Sha256::new();
    hasher.update(config_json.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    format!("{server_name}|{}", &hash[..16])
}

fn remote_transport_identity(server_config: &McpServerConfig) -> Option<serde_json::Value> {
    match &server_config.transport {
        McpTransportConfig::Http { url, headers, .. } => Some(json!({
            "type": "http",
            "url": url,
            "headers": headers,
        })),
        McpTransportConfig::WebSocket { url, headers, .. } => Some(json!({
            "type": "ws",
            "url": url,
            "headers": headers,
        })),
        McpTransportConfig::Sse { url, headers, .. } => Some(json!({
            "type": "sse",
            "url": url,
            "headers": headers,
        })),
        McpTransportConfig::Stdio { .. } => None,
        _ => None,
    }
}

// ── Authorization server metadata ───────────────────────────────────────────

/// OAuth 2.0 Authorization Server Metadata (RFC 8414).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    /// URL of the authorization endpoint.
    pub authorization_endpoint: String,
    /// URL of the token endpoint.
    pub token_endpoint: String,
    /// URL of the dynamic client registration endpoint.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// Scopes supported by the server.
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
    /// PKCE code challenge methods supported.
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
    /// Grant types supported.
    #[serde(default)]
    pub grant_types_supported: Option<Vec<String>>,
    /// Response types supported.
    #[serde(default)]
    pub response_types_supported: Option<Vec<String>>,
}

// ── OAuth flow ──────────────────────────────────────────────────────────────

/// MCP OAuth authentication flow handler.
///
/// Orchestrates the full OAuth 2.0 Authorization Code + PKCE flow:
/// 1. Discover authorization server metadata
/// 2. Generate PKCE parameters
/// 3. Build authorization URL
/// 4. Wait for callback with authorization code
/// 5. Exchange code for tokens
/// 6. Persist and refresh tokens as needed
#[derive(Debug, Clone)]
pub struct McpOAuthFlow {
    /// OAuth client ID.
    client_id: Option<String>,
    /// Local port for the redirect URI callback server.
    #[allow(dead_code)] // used by wait_for_callback callers to determine port
    callback_port: Option<u16>,
    /// Authorization server metadata URL for discovery.
    auth_server_metadata_url: Option<String>,
    /// Whether Cross-App Access (XAA) is enabled.
    xaa_enabled: bool,
}

impl McpOAuthFlow {
    /// Create a new OAuth flow from the given configuration.
    #[must_use]
    pub fn new(config: &McpOAuthConfig) -> Self {
        Self {
            client_id: config.client_id.clone(),
            callback_port: config.callback_port,
            auth_server_metadata_url: config.auth_server_metadata_url.clone(),
            xaa_enabled: config.xaa.unwrap_or(false),
        }
    }

    /// Create with explicit values (for testing).
    #[must_use]
    pub fn with_values(
        client_id: Option<String>,
        callback_port: Option<u16>,
        auth_server_metadata_url: Option<String>,
        xaa_enabled: bool,
    ) -> Self {
        Self {
            client_id,
            callback_port,
            auth_server_metadata_url,
            xaa_enabled,
        }
    }

    /// Whether XAA (Cross-App Access) is enabled.
    #[must_use]
    pub fn xaa_enabled(&self) -> bool {
        self.xaa_enabled
    }

    /// Generate PKCE parameters (code_verifier + code_challenge).
    ///
    /// The verifier is 32 random bytes, base64url-encoded (no padding).
    /// The challenge is SHA-256(verifier), base64url-encoded (no padding).
    pub fn generate_pkce() -> PkceParams {
        let mut rng = rand::rng();
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes);
        let code_verifier = URL_SAFE_NO_PAD.encode(bytes);

        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        PkceParams {
            code_verifier,
            code_challenge,
            code_challenge_method: "S256".to_owned(),
        }
    }

    /// Discover OAuth authorization server metadata from a well-known URL.
    ///
    /// Fetches `<server_url>/.well-known/oauth-authorization-server` and
    /// parses the response as [`AuthorizationServerMetadata`].
    pub async fn discover_metadata(
        &self,
        server_url: &str,
    ) -> Result<AuthorizationServerMetadata, McpRuntimeError> {
        let metadata_url = self
            .auth_server_metadata_url
            .as_deref()
            // Default: append .well-known path
            .unwrap_or("");

        let url = if metadata_url.is_empty() {
            format!("{server_url}/.well-known/oauth-authorization-server")
        } else {
            metadata_url.to_owned()
        };

        let response = reqwest::get(&url)
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: server_url.to_owned(),
                message: format!("metadata discovery failed: {e}"),
            })?;

        if !response.status().is_success() {
            return Err(McpRuntimeError::OAuth {
                server: server_url.to_owned(),
                message: format!("metadata discovery returned status {}", response.status()),
            });
        }

        response
            .json::<AuthorizationServerMetadata>()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: server_url.to_owned(),
                message: format!("metadata parse failed: {e}"),
            })
    }

    /// Build the authorization URL with PKCE parameters.
    ///
    /// Constructs the full URL that the user should visit to authorize
    /// the application.
    #[must_use]
    pub fn build_authorization_url(
        &self,
        metadata: &AuthorizationServerMetadata,
        pkce: &PkceParams,
        state: &str,
        redirect_uri: &str,
    ) -> String {
        let client_id = self.client_id.as_deref().unwrap_or("mcp-client");
        let mut url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method={}",
            metadata.authorization_endpoint,
            client_id,
            urlencoding(redirect_uri),
            state,
            pkce.code_challenge,
            pkce.code_challenge_method,
        );
        if let Some(scopes) = &metadata.scopes_supported
            && !scopes.is_empty()
        {
            url.push_str("&scope=");
            url.push_str(&scopes.join("+"));
        }
        url
    }

    /// Start a localhost HTTP server and wait for the OAuth callback.
    ///
    /// Listens on the specified port for the redirect from the authorization
    /// server. Extracts the `code` query parameter from the request.
    pub async fn wait_for_callback(
        &self,
        port: u16,
        expected_state: &str,
    ) -> Result<String, McpRuntimeError> {
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: format!("failed to bind callback server on port {port}: {e}"),
            })?;

        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: format!("failed to accept callback connection: {e}"),
            })?;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 4096];
        let (mut read_half, mut write_half) = tokio::io::split(stream);
        let n = read_half
            .read(&mut buf)
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: format!("failed to read callback request: {e}"),
            })?;

        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse the request line: GET /callback?code=xxx&state=yyy HTTP/1.1
        let request_line = request.lines().next().unwrap_or("");
        let code =
            extract_query_param(request_line, "code").ok_or_else(|| McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: "callback request missing 'code' parameter".to_owned(),
            })?;

        let state = extract_query_param(request_line, "state").unwrap_or_default();
        if state != expected_state {
            return Err(McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: format!("state mismatch: expected {expected_state}, got {state}"),
            });
        }

        // Send a simple HTML response
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Authorization successful!</h1><p>You can close this tab.</p></body></html>";
        write_half
            .write_all(response.as_bytes())
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "callback".to_owned(),
                message: format!("failed to write callback response: {e}"),
            })?;

        Ok(code)
    }

    /// Exchange an authorization code for tokens.
    ///
    /// Makes a POST request to the token endpoint with the authorization
    /// code, PKCE verifier, and client credentials.
    pub async fn exchange_code_for_tokens(
        &self,
        metadata: &AuthorizationServerMetadata,
        code: &str,
        pkce: &PkceParams,
        redirect_uri: &str,
        client_id: &str,
    ) -> Result<OAuthTokens, McpRuntimeError> {
        let params = [
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", redirect_uri.to_owned()),
            ("client_id", client_id.to_owned()),
            ("code_verifier", pkce.code_verifier.clone()),
        ];

        let client = reqwest::Client::new();
        let response = client
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "token-exchange".to_owned(),
                message: format!("token exchange request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(McpRuntimeError::OAuth {
                server: "token-exchange".to_owned(),
                message: format!("token exchange returned {status}: {body}"),
            });
        }

        response
            .json::<OAuthTokens>()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "token-exchange".to_owned(),
                message: format!("token response parse failed: {e}"),
            })
    }

    /// Refresh an expired access token using the refresh token.
    pub async fn refresh_tokens(
        &self,
        metadata: &AuthorizationServerMetadata,
        tokens: &OAuthTokens,
        client_id: &str,
    ) -> Result<OAuthTokens, McpRuntimeError> {
        let refresh_token =
            tokens
                .refresh_token
                .as_deref()
                .ok_or_else(|| McpRuntimeError::OAuth {
                    server: "token-refresh".to_owned(),
                    message: "no refresh token available".to_owned(),
                })?;

        let params = [
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh_token.to_owned()),
            ("client_id", client_id.to_owned()),
        ];

        let client = reqwest::Client::new();
        let response = client
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "token-refresh".to_owned(),
                message: format!("token refresh request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(McpRuntimeError::OAuth {
                server: "token-refresh".to_owned(),
                message: format!("token refresh returned {status}: {body}"),
            });
        }

        response
            .json::<OAuthTokens>()
            .await
            .map_err(|e| McpRuntimeError::OAuth {
                server: "token-refresh".to_owned(),
                message: format!("token refresh response parse failed: {e}"),
            })
    }

    /// Check whether a token has expired.
    ///
    /// Returns `true` if the token's `expires_at` is in the past
    /// (with a 60-second buffer to handle clock skew).
    #[must_use]
    pub fn is_token_expired(tokens: &OAuthTokens) -> bool {
        match tokens.expires_at {
            Some(expires_at) => {
                let now = epoch_seconds();
                now >= expires_at - 60
            }
            None => false,
        }
    }
}

// ── Token store ─────────────────────────────────────────────────────────────

/// Persistent storage for OAuth tokens keyed by server name.
#[derive(Debug, Clone, Default)]
pub struct OAuthTokenStore {
    /// Server name → tokens.
    tokens: HashMap<String, OAuthTokens>,
    /// File path for persistence.
    store_path: PathBuf,
}

impl OAuthTokenStore {
    /// Create a new token store that persists to the given directory.
    ///
    /// The tokens file will be `<store_dir>/mcp-oauth-tokens.json`.
    pub fn new(store_dir: impl AsRef<Path>) -> Self {
        let store_path = store_dir.as_ref().join("mcp-oauth-tokens.json");
        Self {
            tokens: HashMap::new(),
            store_path,
        }
    }

    /// Save tokens for a server.
    pub fn save_token(&mut self, server_name: &str, tokens: OAuthTokens) {
        self.tokens.insert(server_name.to_owned(), tokens);
    }

    /// Get tokens for a server.
    #[must_use]
    pub fn get_token(&self, server_name: &str) -> Option<&OAuthTokens> {
        self.tokens.get(server_name)
    }

    /// Remove tokens for a server.
    pub fn remove_token(&mut self, server_name: &str) {
        self.tokens.remove(server_name);
    }

    /// Get the number of stored token sets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Check if the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Check if tokens exist for a server.
    #[must_use]
    pub fn contains(&self, server_name: &str) -> bool {
        self.tokens.contains_key(server_name)
    }

    /// Persist tokens to the file system.
    pub async fn persist(&self) -> Result<(), McpRuntimeError> {
        if let Some(parent) = self.store_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| McpRuntimeError::TokenStoreIo {
                    path: parent.to_owned(),
                    source: e,
                })?;
        }

        let json = serde_json::to_string_pretty(&self.tokens)
            .map_err(|e| McpRuntimeError::TokenStoreSerialize { source: e })?;

        tokio::fs::write(&self.store_path, json)
            .await
            .map_err(|e| McpRuntimeError::TokenStoreIo {
                path: self.store_path.clone(),
                source: e,
            })?;

        Ok(())
    }

    /// Load tokens from the file system.
    pub async fn load(&mut self) -> Result<(), McpRuntimeError> {
        if !self.store_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.store_path)
            .await
            .map_err(|e| McpRuntimeError::TokenStoreIo {
                path: self.store_path.clone(),
                source: e,
            })?;

        self.tokens = serde_json::from_str(&content)
            .map_err(|e| McpRuntimeError::TokenStoreSerialize { source: e })?;

        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Minimal URL-encoding for query parameter values.
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "+".to_owned(),
            'A'..='Z'
            | 'a'..='z'
            | '0'..='9'
            | '-'
            | '_'
            | '.'
            | '~'
            | '/'
            | ':'
            | '?'
            | '&'
            | '='
            | '%' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// Extract a query parameter value from a URL string.
fn extract_query_param(url: &str, param: &str) -> Option<String> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    let fragment_end = query.find(" HTTP").unwrap_or(query.len());
    let query = &query[..fragment_end];

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next()?;
        if key == param {
            return kv.next().map(|v| v.to_owned());
        }
    }
    None
}

/// Get current epoch seconds (simple, no chrono dependency).
fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_pkce_produces_valid_params() {
        let pkce = McpOAuthFlow::generate_pkce();
        assert_eq!(pkce.code_challenge_method, "S256");
        // Verifier should be 43 chars (32 bytes → base64url no pad)
        assert_eq!(pkce.code_verifier.len(), 43);
        // Challenge should be 43 chars (32 bytes SHA-256 → base64url no pad)
        assert_eq!(pkce.code_challenge.len(), 43);
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let pkce = McpOAuthFlow::generate_pkce();
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn pkce_verifier_is_url_safe() {
        let pkce = McpOAuthFlow::generate_pkce();
        assert!(
            pkce.code_verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn build_authorization_url_includes_required_params() {
        let flow =
            McpOAuthFlow::with_values(Some("test-client".to_owned()), Some(8080), None, false);
        let metadata = AuthorizationServerMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            scopes_supported: Some(vec!["read".to_owned(), "write".to_owned()]),
            code_challenge_methods_supported: None,
            grant_types_supported: None,
            response_types_supported: None,
        };
        let pkce = McpOAuthFlow::generate_pkce();
        let url = flow.build_authorization_url(
            &metadata,
            &pkce,
            "test-state",
            "http://localhost:8080/callback",
        );

        assert!(url.contains("client_id=test-client"));
        assert!(url.contains("state=test-state"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=read+write"));
    }

    #[test]
    fn is_token_expired_with_past_expiry() {
        let tokens = OAuthTokens {
            access_token: "abc".to_owned(),
            refresh_token: None,
            expires_at: Some(1000), // long past
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        assert!(McpOAuthFlow::is_token_expired(&tokens));
    }

    #[test]
    fn is_token_not_expired_with_future_expiry() {
        let future = epoch_seconds() + 3600;
        let tokens = OAuthTokens {
            access_token: "abc".to_owned(),
            refresh_token: None,
            expires_at: Some(future),
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        assert!(!McpOAuthFlow::is_token_expired(&tokens));
    }

    #[test]
    fn is_token_not_expired_with_no_expiry() {
        let tokens = OAuthTokens {
            access_token: "abc".to_owned(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        assert!(!McpOAuthFlow::is_token_expired(&tokens));
    }

    #[test]
    fn token_store_save_and_get() {
        let mut store = OAuthTokenStore::new("/tmp/test-mcp-tokens");
        let tokens = OAuthTokens {
            access_token: "at-123".to_owned(),
            refresh_token: Some("rt-456".to_owned()),
            expires_at: Some(9999),
            token_type: "Bearer".to_owned(),
            scope: Some("read write".to_owned()),
        };
        store.save_token("my-server", tokens.clone());
        assert!(store.contains("my-server"));
        assert_eq!(store.len(), 1);

        let retrieved = store.get_token("my-server").expect("should exist");
        assert_eq!(retrieved.access_token, "at-123");
        assert_eq!(retrieved.refresh_token.as_deref(), Some("rt-456"));
    }

    #[test]
    fn token_store_remove() {
        let mut store = OAuthTokenStore::new("/tmp/test-mcp-tokens");
        let tokens = OAuthTokens {
            access_token: "at".to_owned(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        store.save_token("srv", tokens);
        assert!(store.contains("srv"));
        store.remove_token("srv");
        assert!(!store.contains("srv"));
        assert!(store.is_empty());
    }

    #[test]
    fn token_store_persist_and_load() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let mut store = OAuthTokenStore::new(dir.path());
        let tokens = OAuthTokens {
            access_token: "persist-test".to_owned(),
            refresh_token: Some("refresh-test".to_owned()),
            expires_at: Some(12345),
            token_type: "Bearer".to_owned(),
            scope: Some("scope1".to_owned()),
        };
        store.save_token("server1", tokens);

        let rt = tokio::runtime::Runtime::new().expect("create runtime");
        rt.block_on(store.persist()).expect("persist");

        let mut store2 = OAuthTokenStore::new(dir.path());
        rt.block_on(store2.load()).expect("load");

        let loaded = store2.get_token("server1").expect("should exist");
        assert_eq!(loaded.access_token, "persist-test");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-test"));
        assert_eq!(loaded.expires_at, Some(12345));
    }

    #[test]
    fn oauth_tokens_serde_roundtrip() {
        let tokens = OAuthTokens {
            access_token: "at".to_owned(),
            refresh_token: Some("rt".to_owned()),
            expires_at: Some(42),
            token_type: "Bearer".to_owned(),
            scope: Some("read".to_owned()),
        };
        let json = serde_json::to_string(&tokens).expect("serialize");
        let back: OAuthTokens = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.access_token, "at");
        assert_eq!(back.refresh_token.as_deref(), Some("rt"));
        assert_eq!(back.expires_at, Some(42));
        assert_eq!(back.scope.as_deref(), Some("read"));
    }

    #[test]
    fn authorization_server_metadata_serde_roundtrip() {
        let metadata = AuthorizationServerMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: Some("https://auth.example.com/register".to_owned()),
            scopes_supported: Some(vec!["read".to_owned()]),
            code_challenge_methods_supported: Some(vec!["S256".to_owned()]),
            grant_types_supported: Some(vec!["authorization_code".to_owned()]),
            response_types_supported: Some(vec!["code".to_owned()]),
        };
        let json = serde_json::to_string(&metadata).expect("serialize");
        let back: AuthorizationServerMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.authorization_endpoint, metadata.authorization_endpoint);
        assert_eq!(back.token_endpoint, metadata.token_endpoint);
        assert_eq!(back.registration_endpoint, metadata.registration_endpoint);
    }

    #[test]
    fn oauth_flow_new_from_config() {
        let config = McpOAuthConfig {
            client_id: Some("my-client".to_owned()),
            callback_port: Some(9090),
            auth_server_metadata_url: Some("https://example.com/.well-known/oauth".to_owned()),
            xaa: Some(true),
        };
        let flow = McpOAuthFlow::new(&config);
        assert!(flow.xaa_enabled());
    }

    #[test]
    fn extract_query_param_finds_code() {
        let request = "GET /callback?code=abc123&state=xyz HTTP/1.1";
        assert_eq!(
            extract_query_param(request, "code"),
            Some("abc123".to_owned())
        );
        assert_eq!(
            extract_query_param(request, "state"),
            Some("xyz".to_owned())
        );
    }

    #[test]
    fn extract_query_param_returns_none_for_missing() {
        let request = "GET /callback?code=abc HTTP/1.1";
        assert_eq!(extract_query_param(request, "state"), None);
    }

    #[test]
    fn urlencoding_handles_spaces() {
        assert_eq!(urlencoding("hello world"), "hello+world");
    }

    // ── Enhanced OAuth tests ──────────────────────────────────────────────

    #[test]
    fn pkce_params_are_unique() {
        let pkce1 = McpOAuthFlow::generate_pkce();
        let pkce2 = McpOAuthFlow::generate_pkce();
        // Verifiers should be different (extremely unlikely to collide)
        assert_ne!(pkce1.code_verifier, pkce2.code_verifier);
        assert_ne!(pkce1.code_challenge, pkce2.code_challenge);
    }

    #[test]
    fn oauth_flow_with_values_constructor() {
        let flow =
            McpOAuthFlow::with_values(Some("client-123".to_owned()), Some(3000), None, false);
        assert!(!flow.xaa_enabled());
    }

    #[test]
    fn build_authorization_url_without_scopes() {
        let flow =
            McpOAuthFlow::with_values(Some("test-client".to_owned()), Some(8080), None, false);
        let metadata = AuthorizationServerMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            scopes_supported: None,
            code_challenge_methods_supported: None,
            grant_types_supported: None,
            response_types_supported: None,
        };
        let pkce = McpOAuthFlow::generate_pkce();
        let url = flow.build_authorization_url(
            &metadata,
            &pkce,
            "mystate",
            "http://localhost:8080/callback",
        );
        assert!(!url.contains("scope="));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn build_authorization_url_with_empty_scopes() {
        let flow =
            McpOAuthFlow::with_values(Some("test-client".to_owned()), Some(8080), None, false);
        let metadata = AuthorizationServerMetadata {
            authorization_endpoint: "https://auth.example.com/authorize".to_owned(),
            token_endpoint: "https://auth.example.com/token".to_owned(),
            registration_endpoint: None,
            scopes_supported: Some(vec![]),
            code_challenge_methods_supported: None,
            grant_types_supported: None,
            response_types_supported: None,
        };
        let pkce = McpOAuthFlow::generate_pkce();
        let url = flow.build_authorization_url(
            &metadata,
            &pkce,
            "state",
            "http://localhost:8080/callback",
        );
        assert!(!url.contains("scope="));
    }

    #[test]
    fn token_store_multiple_servers() {
        let mut store = OAuthTokenStore::new("/tmp/test-multi");
        for i in 0..5 {
            store.save_token(
                &format!("server-{i}"),
                OAuthTokens {
                    access_token: format!("at-{i}"),
                    refresh_token: Some(format!("rt-{i}")),
                    expires_at: Some(1000 + i as i64),
                    token_type: "Bearer".to_owned(),
                    scope: None,
                },
            );
        }
        assert_eq!(store.len(), 5);
        assert!(!store.is_empty());

        // Remove one
        store.remove_token("server-2");
        assert_eq!(store.len(), 4);
        assert!(!store.contains("server-2"));
    }

    #[test]
    fn token_store_overwrite() {
        let mut store = OAuthTokenStore::new("/tmp/test-overwrite");
        store.save_token(
            "srv",
            OAuthTokens {
                access_token: "old-token".to_owned(),
                refresh_token: None,
                expires_at: None,
                token_type: "Bearer".to_owned(),
                scope: None,
            },
        );
        store.save_token(
            "srv",
            OAuthTokens {
                access_token: "new-token".to_owned(),
                refresh_token: Some("refresh".to_owned()),
                expires_at: Some(9999),
                token_type: "Bearer".to_owned(),
                scope: None,
            },
        );
        let token = store.get_token("srv").expect("should exist");
        assert_eq!(token.access_token, "new-token");
    }

    #[test]
    fn is_token_expired_near_boundary() {
        // Token expires in 30 seconds — should NOT be expired (60s buffer)
        let future = epoch_seconds() + 30;
        let tokens = OAuthTokens {
            access_token: "at".to_owned(),
            refresh_token: None,
            expires_at: Some(future),
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        assert!(McpOAuthFlow::is_token_expired(&tokens));

        // Token expires in 120 seconds — should NOT be expired
        let future2 = epoch_seconds() + 120;
        let tokens2 = OAuthTokens {
            access_token: "at".to_owned(),
            refresh_token: None,
            expires_at: Some(future2),
            token_type: "Bearer".to_owned(),
            scope: None,
        };
        assert!(!McpOAuthFlow::is_token_expired(&tokens2));
    }

    #[test]
    fn oauth_tokens_default_token_type() {
        let tokens: OAuthTokens =
            serde_json::from_str(r#"{"access_token":"abc"}"#).expect("deserialize");
        assert_eq!(tokens.token_type, "Bearer");
    }

    #[test]
    fn oauth_server_key_changes_with_remote_url() {
        let make_server = |url: &str| McpServerConfig {
            name: "docs".to_owned(),
            enabled: true,
            transport: McpTransportConfig::Http {
                url: url.to_owned(),
                headers: Default::default(),
                headers_helper: None,
            },
            capabilities: Default::default(),
            startup_timeout_secs: None,
            request_timeout_secs: None,
            metadata: Default::default(),
            oauth: None,
            tool_policy: crate::tool_policy::McpToolPolicy::default(),
        };

        let first = mcp_oauth_server_key("docs", &make_server("https://one.example/mcp"));
        let second = mcp_oauth_server_key("docs", &make_server("https://two.example/mcp"));

        assert!(first.starts_with("docs|"));
        assert_ne!(first, second);
    }

    #[test]
    fn authorization_metadata_defaults() {
        let json = r#"{
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token"
        }"#;
        let metadata: AuthorizationServerMetadata =
            serde_json::from_str(json).expect("deserialize");
        assert!(metadata.registration_endpoint.is_none());
        assert!(metadata.scopes_supported.is_none());
        assert!(metadata.code_challenge_methods_supported.is_none());
    }

    #[test]
    fn extract_query_param_handles_no_query() {
        let result = extract_query_param("GET /path HTTP/1.1", "code");
        assert!(result.is_none());
    }

    #[test]
    fn urlencoding_preserves_safe_chars() {
        let encoded = urlencoding("https://example.com/path?a=1&b=2");
        assert_eq!(encoded, "https://example.com/path?a=1&b=2");
    }

    #[test]
    fn urlencoding_encodes_special() {
        let encoded = urlencoding("hello@world!");
        assert!(encoded.contains("%40")); // @
        assert!(encoded.contains("%21")); // !
    }

    #[test]
    fn token_store_load_nonexistent() {
        let rt = tokio::runtime::Runtime::new().expect("create runtime");
        let mut store = OAuthTokenStore::new("/tmp/nonexistent-path-xyz");
        let result = rt.block_on(store.load());
        assert!(result.is_ok());
        assert!(store.is_empty());
    }

    #[test]
    fn oauth_flow_xaa_toggle() {
        let flow_enabled = McpOAuthFlow::with_values(None, None, None, true);
        assert!(flow_enabled.xaa_enabled());

        let flow_disabled = McpOAuthFlow::with_values(None, None, None, false);
        assert!(!flow_disabled.xaa_enabled());
    }
}
