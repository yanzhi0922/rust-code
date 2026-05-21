//! Provider authentication dispatch.
//!
//! Resolves authentication credentials for each supported provider:
//! - Anthropic API Key (direct, env var, or apiKeyHelper)
//! - Anthropic OAuth (claude.ai tokens)
//! - AWS Bedrock (STS / credential export)
//! - GCP Vertex AI (gcloud access token)
//! - OpenAI-compatible (third-party API key + base URL)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::api_key_helper;
use crate::oauth::types::OAuthTokens;

/// The resolved authentication source for a provider request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthSource {
    /// Direct Anthropic API key.
    AnthropicApiKey { key: String },
    /// OAuth tokens from claude.ai login.
    OAuth { tokens: OAuthTokens },
    /// AWS Bedrock credentials.
    AwsBedrock {
        region: String,
        credentials: AwsCredentials,
    },
    /// GCP Vertex AI credentials.
    GcpVertex {
        project: String,
        region: String,
        credentials: GcpCredentials,
    },
    /// OpenAI-compatible provider.
    OpenAiCompatible { key: String, base_url: String },
}

/// AWS temporary credentials obtained via STS or credential export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// GCP access token obtained via gcloud CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpCredentials {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Errors from provider authentication.
#[derive(Debug, thiserror::Error)]
pub enum ProviderAuthError {
    #[error("no authentication configured")]
    NoAuth,

    #[error("API key helper failed: {0}")]
    ApiKeyHelperFailed(String),

    #[error("AWS credential export failed: {0}")]
    AwsCredentialExportFailed(String),

    #[error("AWS STS check failed: {0}")]
    AwsStsCheckFailed(String),

    #[error("GCP auth failed: {0}")]
    GcpAuthFailed(String),

    #[error("OAuth token expired and refresh failed: {0}")]
    OAuthRefreshFailed(String),

    #[error("command execution error: {0}")]
    CommandExec(String),

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Configuration for resolving provider authentication.
#[derive(Debug, Clone, Default)]
pub struct ProviderAuthConfig {
    /// `ANTHROPIC_API_KEY` environment variable.
    pub anthropic_api_key: Option<String>,
    /// `ANTHROPIC_AUTH_TOKEN` environment variable.
    pub anthropic_auth_token: Option<String>,
    /// Command to execute to obtain an API key.
    pub api_key_helper: Option<String>,
    /// Stored OAuth tokens.
    pub oauth_tokens: Option<OAuthTokens>,
    /// AWS region for Bedrock.
    pub aws_region: Option<String>,
    /// Command to export AWS credentials (outputs JSON).
    pub aws_credential_export: Option<String>,
    /// GCP project ID for Vertex AI.
    pub gcp_project: Option<String>,
    /// GCP region for Vertex AI.
    pub gcp_region: Option<String>,
    /// OpenAI-compatible API key.
    pub openai_compatible_key: Option<String>,
    /// OpenAI-compatible base URL.
    pub openai_compatible_base_url: Option<String>,
    /// Active provider type.
    pub provider_type: ProviderType,
}

/// The type of provider being used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProviderType {
    #[default]
    FirstParty,
    AwsBedrock,
    GcpVertex,
    OpenAiCompatible,
}

/// Resolve the authentication source based on configuration.
///
/// This function dispatches to the appropriate authentication method
/// based on the provider type and available credentials.
pub async fn resolve_auth(config: &ProviderAuthConfig) -> Result<AuthSource, ProviderAuthError> {
    match config.provider_type {
        ProviderType::FirstParty => resolve_first_party_auth(config).await,
        ProviderType::AwsBedrock => resolve_aws_auth(config).await,
        ProviderType::GcpVertex => resolve_gcp_auth(config).await,
        ProviderType::OpenAiCompatible => resolve_openai_compatible_auth(config),
    }
}

/// Resolve first-party (Anthropic) authentication.
///
/// Priority order:
/// 1. `ANTHROPIC_API_KEY` env var
/// 2. `ANTHROPIC_AUTH_TOKEN` env var
/// 3. apiKeyHelper command
/// 4. OAuth tokens
async fn resolve_first_party_auth(
    config: &ProviderAuthConfig,
) -> Result<AuthSource, ProviderAuthError> {
    // 1. Direct API key
    if let Some(ref key) = config.anthropic_api_key {
        debug!("Using ANTHROPIC_API_KEY from environment");
        return Ok(AuthSource::AnthropicApiKey { key: key.clone() });
    }

    // 2. Auth token
    if let Some(ref token) = config.anthropic_auth_token {
        debug!("Using ANTHROPIC_AUTH_TOKEN from environment");
        return Ok(AuthSource::AnthropicApiKey { key: token.clone() });
    }

    // 3. API key helper
    if let Some(ref helper) = config.api_key_helper {
        debug!("Executing apiKeyHelper command");
        let key = api_key_helper::execute_api_key_helper_cached(helper)
            .await
            .map_err(|error| ProviderAuthError::ApiKeyHelperFailed(error.to_string()))?
            .key;
        return Ok(AuthSource::AnthropicApiKey { key });
    }

    // 4. OAuth tokens
    if let Some(ref tokens) = config.oauth_tokens {
        if tokens.access_token.is_empty() {
            return Err(ProviderAuthError::NoAuth);
        }

        // Check if token needs refresh
        if tokens.is_expired() {
            if let Some(ref _refresh_token) = tokens.refresh_token {
                debug!("OAuth token expired, refresh needed");
                // The caller should handle refresh; we return the expired
                // tokens and let the OAuth client refresh them.
                return Ok(AuthSource::OAuth {
                    tokens: tokens.clone(),
                });
            }
            return Err(ProviderAuthError::OAuthRefreshFailed(
                "token expired and no refresh token available".to_owned(),
            ));
        }

        info!("Using OAuth tokens for authentication");
        return Ok(AuthSource::OAuth {
            tokens: tokens.clone(),
        });
    }

    Err(ProviderAuthError::NoAuth)
}

