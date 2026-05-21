//! OAuth 2.0 module with PKCE support.
//!
//! Implements the full OAuth authorization-code flow:
//! - PKCE `code_verifier` / `code_challenge` generation
//! - Local HTTP callback listener
//! - Token exchange and refresh
//! - Profile fetching
//!
//! Mirrors `services/oauth/` from the TypeScript reference.

pub mod auth_code_listener;
pub mod client;
pub mod persistence;
pub mod pkce;
pub mod types;

pub use auth_code_listener::{AuthCodeListener, AuthCodeListenerError, CallbackResult};
pub use client::{
    OAuthClientError, build_auth_url, exchange_code_for_tokens, fetch_profile, refresh_oauth_token,
    refresh_oauth_token_with_existing, run_oauth_flow, subscription_type_from_org_type,
};
pub use persistence::{
    OAuthPersistenceError, PROFILE_CONFIG_FILE, PROFILE_CREDENTIALS_FILE, PersistedOAuthAccount,
    PersistedOAuthState, load_persisted_oauth_state,
};
pub use pkce::{generate_code_challenge, generate_code_verifier, generate_state};
pub use types::{
    BuildAuthUrlParams, OAuthConfig, OAuthFlowResult, OAuthProfileAccount,
    OAuthProfileOrganization, OAuthProfileResponse, OAuthTokenExchangeResponse, OAuthTokens,
    TokenAccount, TokenAccountInfo, TokenOrganization,
};
