//! Local HTTP server that listens for OAuth authorization-code redirects.
//!
//! When the user authorizes in their browser, the OAuth provider redirects to
//! `http://localhost:{port}/callback?code=AUTH_CODE&state=STATE`. This module
//! captures that redirect and extracts the auth code.
//!
//! Mirrors `services/oauth/auth-code-listener.ts`.

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::types::OAuthConfig;

/// Errors that can occur while listening for an OAuth callback.
#[derive(Debug, thiserror::Error)]
pub enum AuthCodeListenerError {
    #[error("failed to bind OAuth callback server: {0}")]
    BindFailed(String),

    #[error("no authorization code received")]
    NoAuthCode,

    #[error("invalid state parameter (CSRF mismatch)")]
    InvalidState,

    #[error("callback listener timed out")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result of a successful callback capture.
#[derive(Debug)]
pub struct CallbackResult {
    pub authorization_code: String,
    pub state: String,
    /// Whether this was an automatic (browser-redirect) flow.
    pub is_automatic: bool,
}

/// A local HTTP server that listens for the OAuth redirect.
pub struct AuthCodeListener {
    port: u16,
    listener: TcpListener,
    config: Arc<OAuthConfig>,
}

impl AuthCodeListener {
    /// Bind to an OS-assigned port on localhost.
    pub async fn start(config: Arc<OAuthConfig>) -> Result<Self, AuthCodeListenerError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AuthCodeListenerError::BindFailed(e.to_string()))?;
        let port = listener
            .local_addr()
            .expect("listener has local addr")
            .port();
        Ok(Self {
            port,
            listener,
            config,
        })
    }

    /// Bind to a specific port on localhost.
    pub async fn start_on_port(
        port: u16,
        config: Arc<OAuthConfig>,
    ) -> Result<Self, AuthCodeListenerError> {
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| AuthCodeListenerError::BindFailed(e.to_string()))?;
        Ok(Self {
            port,
            listener,
            config,
        })
    }

    /// The port the listener is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wait for a single OAuth callback, validating the `state` parameter.
    ///
    /// `on_ready` is called once the listener is prepared to accept
    /// connections — the caller should open the browser at this point.
    pub async fn wait_for_authorization(
        &self,
        expected_state: &str,
        on_ready: Box<dyn FnOnce() + Send>,
    ) -> Result<CallbackResult, AuthCodeListenerError> {
        let (tx, rx) = oneshot::channel::<CallbackResult>();
        let expected_state = expected_state.to_owned();
        let success_url = self.config.claudeai_success_url.clone();

        on_ready();

        // Accept a single connection
        let (stream, _) = self.listener.accept().await?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 4096];
        let mut stream = stream;
        let n = stream
            .read(&mut buf)
            .await
            .map_err(AuthCodeListenerError::Io)?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse the HTTP request line to extract the path + query
        let first_line = request.lines().next().expect("request has a first line");
        let path = first_line
            .split_whitespace()
            .nth(1)
            .expect("HTTP request has a path");

        let (code, state) = parse_callback_query(path);

        let is_automatic = code.is_some() && state.as_deref() == Some(&expected_state);

        match (code, state) {
            (Some(auth_code), Some(received_state)) if received_state == expected_state => {
                // Redirect browser to success page
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {success_url}\r\nContent-Length: 0\r\n\r\n"
                );
                stream.write_all(response.as_bytes()).await?;
                stream.flush().await?;

                let _ = tx.send(CallbackResult {
                    authorization_code: auth_code,
                    state: received_state,
                    is_automatic,
                });
            }
            (Some(_), _) => {
                let body = "Invalid state parameter";
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await?;
                stream.flush().await?;
                return Err(AuthCodeListenerError::InvalidState);
            }
            (None, _) => {
                let body = "Authorization code not found";
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await?;
                stream.flush().await?;
                return Err(AuthCodeListenerError::NoAuthCode);
            }
        }

        rx.await.map_err(|_| AuthCodeListenerError::NoAuthCode)
    }
}

/// Parse `code` and `state` from a callback URL path like
/// `/callback?code=ABC&state=XYZ`.
fn parse_callback_query(path: &str) -> (Option<String>, Option<String>) {
    let query = path.split('?').nth(1).expect("path has query");
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().expect("pair has key");
        let value = kv.next().map(str::to_owned);
        match key {
            "code" => code = value,
            "state" => state = value,
            _ => {}
        }
    }
    (code, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_query_basic() {
        let (code, state) = parse_callback_query("/callback?code=ABC123&state=XYZ789");
        assert_eq!(code.as_deref(), Some("ABC123"));
        assert_eq!(state.as_deref(), Some("XYZ789"));
    }

    #[test]
    fn parse_callback_query_missing_code() {
        let (code, state) = parse_callback_query("/callback?state=XYZ789");
        assert!(code.is_none());
        assert_eq!(state.as_deref(), Some("XYZ789"));
    }
}
