//! OAuth client — authorization URL building, token exchange, and refresh.
//!
//! Mirrors `services/oauth/client.ts`.

use std::sync::Arc;

use once_cell::sync::Lazy;
use serde_json::json;
use tracing::{debug, info};

use super::auth_code_listener::{AuthCodeListener, CallbackResult};
use super::pkce;
use super::types::{
    OAuthConfig, OAuthFlowResult, OAuthProfileResponse, OAuthTokenExchangeResponse, OAuthTokens,
};

static SHARED_HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(reqwest::Client::new);

/// Errors produced by the OAuth client.
#[derive(Debug, thiserror::Error)]
pub enum OAuthClientError {
    #[error("token exchange failed (HTTP {status}): {message}")]
    ExchangeFailed { status: u16, message: String },

    #[error("token refresh failed: {0}")]
    RefreshFailed(String),

    #[error("profile fetch failed: {0}")]
    ProfileFetchFailed(String),

    #[error("auth code listener error: {0}")]
    Listener(#[from] super::auth_code_listener::AuthCodeListenerError),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("OAuth flow cancelled")]
    Cancelled,
}

fn hydrate_tokens_from_profile(
    mut tokens: OAuthTokens,
    profile: Option<&OAuthProfileResponse>,
) -> OAuthTokens {
    let Some(profile) = profile else {
        return tokens;
    };
    if let Some(organization) = profile.organization.as_ref() {
        if tokens.subscription_type.is_none() {
            tokens.subscription_type = organization
                .organization_type
                .as_deref()
                .and_then(subscription_type_from_org_type);
        }
        if tokens.rate_limit_tier.is_none() {
            tokens.rate_limit_tier = organization.rate_limit_tier.clone();
        }
        if tokens.billing_type.is_none() {
            tokens.billing_type = organization.billing_type.clone();
        }
        if tokens.has_extra_usage_enabled.is_none() {
            tokens.has_extra_usage_enabled = organization.has_extra_usage_enabled;
        }
    }
    tokens
}

pub async fn refresh_oauth_token_with_existing(
    config: &OAuthConfig,
    existing_tokens: &OAuthTokens,
    scopes: Option<&[String]>,
) -> Result<OAuthTokens, OAuthClientError> {
    let Some(refresh_token) = existing_tokens.refresh_token.as_deref() else {
        return Err(OAuthClientError::RefreshFailed(
            "missing refresh token".to_owned(),
        ));
    };

    let mut refreshed = refresh_oauth_token(config, refresh_token, scopes).await?;
    if refreshed.subscription_type.is_none() {
        refreshed.subscription_type = existing_tokens.subscription_type.clone();
    }
    if refreshed.rate_limit_tier.is_none() {
        refreshed.rate_limit_tier = existing_tokens.rate_limit_tier.clone();
    }
    if refreshed.billing_type.is_none() {
        refreshed.billing_type = existing_tokens.billing_type.clone();
    }
    if refreshed.has_extra_usage_enabled.is_none() {
        refreshed.has_extra_usage_enabled = existing_tokens.has_extra_usage_enabled;
    }
    Ok(refreshed)
}

/// Build the authorization URL for the OAuth flow.
///
/// Constructs the URL the user's browser should navigate to, including
/// PKCE `code_challenge`, `state`, scopes, and optional parameters.
pub fn build_auth_url(params: &super::types::BuildAuthUrlParams, config: &OAuthConfig) -> String {
    let base_url = if params.login_with_claude_ai {
        &config.authorize_url
    } else {
        &config.console_authorize_url
    };

    let redirect_uri_owned;
    let redirect_uri: &str = if params.is_manual {
        &config.manual_redirect_url
    } else {
        redirect_uri_owned = format!("http://localhost:{}/callback", params.port);
        &redirect_uri_owned
    };

    let scopes = if params.inference_only {
        vec!["org:inference"]
    } else {
        vec![
            "org:inference",
            "user:profile",
            "user:api_keys",
            "org:api_keys",
        ]
    };

    let mut url = reqwest::Url::parse(base_url).unwrap_or_else(|e| {
        tracing::error!("Invalid OAuth base URL '{base_url}': {e}");
        reqwest::Url::parse("https://invalid-url.local").expect("fallback URL should parse")
    });
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("code", "true");
        qp.append_pair("client_id", &config.client_id);
        qp.append_pair("response_type", "code");
        qp.append_pair("redirect_uri", redirect_uri);
        qp.append_pair("scope", &scopes.join(" "));
        qp.append_pair("code_challenge", &params.code_challenge);
        qp.append_pair("code_challenge_method", "S256");
        qp.append_pair("state", &params.state);

        if let Some(ref org_uuid) = params.org_uuid {
            qp.append_pair("orgUUID", org_uuid);
        }
        if let Some(ref hint) = params.login_hint {
            qp.append_pair("login_hint", hint);
        }
        if let Some(ref method) = params.login_method {
            qp.append_pair("login_method", method);
        }
    }

    url.to_string()
}

