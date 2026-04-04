use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const TOKEN_FILE: &str = "oauth_tokens.json";
const CONFIG_DIR: &str = ".claude";
const TOKEN_EXPIRY_BUFFER_SECS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub redirect_url: String,
    pub manual_redirect_url: String,
    pub profile_url: String,
    pub api_key_url: String,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            client_id: "claude-code".to_string(),
            authorize_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/oauth/token".to_string(),
            redirect_url: "http://localhost:{port}/oauth/callback".to_string(),
            manual_redirect_url: "https://console.anthropic.com/oauth/manual/callback"
                .to_string(),
            profile_url: "https://api.anthropic.com/api/oauth/profile".to_string(),
            api_key_url: "https://api.anthropic.com/api/oauth/api_key".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProfile {
    pub account_uuid: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_type: Option<String>,
}

pub struct OAuthService {
    config: OAuthConfig,
    code_verifier: String,
    http: reqwest::Client,
}

impl OAuthService {
    pub fn new() -> Self {
        Self {
            config: OAuthConfig::default(),
            code_verifier: generate_code_verifier(),
            http: reqwest::Client::new(),
        }
    }

    pub fn with_config(config: OAuthConfig) -> Self {
        Self {
            config,
            code_verifier: generate_code_verifier(),
            http: reqwest::Client::new(),
        }
    }

