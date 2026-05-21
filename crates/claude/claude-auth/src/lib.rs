//! # rc-auth — Authentication system for Remote Code Rust.
//!
//! This crate provides the full authentication stack:
//!
//! - **OAuth 2.0 PKCE flow** — authorization URL building, code exchange, token refresh
//! - **API Key Helper** — external command execution with 5-minute TTL cache (SWR pattern)
//! - **Provider auth dispatch** — Anthropic API Key / OAuth / AWS Bedrock / GCP Vertex / OpenAI Compatible
//! - **Secure storage** — cross-platform keychain / credential manager / secret service
//! - **Subscription tiers** — Free / Pro / Team / Enterprise / Max
//!
//! ## Quick start
//!
//! ```no_run
//! use claude_auth::provider_auth::{ProviderAuthConfig, ProviderType, resolve_auth};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = ProviderAuthConfig {
//!     anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
//!     provider_type: ProviderType::FirstParty,
//!     ..Default::default()
//! };
//! let auth_source = resolve_auth(&config).await?;
//! # Ok(())
//! # }
//! ```

pub mod api_key_helper;
pub mod oauth;
pub mod provider_auth;
pub mod secure_storage;
pub mod subscription;

// Re-export the most commonly used types at the crate root.

// OAuth
pub use oauth::{
    AuthCodeListener, AuthCodeListenerError, CallbackResult, OAuthClientError, OAuthConfig,
    OAuthFlowResult, OAuthPersistenceError, OAuthTokens, PROFILE_CONFIG_FILE,
    PROFILE_CREDENTIALS_FILE, PersistedOAuthAccount, PersistedOAuthState, build_auth_url,
    exchange_code_for_tokens, fetch_profile, generate_code_challenge, generate_code_verifier,
    generate_state, load_persisted_oauth_state, refresh_oauth_token,
    refresh_oauth_token_with_existing, run_oauth_flow,
};

// Provider auth
pub use provider_auth::{
    AuthSource, AwsCredentials, GcpCredentials, ProviderAuthConfig, ProviderAuthError,
    ProviderType, resolve_auth,
};

// API key helper
pub use api_key_helper::{
    ApiKeyHelperCache, ApiKeyHelperError, ApiKeyHelperResult, ApiKeySource,
    DEFAULT_API_KEY_HELPER_TTL, clear_global_api_key_helper_cache, execute_api_key_helper,
    execute_api_key_helper_cached,
};

// Secure storage
pub use secure_storage::{
    MockSecureStorage, SecureStorage, SecureStorageError, platform_secure_storage,
};

// Subscription
pub use subscription::{BillingType, RateLimitTier, SubscriptionInfo, SubscriptionTier};