/// Exchange an authorization code for OAuth tokens.
pub async fn exchange_code_for_tokens(
    config: &OAuthConfig,
    authorization_code: &str,
    state: &str,
    code_verifier: &str,
    port: u16,
    is_manual: bool,
    expires_in: Option<u64>,
) -> Result<OAuthTokenExchangeResponse, OAuthClientError> {
    let redirect_uri = if is_manual {
        config.manual_redirect_url.clone()
    } else {
        format!("http://localhost:{port}/callback")
    };

    let mut body = json!({
        "grant_type": "authorization_code",
        "code": authorization_code,
        "redirect_uri": redirect_uri,
        "client_id": config.client_id,
        "code_verifier": code_verifier,
        "state": state,
    });

    if let Some(expires_in) = expires_in {
        body["expires_in"] = json!(expires_in);
    }

    let response = SHARED_HTTP_CLIENT
        .post(&config.token_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    let status = response.status().as_u16();
    if status != 200 {
        let text = response.text().await.unwrap_or_else(|e| {
            tracing::warn!("Failed to read OAuth error response body: {e}");
            format!("<unreadable response body: {e}>")
        });
        return Err(OAuthClientError::ExchangeFailed {
            status,
            message: text,
        });
    }

    let token_response: OAuthTokenExchangeResponse = response.json().await?;
    info!("OAuth token exchange succeeded");
    Ok(token_response)
}

/// Refresh an OAuth token using a refresh token.
pub async fn refresh_oauth_token(
    config: &OAuthConfig,
    refresh_token: &str,
    scopes: Option<&[String]>,
) -> Result<OAuthTokens, OAuthClientError> {
    let default_scopes = [
        "org:inference".to_owned(),
        "user:profile".to_owned(),
        "user:api_keys".to_owned(),
        "org:api_keys".to_owned(),
    ];
    let scope_str = scopes
        .map(|s| s.join(" "))
        .unwrap_or_else(|| default_scopes.join(" "));

    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": config.client_id,
        "scope": scope_str,
    });

    let response = SHARED_HTTP_CLIENT
        .post(&config.token_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if response.status().as_u16() != 200 {
        let text = response.text().await.unwrap_or_else(|e| {
            tracing::warn!("Failed to read OAuth refresh error body: {e}");
            format!("<unreadable response body: {e}>")
        });
        return Err(OAuthClientError::RefreshFailed(text));
    }

    let data: OAuthTokenExchangeResponse = response.json().await?;
    let expires_at =
        chrono::Utc::now().timestamp_millis() + (data.expires_in as i64).saturating_mul(1000);

    info!("OAuth token refresh succeeded");
    Ok(OAuthTokens {
        access_token: data.access_token.clone(),
        refresh_token: data
            .refresh_token
            .clone()
            .or_else(|| Some(refresh_token.to_owned())),
        expires_at: Some(expires_at),
        scope: data.scope.clone(),
        subscription_type: None,
        rate_limit_tier: None,
        billing_type: None,
        has_extra_usage_enabled: None,
    })
}

/// Fetch the user's OAuth profile.
pub async fn fetch_profile(
    config: &OAuthConfig,
    access_token: &str,
) -> Result<OAuthProfileResponse, OAuthClientError> {
    let response = SHARED_HTTP_CLIENT
        .get(&config.profile_url)
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?;

    if response.status().as_u16() != 200 {
        let text = response.text().await.unwrap_or_else(|e| {
            tracing::warn!("Failed to read OAuth profile error body: {e}");
            format!("<unreadable response body: {e}>")
        });
        return Err(OAuthClientError::ProfileFetchFailed(text));
    }

    let profile: OAuthProfileResponse = response.json().await?;
    debug!("OAuth profile fetched successfully");
    Ok(profile)
}

/// Determine the subscription type from an organization type string.
pub fn subscription_type_from_org_type(org_type: &str) -> Option<String> {
    match org_type {
        "claude_max" => Some("max".to_owned()),
        "claude_pro" => Some("pro".to_owned()),
        "claude_enterprise" => Some("enterprise".to_owned()),
        "claude_team" => Some("team".to_owned()),
        _ => None,
    }
}

