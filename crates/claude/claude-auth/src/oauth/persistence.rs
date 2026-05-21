use std::fs;
use std::path::{Path, PathBuf};

use claude_context::{RuntimeIdentityContext, RuntimeSubscriptionContext, RuntimeUserType};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::types::OAuthTokens;

pub const PROFILE_CREDENTIALS_FILE: &str = ".credentials.json";
pub const PROFILE_CONFIG_FILE: &str = ".config.json";

#[derive(Debug, thiserror::Error)]
pub enum OAuthPersistenceError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct PersistedOAuthState {
    pub tokens: Option<OAuthTokens>,
    pub account: Option<PersistedOAuthAccount>,
}

impl PersistedOAuthState {
    pub fn from_profile_dir(profile_dir: &Path) -> Result<Self, OAuthPersistenceError> {
        let credentials_path = profile_dir.join(PROFILE_CREDENTIALS_FILE);
        let config_path = profile_dir.join(PROFILE_CONFIG_FILE);
        let credentials = read_json_file::<PersistedCredentialsFile>(&credentials_path)?;
        let global_config = read_json_file::<PersistedGlobalConfig>(&config_path)?;

        Ok(Self {
            tokens: credentials
                .and_then(|file| file.claude_ai_oauth)
                .map(PersistedClaudeAiOauth::into_oauth_tokens),
            account: global_config.and_then(|config| config.oauth_account),
        })
    }

    #[must_use]
    pub fn has_tokens(&self) -> bool {
        self.tokens.is_some()
    }

    #[must_use]
    pub fn runtime_identity_fragment(&self) -> RuntimeIdentityContext {
        let tokens = self.tokens.as_ref();
        let account = self.account.as_ref();

        RuntimeIdentityContext {
            user_type: if tokens.is_some() {
                RuntimeUserType::External
            } else {
                RuntimeUserType::Unknown
            },
            organization_uuid: account.and_then(|value| value.organization_uuid.clone()),
            account_uuid: account.map(|value| value.account_uuid.clone()),
            email: account.map(|value| value.email_address.clone()),
            subscription: RuntimeSubscriptionContext {
                subscription_type: tokens.and_then(|value| value.subscription_type.clone()),
                rate_limit_tier: tokens.and_then(|value| value.rate_limit_tier.clone()),
                billing_type: account
                    .and_then(|value| value.billing_type.clone())
                    .or_else(|| tokens.and_then(|value| value.billing_type.clone())),
                has_extra_usage_enabled: account
                    .and_then(|value| value.has_extra_usage_enabled)
                    .or_else(|| tokens.and_then(|value| value.has_extra_usage_enabled)),
                display_name: account.and_then(|value| value.display_name.clone()),
                account_created_at: account.and_then(|value| value.account_created_at.clone()),
                subscription_created_at: account
                    .and_then(|value| value.subscription_created_at.clone()),
            },
            ..RuntimeIdentityContext::default()
        }
    }
}

pub fn load_persisted_oauth_state(
    profile_dir: &Path,
) -> Result<PersistedOAuthState, OAuthPersistenceError> {
    PersistedOAuthState::from_profile_dir(profile_dir)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedOAuthAccount {
    pub account_uuid: String,
    pub email_address: String,
    #[serde(default)]
    pub organization_uuid: Option<String>,
    #[serde(default)]
    pub organization_name: Option<String>,
    #[serde(default)]
    pub organization_role: Option<String>,
    #[serde(default)]
    pub workspace_role: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub has_extra_usage_enabled: Option<bool>,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default)]
    pub account_created_at: Option<String>,
    #[serde(default)]
    pub subscription_created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PersistedCredentialsFile {
    #[serde(default, rename = "claudeAiOauth", alias = "claude_ai_oauth")]
    claude_ai_oauth: Option<PersistedClaudeAiOauth>,
}