    pub fn generate_auth_url(&self, state: &str, port: u16, is_manual: bool) -> String {
        let redirect_uri = if is_manual {
            self.config.manual_redirect_url.clone()
        } else {
            self.config
                .redirect_url
                .replace("{port}", &port.to_string())
        };

        let code_challenge = generate_code_challenge(&self.code_verifier);

        let params = vec![
            ("response_type", "code".to_string()),
            ("client_id", self.config.client_id.clone()),
            ("redirect_uri", redirect_uri),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256".to_string()),
            ("state", state.to_string()),
        ];

        let query: String = params
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    percent_encode(k),
                    percent_encode(v)
                )
            })
            .collect::<Vec<_>>()
            .join("&");

        format!("{}?{}", self.config.authorize_url, query)
    }

    pub fn start_callback_server(
        port: u16,
    ) -> Result<tokio::task::JoinHandle<Result<(String, String)>>> {
        let handle = tokio::spawn(async move {
            let addr = format!("127.0.0.1:{port}");
            let listener = TcpListener::bind(&addr)
                .await
                .context("Failed to bind callback server")?;

            let (mut stream, _) = listener
                .accept()
                .await
                .context("Failed to accept connection")?;

            let mut buf = vec![0u8; 4096];
            let n = stream
                .read(&mut buf)
                .await
                .context("Failed to read request")?;

            let request = String::from_utf8_lossy(&buf[..n]);
            let first_line = request
                .lines()
                .next()
                .unwrap_or("");

            let path = first_line.split_whitespace().nth(1).unwrap_or("");
            let query_str = path.split('?').nth(1).unwrap_or("");

            let mut code_opt: Option<String> = None;
            let mut state_opt: Option<String> = None;

            for pair in query_str.split('&') {
                let mut kv = pair.splitn(2, '=');
                let key = kv.next().unwrap_or("");
                let val = kv.next().unwrap_or("");
                match key {
                    "code" => code_opt = Some(percent_decode(val).to_string()),
                    "state" => state_opt = Some(percent_decode(val).to_string()),
                    "error" => {
                        let error_desc = query_str
                            .split('&')
                            .find(|p| p.starts_with("error_description="))
                            .and_then(|p| p.splitn(2, '=').nth(1))
                            .unwrap_or(val);
                        let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authentication failed</h2><p>You can close this tab.</p></body></html>";
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.flush().await;
                        anyhow::bail!("OAuth error: {error_desc}");
                    }
                    _ => {}
                }
            }

            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authentication successful!</h2><p>You can close this tab and return to the terminal.</p></body></html>";
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;

            let code = code_opt.context("Missing authorization code in callback")?;
            let state = state_opt.context("Missing state in callback")?;

            Ok((code, state))
        });

        Ok(handle)
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        _state: &str,
        port: u16,
        is_manual: bool,
    ) -> Result<OAuthTokens> {
        let redirect_uri = if is_manual {
            self.config.manual_redirect_url.clone()
        } else {
            self.config
                .redirect_url
                .replace("{port}", &port.to_string())
        };

        #[derive(Serialize)]
        struct TokenRequest {
            grant_type: String,
            code: String,
            client_id: String,
            redirect_uri: String,
            code_verifier: String,
        }

        let resp = self
            .http
            .post(&self.config.token_url)
            .json(&TokenRequest {
                grant_type: "authorization_code".to_string(),
                code: code.to_string(),
                client_id: self.config.client_id.clone(),
                redirect_uri,
                code_verifier: self.code_verifier.clone(),
            })
            .send()
            .await
            .context("Failed to exchange authorization code")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Token exchange failed ({status}): {body}");
        }

        #[derive(Deserialize)]
        struct RawTokenResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            expires_in: i64,
            #[serde(default)]
            scope: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            subscription_type: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            rate_limit_tier: Option<String>,
        }

        let raw: RawTokenResponse = resp
            .json()
            .await
            .context("Failed to parse token response")?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let expires_at = now_ms + raw.expires_in * 1000;

        Ok(OAuthTokens {
            access_token: raw.access_token,
            refresh_token: raw.refresh_token.unwrap_or_default(),
            expires_at,
            scopes: raw
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            subscription_type: raw.subscription_type,
            rate_limit_tier: raw.rate_limit_tier,
        })
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthTokens> {
        #[derive(Serialize)]
        struct RefreshRequest {
            grant_type: String,
            refresh_token: String,
            client_id: String,
        }

        let resp = self
            .http
            .post(&self.config.token_url)
            .json(&RefreshRequest {
                grant_type: "refresh_token".to_string(),
                refresh_token: refresh_token.to_string(),
                client_id: self.config.client_id.clone(),
            })
            .send()
            .await
            .context("Failed to refresh token")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Token refresh failed ({status}): {body}");
        }

        #[derive(Deserialize)]
        struct RawTokenResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            expires_in: i64,
            #[serde(default)]
            scope: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            subscription_type: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            rate_limit_tier: Option<String>,
        }

        let raw: RawTokenResponse = resp
            .json()
            .await
            .context("Failed to parse refresh response")?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let expires_at = now_ms + raw.expires_in * 1000;

        Ok(OAuthTokens {
            access_token: raw.access_token,
            refresh_token: raw
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_string()),
            expires_at,
            scopes: raw
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            subscription_type: raw.subscription_type,
            rate_limit_tier: raw.rate_limit_tier,
        })
    }

    pub async fn fetch_profile(&self, access_token: &str) -> Result<OAuthProfile> {
        let resp = self
            .http
            .get(&self.config.profile_url)
            .bearer_auth(access_token)
            .send()
            .await
            .context("Failed to fetch profile")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Profile fetch failed ({status}): {body}");
        }

        resp.json()
            .await
            .context("Failed to parse profile response")
    }

    pub async fn create_api_key(&self, access_token: &str) -> Result<String> {
        #[derive(Serialize)]
        struct CreateApiKeyRequest {
            grant_type: String,
        }

        let resp = self
            .http
            .post(&self.config.api_key_url)
            .bearer_auth(access_token)
            .json(&CreateApiKeyRequest {
                grant_type: "api_key".to_string(),
            })
            .send()
            .await
            .context("Failed to create API key")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("API key creation failed ({status}): {body}");
        }

        #[derive(Deserialize)]
        struct ApiKeyResponse {
            api_key: String,
        }

        let raw: ApiKeyResponse = resp
            .json()
            .await
            .context("Failed to parse API key response")?;

        Ok(raw.api_key)
    }

    pub fn is_token_expired(expires_at: Option<i64>) -> bool {
        match expires_at {
            Some(exp) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                now_ms >= (exp - TOKEN_EXPIRY_BUFFER_SECS * 1000)
            }
            None => true,
        }
    }
}

