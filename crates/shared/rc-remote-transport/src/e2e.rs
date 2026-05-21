//! Application-layer end-to-end encryption.
//!
//! Uses X25519 for key exchange and AES-256-GCM for payload encryption.
//! Keys never leave the mobile device and the runner — the control plane
//! sees only encrypted blobs.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
use x25519_dalek::{EphemeralSecret, PublicKey};

/// An E2E encryption session between two endpoints.
pub struct E2eSession {
    cipher: Aes256Gcm,
}

impl E2eSession {
    /// Perform a Diffie-Hellman key exchange using the peer's public key
    /// and our ephemeral secret. Returns the session.
    pub fn from_secret_and_public(secret: EphemeralSecret, peer_public: &PublicKey) -> Self {
        let shared = secret.diffie_hellman(peer_public);
        let key = aes_gcm::Key::<Aes256Gcm>::from_slice(shared.as_bytes());
        let cipher = Aes256Gcm::new(key);
        Self { cipher }
    }

    /// Generate a new ephemeral keypair for key exchange.
    pub fn generate_keypair() -> (EphemeralSecret, PublicKey) {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        (secret, public)
    }

    /// Encrypt a plaintext payload. Returns nonce + ciphertext.
    pub fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 96-bit random nonce
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;
        // Prepend nonce to ciphertext.
        let mut output = Vec::with_capacity(12 + ciphertext.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    /// Decrypt a payload produced by `encrypt`. Returns plaintext.
    pub fn decrypt(&self, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
        if payload.len() < 12 {
            anyhow::bail!("payload too short for nonce");
        }
        let nonce = Nonce::from_slice(&payload[..12]);
        let ciphertext = &payload[12..];
        self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (secret_a, public_a) = E2eSession::generate_keypair();
        let (secret_b, public_b) = E2eSession::generate_keypair();

        let session_a = E2eSession::from_secret_and_public(secret_a, &public_b);
        let session_b = E2eSession::from_secret_and_public(secret_b, &public_a);

        let plaintext = b"hello, encrypted world!";
        let encrypted = session_a.encrypt(plaintext).unwrap();
        let decrypted = session_b.decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (secret_a, public_a) = E2eSession::generate_keypair();
        let (secret_b, public_b) = E2eSession::generate_keypair();

        let session_a = E2eSession::from_secret_and_public(secret_a, &public_b);
        let session_b = E2eSession::from_secret_and_public(secret_b, &public_a);

        let mut encrypted = session_a.encrypt(b"secret data").unwrap();
        // Tamper with ciphertext.
        if let Some(last) = encrypted.last_mut() {
            *last ^= 0xff;
        }
        assert!(session_b.decrypt(&encrypted).is_err());
    }
}
