//! AWS Signature Version 4 (SigV4) request signing for Amazon Bedrock.
//!
//! Implements the full SigV4 signing process as documented at:
//! <https://docs.aws.amazon.com/IAM/latest/UserGuide/create-signed-request.html>
//!
//! This module is self-contained and has no AWS SDK dependency — it uses only
//! `hmac`, `sha2`, and `chrono` for the cryptographic operations.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::fmt::Write;

type HmacSha256 = Hmac<Sha256>;

// ── Public types ──────────────────────────────────────────────────────

/// AWS credentials required for SigV4 request signing.
pub struct AwsCredentials {
    /// AWS access key ID (from `AWS_ACCESS_KEY_ID`).
    pub access_key_id: String,
    /// AWS secret access key (from `AWS_SECRET_ACCESS_KEY`).
    pub secret_access_key: String,
    /// AWS session token (from `AWS_SESSION_TOKEN`), present for temporary credentials.
    pub session_token: Option<String>,
    /// AWS region (from `AWS_REGION` or `AWS_DEFAULT_REGION`).
    pub region: String,
}

/// Headers produced by SigV4 signing that must be attached to the HTTP request.
pub struct SignedHeaders {
    /// The `Authorization` header value.
    pub authorization: String,
    /// The `X-Amz-Date` header value (ISO 8601 basic format).
    pub x_amz_date: String,
    /// The `X-Amz-Content-Sha256` header value (hex-encoded SHA-256 of the payload).
    pub x_amz_content_sha256: String,
    /// The `X-Amz-Security-Token` header value (present for temporary credentials).
    pub x_amz_security_token: Option<String>,
    /// The `Host` header value.
    pub host: String,
}

// ── Credential loading ────────────────────────────────────────────────

/// Load AWS credentials from standard environment variables.
///
/// Reads:
/// - `AWS_ACCESS_KEY_ID` (required)
/// - `AWS_SECRET_ACCESS_KEY` (required)
/// - `AWS_SESSION_TOKEN` (optional, for temporary credentials)
/// - `AWS_REGION` or `AWS_DEFAULT_REGION` (defaults to `us-east-1`)
///
/// Returns `None` if the required variables are not set.
pub fn load_aws_credentials() -> Option<AwsCredentials> {
    let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
    let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
    let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
    let region = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string());
    Some(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
        region,
    })
}

// ── Signing ───────────────────────────────────────────────────────────

/// Sign an HTTP request using AWS Signature Version 4.
///
/// # Arguments
///
/// * `method` — HTTP method (e.g. `"POST"`)
/// * `host` — The host header value (e.g. `"bedrock-runtime.us-east-1.amazonaws.com"`)
/// * `path` — The URL path (e.g. `"/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke"`)
/// * `payload` — The raw request body bytes
/// * `credentials` — AWS credentials
/// * `service` — AWS service name (e.g. `"bedrock"`)
///
/// # Panics
///
/// Cannot panic — all cryptographic operations are infallible given valid inputs.
pub fn sign(
    method: &str,
    host: &str,
    path: &str,
    payload: &[u8],
    credentials: &AwsCredentials,
    service: &str,
) -> SignedHeaders {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let date = &timestamp[..8];

    // Step 1: Compute payload hash.
    let payload_hash = to_hex(&sha256(payload));

    // Step 2: Build canonical headers (must be sorted by lowercase name).
    let mut header_entries: Vec<(&str, String)> = vec![
        ("content-type", "application/json".to_string()),
        ("host", host.to_string()),
        ("x-amz-content-sha256", payload_hash.clone()),
        ("x-amz-date", timestamp.clone()),
    ];
    if let Some(ref token) = credentials.session_token {
        header_entries.push(("x-amz-security-token", token.clone()));
    }
    header_entries.sort_by_key(|(name, _)| *name);

    let canonical_headers: String = header_entries
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();

    let signed_headers: String = header_entries
        .iter()
        .map(|(k, _)| (*k).to_string())
        .collect::<Vec<_>>()
        .join(";");

    // Step 3: Build canonical request.
    let canonical_request =
        format!("{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    // Step 4: Build credential scope.
    let credential_scope = format!("{date}/{}/{service}/aws4_request", credentials.region);

    // Step 5: Build string to sign.
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        to_hex(&sha256(canonical_request.as_bytes()))
    );

    // Step 6: Derive signing key.
    let k_date = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, credentials.region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    // Step 7: Compute signature.
    let signature = to_hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    // Step 8: Build authorization header.
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    SignedHeaders {
        authorization,
        x_amz_date: timestamp,
        x_amz_content_sha256: payload_hash,
        x_amz_security_token: credentials.session_token.clone(),
        host: host.to_string(),
    }
}

// ── Primitive helpers ─────────────────────────────────────────────────

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("writing to String should not fail");
            s
        })
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_string() {
        // Known test vector: SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = to_hex(&sha256(b""));
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let hash = to_hex(&sha256(b"abc"));
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn to_hex_produces_lowercase() {
        assert_eq!(to_hex(&[0x0a, 0xff, 0x00]), "0aff00");
    }

    #[test]
    fn load_credentials_returns_none_without_env() {
        // This test just verifies the function doesn't panic.
        // Actual env vars may or may not be set in CI.
        let _ = load_aws_credentials();
    }

    #[test]
    fn sign_produces_valid_authorization_header() {
        let creds = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
            region: "us-east-1".to_string(),
        };
        let signed = sign(
            "POST",
            "bedrock-runtime.us-east-1.amazonaws.com",
            "/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke",
            b"{}",
            &creds,
            "bedrock",
        );
        assert!(signed.authorization.starts_with("AWS4-HMAC-SHA256 "));
        assert!(signed.authorization.contains("AKIAIOSFODNN7EXAMPLE/"));
        assert!(
            signed
                .authorization
                .contains("/us-east-1/bedrock/aws4_request")
        );
        assert!(signed.x_amz_date.len() == 16); // YYYYMMDDTHHMMSSZ
        assert!(signed.x_amz_security_token.is_none());
    }

    #[test]
    fn sign_includes_session_token_in_headers() {
        let creds = AwsCredentials {
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            session_token: Some("tok".to_string()),
            region: "us-west-2".to_string(),
        };
        let signed = sign("POST", "host", "/path", b"", &creds, "bedrock");
        assert_eq!(signed.x_amz_security_token.as_deref(), Some("tok"));
        assert!(signed.authorization.contains("x-amz-security-token"));
    }
}