pub fn generate_code_verifier() -> String {
    let random_bytes: [u8; 32] = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut state = seed;
        let mut bytes = [0u8; 32];
        for byte in bytes.iter_mut() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *byte = ((state >> 33) & 0xFF) as u8;
        }
        bytes
    };
    URL_SAFE_NO_PAD.encode(random_bytes)
}

pub fn generate_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

pub fn generate_state() -> String {
    let random_bytes: [u8; 32] = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut state = seed.wrapping_add(9876543210);
        let mut bytes = [0u8; 32];
        for byte in bytes.iter_mut() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *byte = ((state >> 33) & 0xFF) as u8;
        }
        bytes
    };
    URL_SAFE_NO_PAD.encode(random_bytes)
}

pub struct OAuthTokenStorage;

impl OAuthTokenStorage {
    fn token_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(home.join(CONFIG_DIR).join(TOKEN_FILE))
    }

    pub fn save(tokens: &OAuthTokens) -> Result<()> {
        let path = Self::token_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(tokens)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn load() -> Result<Option<OAuthTokens>> {
        let path = Self::token_path()?;
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&path)?;
        let tokens: OAuthTokens = serde_json::from_str(&json)?;
        Ok(Some(tokens))
    }

    pub fn clear() -> Result<()> {
        let path = Self::token_path()?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

fn percent_encode(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let mut result = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte);
            }
        } else {
            result.push(c as u8);
        }
    }
    String::from_utf8(result).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_verifier_generation() {
        let verifier = generate_code_verifier();
        assert!(!verifier.is_empty(), "Code verifier should not be empty");
        assert!(
            verifier.len() >= 43,
            "Base64url-encoded 32 bytes should be at least 43 chars, got {}",
            verifier.len()
        );
        for c in verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "Invalid char in verifier: {c}"
            );
        }
    }

    #[test]
    fn test_code_challenge_derivation() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = generate_code_challenge(verifier);
        assert_eq!(
            challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "SHA256 + base64url must match RFC 7636 test vector"
        );
    }

    #[test]
    fn test_state_generation() {
        let state1 = generate_state();
        let state2 = generate_state();
        assert!(!state1.is_empty(), "State should not be empty");
        assert_eq!(
            state1.len(),
            state2.len(),
            "States should have consistent length"
        );
    }

    #[test]
    fn test_token_expiry_check() {
        let now_ms = chrono::Utc::now().timestamp_millis();

        assert!(
            OAuthService::is_token_expired(Some(now_ms - 600_000)),
            "Token expired in the past should be expired"
        );
        assert!(
            !OAuthService::is_token_expired(Some(now_ms + 600_000)),
            "Token in the future should not be expired"
        );
        assert!(
            OAuthService::is_token_expired(None),
            "None expiry should be considered expired"
        );

        let just_under_buffer = now_ms + (TOKEN_EXPIRY_BUFFER_SECS - 60) * 1000;
        assert!(
            OAuthService::is_token_expired(Some(just_under_buffer)),
            "Token within 5min buffer should be expired"
        );
    }

    #[test]
    fn test_auth_url_generation() {
        let service = OAuthService::new();
        let state = "test-state-123";
        let url = service.generate_auth_url(state, 8080, false);

        assert!(
            url.starts_with("https://console.anthropic.com/oauth/authorize?"),
            "URL should start with authorize endpoint"
        );
        assert!(
            url.contains("response_type=code"),
            "Should contain response_type=code"
        );
        assert!(
            url.contains("client_id=claude-code"),
            "Should contain client_id"
        );
        assert!(
            url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Foauth%2Fcallback"),
            "Should contain correctly encoded redirect_uri with port"
        );
        assert!(
            url.contains("code_challenge_method=S256"),
            "Should contain S256 method"
        );
        assert!(
            url.contains(&format!("state={}", state)),
            "Should contain state param"
        );

        let manual_url = service.generate_auth_url(state, 8080, true);
        assert!(
            manual_url.contains(
                "redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fmanual%2Fcallback"
            ),
            "Manual mode should use manual redirect URL"
        );
    }
}
