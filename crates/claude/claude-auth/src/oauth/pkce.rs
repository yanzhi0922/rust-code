//! PKCE (Proof Key for Code Exchange) utilities.
//!
//! Implements the S256 code challenge method as specified in RFC 7636.
//! Mirrors `services/oauth/crypto.ts`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Generate a cryptographically-random `code_verifier` (43–128 chars,
/// URL-safe base64 of 32 random bytes).
///
/// Uses `getrandom` for cryptographically secure randomness.
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    // getrandom provides cryptographically secure random bytes from the OS CSPRNG.
    // Falls back to a panic only if the system RNG is fundamentally broken.
    getrandom::fill(&mut bytes)
        .expect("getrandom: system RNG failure — cannot generate secure code_verifier");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive the `code_challenge` from a `code_verifier` using SHA-256
/// and URL-safe base64 encoding (no padding).
pub fn generate_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

/// Generate a random `state` parameter for CSRF protection.
///
/// Uses `getrandom` for cryptographically secure randomness.
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .expect("getrandom: system RNG failure — cannot generate secure state");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_verifier_length() {
        let v = generate_code_verifier();
        // 32 bytes → 43 chars in URL_SAFE_NO_PAD
        assert_eq!(v.len(), 43);
    }

    #[test]
    fn code_challenge_deterministic() {
        let verifier = "test-verifier-value";
        let c1 = generate_code_challenge(verifier);
        let c2 = generate_code_challenge(verifier);
        assert_eq!(c1, c2);
    }

    #[test]
    fn state_length() {
        let s = generate_state();
        assert_eq!(s.len(), 43);
    }

    #[test]
    fn different_verifiers() {
        let v1 = generate_code_verifier();
        let v2 = generate_code_verifier();
        // Statistically should differ
        assert_ne!(v1, v2);
    }

    #[test]
    fn verifier_is_url_safe() {
        let v = generate_code_verifier();
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn state_is_url_safe() {
        let s = generate_state();
        assert!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}