/// Resolve AWS Bedrock authentication.
async fn resolve_aws_auth(config: &ProviderAuthConfig) -> Result<AuthSource, ProviderAuthError> {
    let region = config
        .aws_region
        .clone()
        .unwrap_or_else(|| "us-east-1".to_owned());

    // Try credential export command first
    if let Some(ref export_cmd) = config.aws_credential_export {
        let creds = execute_aws_credential_export(export_cmd).await?;
        return Ok(AuthSource::AwsBedrock {
            region,
            credentials: creds,
        });
    }

    // Fall back to environment-based credentials
    // (AWS SDK will pick up AWS_ACCESS_KEY_ID, etc.)
    Err(ProviderAuthError::NoAuth)
}

/// Resolve GCP Vertex AI authentication.
async fn resolve_gcp_auth(config: &ProviderAuthConfig) -> Result<AuthSource, ProviderAuthError> {
    let project = config
        .gcp_project
        .clone()
        .ok_or(ProviderAuthError::GcpAuthFailed(
            "GCP project not configured".to_owned(),
        ))?;
    let region = config
        .gcp_region
        .clone()
        .unwrap_or_else(|| "us-central1".to_owned());

    let access_token = execute_gcloud_auth().await?;
    Ok(AuthSource::GcpVertex {
        project,
        region,
        credentials: GcpCredentials {
            access_token,
            expires_at: None,
        },
    })
}

/// Resolve OpenAI-compatible provider authentication.
fn resolve_openai_compatible_auth(
    config: &ProviderAuthConfig,
) -> Result<AuthSource, ProviderAuthError> {
    let key = config
        .openai_compatible_key
        .clone()
        .ok_or(ProviderAuthError::NoAuth)?;
    let base_url = config
        .openai_compatible_base_url
        .clone()
        .ok_or(ProviderAuthError::NoAuth)?;

    Ok(AuthSource::OpenAiCompatible { key, base_url })
}

/// Execute an AWS credential export command and parse JSON output.
async fn execute_aws_credential_export(command: &str) -> Result<AwsCredentials, ProviderAuthError> {
    tracing::warn!(
        "Executing aws_credential_export command from config — ensure this is trusted: {command}"
    );
    let output = if cfg!(windows) {
        tokio::process::Command::new("cmd")
            .args(["/C", command])
            .output()
            .await
    } else {
        tokio::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .await
    }
    .map_err(|e| ProviderAuthError::CommandExec(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProviderAuthError::AwsCredentialExportFailed(
            stderr.trim().to_owned(),
        ));
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AwsCredJson {
        access_key_id: String,
        secret_access_key: String,
        session_token: String,
    }

    let creds: AwsCredJson = serde_json::from_slice(&output.stdout)?;
    Ok(AwsCredentials {
        access_key_id: creds.access_key_id,
        secret_access_key: creds.secret_access_key,
        session_token: creds.session_token,
        expires_at: None,
    })
}

/// Execute `gcloud auth print-access-token` to obtain a GCP access token.
async fn execute_gcloud_auth() -> Result<String, ProviderAuthError> {
    let output = tokio::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await
        .map_err(|e| ProviderAuthError::GcpAuthFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProviderAuthError::GcpAuthFailed(stderr.trim().to_owned()));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if token.is_empty() {
        return Err(ProviderAuthError::GcpAuthFailed(
            "gcloud returned empty token".to_owned(),
        ));
    }

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_compatible_auth_missing_key() {
        let config = ProviderAuthConfig {
            provider_type: ProviderType::OpenAiCompatible,
            openai_compatible_key: None,
            openai_compatible_base_url: Some("https://api.example.com".to_owned()),
            ..Default::default()
        };
        let result = resolve_openai_compatible_auth(&config);
        assert!(result.is_err());
    }

    #[test]
    fn openai_compatible_auth_success() {
        let config = ProviderAuthConfig {
            provider_type: ProviderType::OpenAiCompatible,
            openai_compatible_key: Some("sk-test".to_owned()),
            openai_compatible_base_url: Some("https://api.example.com".to_owned()),
            ..Default::default()
        };
        let result = resolve_openai_compatible_auth(&config);
        assert!(result.is_ok());
        if let AuthSource::OpenAiCompatible { key, base_url } = result.expect("ok") {
            assert_eq!(key, "sk-test");
            assert_eq!(base_url, "https://api.example.com");
        }
    }

    #[tokio::test]
    async fn first_party_env_key() {
        let config = ProviderAuthConfig {
            provider_type: ProviderType::FirstParty,
            anthropic_api_key: Some("sk-ant-test".to_owned()),
            ..Default::default()
        };
        let result = resolve_first_party_auth(&config).await;
        assert!(result.is_ok());
        if let AuthSource::AnthropicApiKey { key } = result.expect("ok") {
            assert_eq!(key, "sk-ant-test");
        }
    }

    #[tokio::test]
    async fn first_party_api_key_helper_uses_shared_shell_helper() {
        api_key_helper::clear_global_api_key_helper_cache();
        let config = ProviderAuthConfig {
            provider_type: ProviderType::FirstParty,
            api_key_helper: Some("echo provider-helper-key".to_owned()),
            ..Default::default()
        };
        let result = resolve_first_party_auth(&config).await;
        assert!(result.is_ok());
        if let AuthSource::AnthropicApiKey { key } = result.expect("ok") {
            assert_eq!(key, "provider-helper-key");
        }
        api_key_helper::clear_global_api_key_helper_cache();
    }
}
