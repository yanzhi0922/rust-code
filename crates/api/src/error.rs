use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Rate limit exceeded")]
    RateLimit { retry_after_ms: Option<u64> },

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Timeout")]
    Timeout,

    #[error("{0}")]
    Other(String),
}
