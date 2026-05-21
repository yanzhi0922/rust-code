//! Token rotation manager — short-lived access tokens with automatic refresh.

use std::sync::Arc;

use tokio::sync::RwLock;

/// Token pair returned by the auth server.
#[derive(Debug, Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Manages token lifecycle with automatic rotation.
#[derive(Debug, Clone)]
pub struct TokenManager {
    inner: Arc<RwLock<TokenState>>,
    /// How many seconds before expiry to attempt a refresh.
    refresh_buffer_secs: i64,
    /// Control plane base URL for token refresh.
    control_plane_url: Option<String>,
}

#[derive(Debug)]
enum TokenState {
    /// No token yet.
    None,
    /// Have a valid token.
    Valid(TokenPair),
    /// Token is expired, need to re-authenticate.
    Expired,
}

impl TokenManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(TokenState::None)),
            refresh_buffer_secs: 120,
            control_plane_url: None,
        }
    }

    /// Set the control plane URL for token refresh calls.
    pub fn set_control_plane_url(&mut self, url: String) {
        self.control_plane_url = Some(url);
    }

    /// Store a new token pair.
    pub async fn set_token(&self, pair: TokenPair) {
        *self.inner.write().await = TokenState::Valid(pair);
    }

    /// Get the current access token, attempting refresh if needed.
    /// Returns None if the token is expired and no refresh token is available.
    pub async fn access_token(&self) -> Option<String> {
        let state = self.inner.read().await;
        match &*state {
            TokenState::None => None,
            TokenState::Expired => None,
            TokenState::Valid(pair) => {
                let now = chrono::Utc::now();
                let refresh_at =
                    pair.expires_at - chrono::Duration::seconds(self.refresh_buffer_secs);
                if now >= refresh_at {
                    // Need refresh — drop read lock and try to refresh.
                    drop(state);
                    self.try_refresh().await
                } else {
                    Some(pair.access_token.clone())
                }
            }
        }
    }

    /// Mark the current token as expired (e.g., on 401 response).
    pub async fn mark_expired(&self) {
        *self.inner.write().await = TokenState::Expired;
    }

    /// Clear all tokens (logout).
    pub async fn clear(&self) {
        *self.inner.write().await = TokenState::None;
    }

    async fn try_refresh(&self) -> Option<String> {
        let state = self.inner.read().await;
        let TokenState::Valid(pair) = &*state else {
            return None;
        };

        let refresh_token = pair.refresh_token.clone()?;
        let cp_url = self.control_plane_url.clone()?;

        // Drop read lock before making HTTP call.
        drop(state);

        let client = reqwest::Client::new();
        let url = format!("{cp_url}/v1/auth/refresh");

        let result = client
            .post(&url)
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                #[derive(serde::Deserialize)]
                struct RefreshResponse {
                    access_token: String,
                }
                match response.json::<RefreshResponse>().await {
                    Ok(body) => {
                        let mut state = self.inner.write().await;
                        if let TokenState::Valid(pair) = &mut *state {
                            pair.access_token = body.access_token.clone();
                        }
                        Some(body.access_token)
                    }
                    Err(e) => {
                        tracing::warn!("token refresh response parse error: {e}");
                        let mut state = self.inner.write().await;
                        *state = TokenState::Expired;
                        None
                    }
                }
            }
            Ok(response) => {
                let status = response.status();
                tracing::warn!("token refresh failed with HTTP {status}");
                let mut state = self.inner.write().await;
                *state = TokenState::Expired;
                None
            }
            Err(e) => {
                tracing::warn!("token refresh network error: {e}");
                // Don't mark expired on network errors — might be transient.
                None
            }
        }
    }
}

impl Default for TokenManager {
    fn default() -> Self {
        Self::new()
    }
}
