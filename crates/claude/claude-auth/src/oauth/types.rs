//! OAuth type definitions.
//!
//! Mirrors the TypeScript types from `services/oauth/types.ts` and the
//! token-exchange response shape used by the Anthropic OAuth endpoints.

use chrono::{DateTime, Utc};
use claude_context::{RuntimeIdentityContext, RuntimeSubscriptionContext, RuntimeUserType};
use serde::{Deserialize, Serialize};

/// OAuth token set returned by the token endpoint or stored locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Bearer token for API calls.
    pub access_token: String,
    /// Long-lived token used to obtain fresh access tokens.
    pub refresh_token: Option<String>,
    /// Unix-millis epoch at which the access token expires.
    pub expires_at: Option<i64>,
    /// Space-separated OAuth scopes granted.
    pub scope: Option<String>,
    /// Subscription tier resolved from the profile endpoint.
    pub subscription_type: Option<String>,
    /// Rate-limit tier from the profile endpoint.
    pub rate_limit_tier: Option<String>,
    /// Billing type from the profile endpoint.
    pub billing_type: Option<String>,
    /// Whether extra usage is enabled.
    pub has_extra_usage_enabled: Option<bool>,
}

impl OAuthTokens {
    /// Returns `true` when the access token is within 5 minutes of expiry
    /// (or already expired).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let buffer_ms = 5 * 60 * 1000;
                chrono::Utc::now().timestamp_millis() + buffer_ms >= expires_at
            }
            None => false,
        }
    }

    /// Parse the scope string into individual scope tokens.
    ///
    /// Returns an empty vector when no scope string is available.
    pub fn scopes(&self) -> Vec<String> {
        self.scope
            .as_deref()
            .map(|s| s.split(' ').map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

/// Response body from the OAuth token exchange endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthTokenExchangeResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub account: Option<TokenAccount>,
    #[serde(default)]
    pub organization: Option<TokenOrganization>,
}

/// Account info embedded in a token-exchange response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenAccount {
    pub uuid: String,
    pub email_address: String,
}

/// Organization info embedded in a token-exchange response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenOrganization {
    pub uuid: String,
}

/// OAuth profile response from `/api/oauth/profile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthProfileResponse {
    #[serde(default)]
    pub account: Option<OAuthProfileAccount>,
    #[serde(default)]
    pub organization: Option<OAuthProfileOrganization>,
}

/// Account section of the profile response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthProfileAccount {
    pub uuid: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Organization section of the profile response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthProfileOrganization {
    pub uuid: String,
    #[serde(default)]
    pub organization_type: Option<String>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    #[serde(default)]
    pub has_extra_usage_enabled: Option<bool>,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default)]
    pub subscription_created_at: Option<String>,
}

/// Configuration for an OAuth client (endpoints, client ID, etc.).
#[derive(Debug, Clone, Default)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub console_authorize_url: String,
    pub token_url: String,
    pub manual_redirect_url: String,
    pub claudeai_success_url: String,
    pub console_success_url: String,
    pub profile_url: String,
    pub api_key_url: String,
    pub roles_url: String,
}

/// Parameters for building the authorization URL.
#[derive(Debug, Clone)]
pub struct BuildAuthUrlParams {
    pub code_challenge: String,
    pub state: String,
    pub port: u16,
    pub is_manual: bool,
    pub login_with_claude_ai: bool,
    pub inference_only: bool,
    pub org_uuid: Option<String>,
    pub login_hint: Option<String>,
    pub login_method: Option<String>,
}

/// Result of a completed OAuth flow.
#[derive(Debug, Clone)]
pub struct OAuthFlowResult {
    pub tokens: OAuthTokens,
    pub profile: Option<OAuthProfileResponse>,
    pub token_account: Option<TokenAccountInfo>,
}

impl OAuthFlowResult {
    #[must_use]
    pub fn runtime_identity_context(&self) -> RuntimeIdentityContext {
        let account = self
            .profile
            .as_ref()
            .and_then(|profile| profile.account.as_ref());
        let organization = self
            .profile
            .as_ref()
            .and_then(|profile| profile.organization.as_ref());

        RuntimeIdentityContext {
            user_type: RuntimeUserType::External,
            account_uuid: self
                .token_account
                .as_ref()
                .map(|account| account.uuid.clone())
                .or_else(|| account.map(|account| account.uuid.clone())),
            organization_uuid: self
                .token_account
                .as_ref()
                .and_then(|account| account.organization_uuid.clone())
                .or_else(|| organization.map(|organization| organization.uuid.clone())),
            email: self
                .token_account
                .as_ref()
                .map(|account| account.email_address.clone())
                .or_else(|| account.map(|account| account.email.clone())),
            subscription: RuntimeSubscriptionContext {
                subscription_type: self.tokens.subscription_type.clone(),
                rate_limit_tier: self.tokens.rate_limit_tier.clone(),
                billing_type: self.tokens.billing_type.clone(),
                has_extra_usage_enabled: self.tokens.has_extra_usage_enabled,
                display_name: account.and_then(|account| account.display_name.clone()),
                account_created_at: account.and_then(|account| account.created_at.clone()),
                subscription_created_at: organization
                    .and_then(|organization| organization.subscription_created_at.clone()),
            },
            ..RuntimeIdentityContext::default()
        }
    }
}

/// Token-derived account info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAccountInfo {
    pub uuid: String,
    pub email_address: String,
    pub organization_uuid: Option<String>,
}

/// Timestamp helper — convert `DateTime<Utc>` to epoch millis.
pub fn datetime_to_epoch_millis(dt: &DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}
