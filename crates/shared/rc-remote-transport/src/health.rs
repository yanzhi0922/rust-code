//! Health probe engine for endpoint reachability checking.

use std::time::{Duration, Instant};

use crate::EndpointHealth;

/// Probe a single HTTP endpoint for reachability and latency.
pub async fn probe_endpoint(
    url: &str,
    auth_token: Option<&str>,
    timeout: Duration,
) -> EndpointHealth {
    let start = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            return EndpointHealth {
                url: url.to_owned(),
                reachable: false,
                latency_ms: None,
                auth_valid: false,
                error: Some(e.to_string()),
            };
        }
    };

    let mut request = client.get(url);
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }

    match request.send().await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as u32;
            let status = response.status();
            EndpointHealth {
                url: url.to_owned(),
                reachable: status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED,
                latency_ms: Some(latency),
                auth_valid: status != reqwest::StatusCode::UNAUTHORIZED,
                error: if status.is_success() {
                    None
                } else {
                    Some(format!("HTTP {status}"))
                },
            }
        }
        Err(e) => EndpointHealth {
            url: url.to_owned(),
            reachable: false,
            latency_ms: None,
            auth_valid: false,
            error: Some(e.to_string()),
        },
    }
}

/// Probe multiple endpoints concurrently and return ranked results.
/// Best endpoint (lowest latency + reachable + auth_valid) is first.
pub async fn probe_endpoints(
    endpoints: &[(&str, Option<&str>)],
    timeout: Duration,
) -> Vec<EndpointHealth> {
    let mut results: Vec<EndpointHealth> = futures::future::join_all(
        endpoints
            .iter()
            .map(|(url, token)| probe_endpoint(url, token.as_deref(), timeout)),
    )
    .await;

    // Sort: reachable+auth_valid first, then by latency.
    results.sort_by(|a, b| {
        let score_a = endpoint_score(a);
        let score_b = endpoint_score(b);
        score_b.cmp(&score_a)
    });

    results
}

fn endpoint_score(h: &EndpointHealth) -> i32 {
    if h.reachable && h.auth_valid {
        1000 - h.latency_ms.unwrap_or(999) as i32
    } else if h.reachable {
        500
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_unreachable_endpoint() {
        let health = probe_endpoint(
            "http://127.0.0.1:1/healthz",
            None,
            Duration::from_millis(500),
        )
        .await;
        assert!(!health.reachable);
    }

    #[test]
    fn endpoint_scoring_prefers_reachable_low_latency() {
        let good = EndpointHealth {
            url: "a".into(),
            reachable: true,
            latency_ms: Some(10),
            auth_valid: true,
            error: None,
        };
        let slow = EndpointHealth {
            url: "b".into(),
            reachable: true,
            latency_ms: Some(200),
            auth_valid: true,
            error: None,
        };
        let bad = EndpointHealth {
            url: "c".into(),
            reachable: false,
            latency_ms: None,
            auth_valid: false,
            error: Some("conn refused".into()),
        };
        assert!(endpoint_score(&good) > endpoint_score(&slow));
        assert!(endpoint_score(&slow) > endpoint_score(&bad));
    }
}
