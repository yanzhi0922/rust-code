use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode};
use serde::Serialize;

#[derive(Debug, Clone)]
pub(crate) struct ProbeSpec {
    pub label: String,
    pub url: String,
    pub method: Method,
    pub headers: Vec<(String, String)>,
}

impl ProbeSpec {
    pub(crate) fn new(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
            method: Method::GET,
            headers: Vec::new(),
        }
    }

    pub(crate) fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProbeOutcomeKind {
    Reachable,
    AuthRejected,
    RateLimited,
    ServerError,
    TransportError,
}

impl ProbeOutcomeKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::AuthRejected => "auth_rejected",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::TransportError => "transport_error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProbeResult {
    pub label: String,
    pub url: String,
    pub method: String,
    pub outcome: ProbeOutcomeKind,
    pub status_code: Option<u16>,
    pub latency_ms: u128,
    pub detail: String,
}

impl ProbeResult {
    pub(crate) fn is_issue(&self) -> bool {
        matches!(
            self.outcome,
            ProbeOutcomeKind::AuthRejected
                | ProbeOutcomeKind::ServerError
                | ProbeOutcomeKind::TransportError
        )
    }

    pub(crate) fn is_warning(&self) -> bool {
        matches!(self.outcome, ProbeOutcomeKind::RateLimited)
    }
}

pub(crate) async fn run_probe(spec: ProbeSpec) -> ProbeResult {
    let client = match reqwest::Client::builder()
        .user_agent("remote-code-rust-doctor")
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ProbeResult {
                label: spec.label,
                url: spec.url,
                method: spec.method.as_str().to_owned(),
                outcome: ProbeOutcomeKind::TransportError,
                status_code: None,
                latency_ms: 0,
                detail: format!("failed to build HTTP client: {error}"),
            };
        }
    };

    let start = Instant::now();
    let mut request = client.request(spec.method.clone(), &spec.url);
    for (name, value) in &spec.headers {
        request = request.header(name, value);
    }

    match request.send().await {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis();
            let status = response.status();
            let (outcome, detail) = classify_http_status(status);
            ProbeResult {
                label: spec.label,
                url: spec.url,
                method: spec.method.as_str().to_owned(),
                outcome,
                status_code: Some(status.as_u16()),
                latency_ms,
                detail,
            }
        }
        Err(error) => ProbeResult {
            label: spec.label,
            url: spec.url,
            method: spec.method.as_str().to_owned(),
            outcome: ProbeOutcomeKind::TransportError,
            status_code: None,
            latency_ms: start.elapsed().as_millis(),
            detail: error.to_string(),
        },
    }
}

fn classify_http_status(status: StatusCode) -> (ProbeOutcomeKind, String) {
    let code = status.as_u16();
    if status.is_success() {
        return (
            ProbeOutcomeKind::Reachable,
            format!("HTTP {code} confirms the endpoint is reachable"),
        );
    }

    match code {
        400 | 404 | 405 | 406 | 409 | 415 | 422 => (
            ProbeOutcomeKind::Reachable,
            format!("HTTP {code} confirms the endpoint is reachable"),
        ),
        401 | 403 => (
            ProbeOutcomeKind::AuthRejected,
            format!("HTTP {code} indicates the endpoint rejected the supplied credentials"),
        ),
        429 => (
            ProbeOutcomeKind::RateLimited,
            "HTTP 429 indicates the endpoint is reachable but currently rate limited".to_owned(),
        ),
        500..=599 => (
            ProbeOutcomeKind::ServerError,
            format!("HTTP {code} indicates an upstream server failure"),
        ),
        _ => (
            ProbeOutcomeKind::Reachable,
            format!("HTTP {code} returned from the endpoint"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{ProbeOutcomeKind, classify_http_status};
    use reqwest::StatusCode;

    #[test]
    fn provider_probe_classification_distinguishes_auth_and_transport_classes() {
        assert_eq!(
            classify_http_status(StatusCode::METHOD_NOT_ALLOWED).0,
            ProbeOutcomeKind::Reachable
        );
        assert_eq!(
            classify_http_status(StatusCode::UNAUTHORIZED).0,
            ProbeOutcomeKind::AuthRejected
        );
        assert_eq!(
            classify_http_status(StatusCode::TOO_MANY_REQUESTS).0,
            ProbeOutcomeKind::RateLimited
        );
        assert_eq!(
            classify_http_status(StatusCode::BAD_GATEWAY).0,
            ProbeOutcomeKind::ServerError
        );
    }
}
