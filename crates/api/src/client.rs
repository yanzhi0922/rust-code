use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".to_string(),
            api_key: None,
            timeout_secs: 120,
        }
    }
}

pub trait ProviderClient: Send + Sync {
    fn create_message(
        &self,
        messages: Vec<claude_runtime::session::ConversationMessage>,
        system: String,
        config: claude_runtime::conversation::QueryConfig,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<crate::types::MessageResponse, crate::error::ApiError>>
                + Send,
        >,
    >;

    fn stream_message(
        &self,
        messages: Vec<claude_runtime::session::ConversationMessage>,
        system: String,
        config: claude_runtime::conversation::QueryConfig,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<
                            Box<
                                dyn futures::Stream<
                                        Item = Result<
                                            claude_runtime::conversation::AssistantEvent,
                                            crate::error::ApiError,
                                        >,
                                    > + Send,
                            >,
                        >,
                        crate::error::ApiError,
                    >,
                > + Send,
        >,
    >;
}

use std::future::Future;
use std::pin::Pin;