/// Run the full OAuth PKCE flow:
/// 1. Generate PKCE values
/// 2. Start a local callback listener
/// 3. Build the auth URL and invoke `on_auth_url`
/// 4. Wait for the callback
/// 5. Exchange the code for tokens
pub async fn run_oauth_flow(
    config: Arc<OAuthConfig>,
    on_auth_url: Box<dyn Fn(String) + Send>,
    login_with_claude_ai: bool,
    inference_only: bool,
) -> Result<OAuthFlowResult, OAuthClientError> {
    // 1. Generate PKCE values
    let code_verifier = pkce::generate_code_verifier();
    let code_challenge = pkce::generate_code_challenge(&code_verifier);
    let state = pkce::generate_state();

    // 2. Start callback listener
    let listener = AuthCodeListener::start(config.clone()).await?;
    let port = listener.port();

    // 3. Build auth URL
    let params = super::types::BuildAuthUrlParams {
        code_challenge,
        state: state.clone(),
        port,
        is_manual: false,
        login_with_claude_ai,
        inference_only,
        org_uuid: None,
        login_hint: None,
        login_method: None,
    };
    let auth_url = build_auth_url(&params, &config);

    // 4. Wait for callback
    let state_clone = state.clone();
    let on_ready = Box::new(move || {
        on_auth_url(auth_url);
    });

    let CallbackResult {
        authorization_code,
        state: received_state,
        is_automatic: _,
    } = listener
        .wait_for_authorization(&state_clone, on_ready)
        .await?;

    debug!(?received_state, "Received authorization code");

    // 5. Exchange code for tokens
    let token_response = exchange_code_for_tokens(
        &config,
        &authorization_code,
        &received_state,
        &code_verifier,
        port,
        false,
        None,
    )
    .await?;

    let expires_at = chrono::Utc::now().timestamp_millis()
        + (token_response.expires_in as i64).saturating_mul(1000);

    let tokens = OAuthTokens {
        access_token: token_response.access_token.clone(),
        refresh_token: token_response.refresh_token.clone(),
        expires_at: Some(expires_at),
        scope: token_response.scope.clone(),
        subscription_type: None,
        rate_limit_tier: None,
        billing_type: None,
        has_extra_usage_enabled: None,
    };

    // 6. Optionally fetch profile
    let profile = match fetch_profile(&config, &tokens.access_token).await {
        Ok(p) => Some(p),
        Err(e) => {
            debug!("Profile fetch skipped: {e}");
            None
        }
    };

    let token_account = token_response
        .account
        .map(|a| super::types::TokenAccountInfo {
            uuid: a.uuid,
            email_address: a.email_address,
            organization_uuid: token_response.organization.map(|o| o.uuid),
        });

    let tokens = hydrate_tokens_from_profile(tokens, profile.as_ref());

    Ok(OAuthFlowResult {
        tokens,
        profile,
        token_account,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::types::{OAuthProfileAccount, OAuthProfileOrganization};

    #[test]
    fn hydrate_tokens_from_profile_populates_subscription_fields() {
        let tokens = OAuthTokens {
            access_token: "token".to_owned(),
            refresh_token: Some("refresh".to_owned()),
            expires_at: Some(1),
            scope: None,
            subscription_type: None,
            rate_limit_tier: None,
            billing_type: None,
            has_extra_usage_enabled: None,
        };
        let profile = OAuthProfileResponse {
            account: Some(OAuthProfileAccount {
                uuid: "acct".to_owned(),
                email: "user@example.com".to_owned(),
                display_name: Some("User".to_owned()),
                created_at: Some("2025-01-01T00:00:00Z".to_owned()),
            }),
            organization: Some(OAuthProfileOrganization {
                uuid: "org".to_owned(),
                organization_type: Some("claude_pro".to_owned()),
                rate_limit_tier: Some("elevated".to_owned()),
                has_extra_usage_enabled: Some(true),
                billing_type: Some("stripe_subscription".to_owned()),
                subscription_created_at: Some("2025-01-02T00:00:00Z".to_owned()),
            }),
        };

        let hydrated = hydrate_tokens_from_profile(tokens, Some(&profile));
        assert_eq!(hydrated.subscription_type.as_deref(), Some("pro"));
        assert_eq!(hydrated.rate_limit_tier.as_deref(), Some("elevated"));
        assert_eq!(
            hydrated.billing_type.as_deref(),
            Some("stripe_subscription")
        );
        assert_eq!(hydrated.has_extra_usage_enabled, Some(true));
    }

    #[test]
    fn refresh_identity_can_be_preserved_locally() {
        let refreshed = OAuthTokens {
            access_token: "fresh".to_owned(),
            refresh_token: Some("refresh-2".to_owned()),
            expires_at: Some(2),
            scope: None,
            subscription_type: None,
            rate_limit_tier: None,
            billing_type: None,
            has_extra_usage_enabled: None,
        };
        let existing = OAuthTokens {
            access_token: "old".to_owned(),
            refresh_token: Some("refresh".to_owned()),
            expires_at: Some(1),
            scope: None,
            subscription_type: Some("team".to_owned()),
            rate_limit_tier: Some("high".to_owned()),
            billing_type: Some("stripe_subscription_contracted".to_owned()),
            has_extra_usage_enabled: Some(false),
        };

        let merged = OAuthTokens {
            subscription_type: refreshed
                .subscription_type
                .or_else(|| existing.subscription_type.clone()),
            rate_limit_tier: refreshed
                .rate_limit_tier
                .or_else(|| existing.rate_limit_tier.clone()),
            billing_type: refreshed
                .billing_type
                .or_else(|| existing.billing_type.clone()),
            has_extra_usage_enabled: refreshed
                .has_extra_usage_enabled
                .or(existing.has_extra_usage_enabled),
            ..refreshed
        };

        assert_eq!(merged.access_token, "fresh");
        assert_eq!(merged.subscription_type.as_deref(), Some("team"));
        assert_eq!(merged.rate_limit_tier.as_deref(), Some("high"));
        assert_eq!(
            merged.billing_type.as_deref(),
            Some("stripe_subscription_contracted")
        );
        assert_eq!(merged.has_extra_usage_enabled, Some(false));
    }
}
