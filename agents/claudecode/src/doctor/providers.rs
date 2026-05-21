use claude_config::{ProviderConfig, discover_env_providers};
use claude_core::ProviderProtocol;
use serde::Serialize;

use super::network::ProbeSpec;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EnvProviderSummary {
    pub name: String,
    pub protocol: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key_present: bool,
}

pub(crate) fn env_provider_summaries() -> Vec<EnvProviderSummary> {
    let mut providers = discover_env_providers()
        .into_iter()
        .map(|provider| EnvProviderSummary {
            name: provider.name,
            protocol: provider.protocol.as_str().to_owned(),
            base_url: provider.base_url,
            model: provider.model,
            api_key_present: provider.api_key.is_some(),
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.name.cmp(&right.name));
    providers
}

pub(crate) fn provider_endpoint_url(provider: &ProviderConfig) -> Option<String> {
    provider
        .base_url
        .clone()
        .or_else(|| default_protocol_endpoint(provider.protocol))
}

pub(crate) fn provider_probe_spec(provider: &ProviderConfig) -> Option<ProbeSpec> {
    let url = provider_endpoint_url(provider)?;
    let mut spec = ProbeSpec::new(format!("provider:{}", provider.name), url);
    spec.headers.extend(
        provider
            .request_header_overrides
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );

    match provider.protocol {
        ProviderProtocol::Anthropic => {
            spec = spec.with_header("anthropic-version", "2023-06-01");
            if let Some(api_key) = &provider.api_key {
                spec = spec.with_header("x-api-key", api_key);
            }
        }
        ProviderProtocol::OpenAi => {
            if let Some(api_key) = &provider.api_key {
                spec = spec.with_header("authorization", format!("Bearer {api_key}"));
            }
        }
        ProviderProtocol::Bedrock | ProviderProtocol::Vertex => {}
    }

    Some(spec)
}

fn default_protocol_endpoint(protocol: ProviderProtocol) -> Option<String> {
    match protocol {
        ProviderProtocol::Anthropic => Some("https://api.anthropic.com/v1/messages".to_owned()),
        ProviderProtocol::OpenAi => Some("https://api.openai.com/v1/chat/completions".to_owned()),
        ProviderProtocol::Bedrock | ProviderProtocol::Vertex => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{provider_endpoint_url, provider_probe_spec};
    use claude_config::ProviderConfig;
    use claude_core::ProviderProtocol;
    use std::collections::BTreeMap;

    fn provider(protocol: ProviderProtocol, base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: "test".to_owned(),
            base_url: base_url.map(ToOwned::to_owned),
            api_key: Some("secret".to_owned()),
            model: Some("model".to_owned()),
            protocol,
            timeout_ms: 60_000,
            max_output_tokens: 4_096,
            max_retries: 2,
            retry_initial_backoff_ms: 100,
            retry_max_backoff_ms: 1_000,
            respect_retry_after: true,
            request_header_overrides: BTreeMap::new(),
            request_metadata: BTreeMap::new(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }
    }

    #[test]
    fn provider_probe_uses_protocol_defaults_when_base_url_is_missing() {
        let anthropic = provider(ProviderProtocol::Anthropic, None);
        assert_eq!(
            provider_endpoint_url(&anthropic).as_deref(),
            Some("https://api.anthropic.com/v1/messages")
        );

        let openai = provider(ProviderProtocol::OpenAi, None);
        assert_eq!(
            provider_endpoint_url(&openai).as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
    }

    #[test]
    fn provider_probe_adds_protocol_auth_headers() {
        let anthropic = provider_probe_spec(&provider(
            ProviderProtocol::Anthropic,
            Some("https://example.com/anthropic/v1/messages"),
        ))
        .expect("anthropic provider should build probe spec");
        assert!(
            anthropic
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        );

        let openai = provider_probe_spec(&provider(
            ProviderProtocol::OpenAi,
            Some("https://example.com/v1/chat/completions"),
        ))
        .expect("openai provider should build probe spec");
        assert!(
            openai
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value.starts_with("Bearer "))
        );
    }
}
