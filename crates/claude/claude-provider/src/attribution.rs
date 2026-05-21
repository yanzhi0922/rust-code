//! Attribution header construction.
//!
//! Builds the `x-attribution` HTTP header and the `x-anthropic-billing-header`
//! text block embedded in the system prompt.
//!
//! Based on upstream Claude Code's `getAttributionHeader` in
//! `constants/system.ts` and `computeFingerprint` in `utils/fingerprint.ts`.

use reqwest::header::{HeaderName, HeaderValue};

/// The HTTP attribution header name.
pub const ATTRIBUTION_HEADER: &str = "x-attribution";

/// Build the `x-attribution` HTTP header value.
///
/// The official CLI sends a JSON object with `client` and `version` keys.
/// We match the exact format used by Claude Code.
pub fn build_attribution_header() -> Result<HeaderValue, reqwest::header::InvalidHeaderValue> {
    let value = format!(
        r#"{{"client":"claude-code","version":"{}"}}"#,
        claude_config::runtime_version()
    );
    HeaderValue::from_str(&value)
}

/// Build the attribution header as a `(HeaderName, HeaderValue)` pair.
pub fn build_attribution_header_pair()
-> Result<(HeaderName, HeaderValue), reqwest::header::InvalidHeaderValue> {
    let name = HeaderName::from_static(ATTRIBUTION_HEADER);
    let value = build_attribution_header()?;
    Ok((name, value))
}

/// Build the `x-anthropic-billing-header` text that is prepended as the
/// **first** block of the system prompt array.
///
/// Format: `x-anthropic-billing-header: cc_version=VERSION.FINGERPRINT; cc_entrypoint=ENTRYPOINT;`
///
/// This matches the TS reference `getAttributionHeader` in `constants/system.ts`.
pub fn build_billing_attribution_text(fingerprint: &str) -> String {
    let version = claude_config::runtime_version();
    let entrypoint = std::env::var("CLAUDE_CODE_ENTRYPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "cli".to_owned());
    format!(
        "x-anthropic-billing-header: cc_version={version}.{fingerprint}; cc_entrypoint={entrypoint};"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_header_format() {
        let header = build_attribution_header().expect("should build header");
        let value = header.to_str().expect("should be valid UTF-8");
        assert!(value.contains("\"client\":\"claude-code\""));
        assert!(value.contains("\"version\""));
    }

    #[test]
    fn attribution_header_pair_is_valid() {
        let (name, value) = build_attribution_header_pair().expect("should build pair");
        assert_eq!(name.as_str(), ATTRIBUTION_HEADER);
        assert!(value.to_str().is_ok());
    }

    #[test]
    fn billing_attribution_text_format() {
        let text = build_billing_attribution_text("abc");
        assert!(text.starts_with("x-anthropic-billing-header: cc_version="));
        assert!(text.contains(".abc;"));
        assert!(text.contains("cc_entrypoint="));
    }
}