#[derive(Debug, Clone, Deserialize)]
struct PersistedGlobalConfig {
    #[serde(default, rename = "oauthAccount", alias = "oauth_account")]
    oauth_account: Option<PersistedOAuthAccount>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedClaudeAiOauth {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    scopes: Option<PersistedScopes>,
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    rate_limit_tier: Option<String>,
}

impl PersistedClaudeAiOauth {
    fn into_oauth_tokens(self) -> OAuthTokens {
        OAuthTokens {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at: self.expires_at,
            scope: normalize_scopes(self.scopes),
            subscription_type: self.subscription_type,
            rate_limit_tier: self.rate_limit_tier,
            billing_type: None,
            has_extra_usage_enabled: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PersistedScopes {
    List(Vec<String>),
    String(String),
}

fn normalize_scopes(scopes: Option<PersistedScopes>) -> Option<String> {
    match scopes {
        Some(PersistedScopes::List(scopes)) => {
            let filtered = scopes
                .into_iter()
                .map(|scope| scope.trim().to_owned())
                .filter(|scope| !scope.is_empty())
                .collect::<Vec<_>>();
            (!filtered.is_empty()).then(|| filtered.join(" "))
        }
        Some(PersistedScopes::String(scopes)) => {
            let trimmed = scopes.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        None => None,
    }
}

fn read_json_file<T>(path: &Path) -> Result<Option<T>, OAuthPersistenceError>
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path).map_err(|source| OAuthPersistenceError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = serde_json::from_str(&raw).map_err(|source| OAuthPersistenceError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::{PROFILE_CONFIG_FILE, PROFILE_CREDENTIALS_FILE, PersistedOAuthState};
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn loads_research_style_credentials_and_global_config() {
        let tempdir = tempdir().expect("tempdir");
        let profile_dir = tempdir.path();
        fs::write(
            profile_dir.join(PROFILE_CREDENTIALS_FILE),
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "token-1",
                    "refreshToken": "refresh-1",
                    "expiresAt": 1234,
                    "scopes": ["user:profile", "user:inference"],
                    "subscriptionType": "team",
                    "rateLimitTier": "high"
                }
            })
            .to_string(),
        )
        .expect("write credentials");
        fs::write(
            profile_dir.join(PROFILE_CONFIG_FILE),
            serde_json::json!({
                "oauthAccount": {
                    "accountUuid": "acct-1",
                    "emailAddress": "dev@example.com",
                    "organizationUuid": "org-1",
                    "displayName": "Dev User",
                    "hasExtraUsageEnabled": false,
                    "billingType": "stripe_subscription_contracted",
                    "accountCreatedAt": "2025-01-01T00:00:00Z",
                    "subscriptionCreatedAt": "2025-02-01T00:00:00Z"
                }
            })
            .to_string(),
        )
        .expect("write config");

        let state = PersistedOAuthState::from_profile_dir(profile_dir).expect("load state");
        let identity = state.runtime_identity_fragment();

        assert!(state.has_tokens());
        assert_eq!(
            state.tokens.as_ref().map(|value| value.scope.as_deref()),
            Some(Some("user:profile user:inference"))
        );
        assert_eq!(identity.account_uuid.as_deref(), Some("acct-1"));
        assert_eq!(identity.organization_uuid.as_deref(), Some("org-1"));
        assert_eq!(identity.email.as_deref(), Some("dev@example.com"));
        assert_eq!(
            identity.subscription.subscription_type.as_deref(),
            Some("team")
        );
        assert_eq!(
            identity.subscription.rate_limit_tier.as_deref(),
            Some("high")
        );
        assert_eq!(
            identity.subscription.billing_type.as_deref(),
            Some("stripe_subscription_contracted")
        );
        assert_eq!(identity.subscription.has_extra_usage_enabled, Some(false));
        assert_eq!(
            identity.subscription.display_name.as_deref(),
            Some("Dev User")
        );
    }

    #[test]
    fn missing_profile_files_yield_empty_state() {
        let tempdir = tempdir().expect("tempdir");
        let state = PersistedOAuthState::from_profile_dir(tempdir.path()).expect("load state");
        assert!(!state.has_tokens());
        assert!(state.account.is_none());
    }
}
