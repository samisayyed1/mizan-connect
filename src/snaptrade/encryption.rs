//! AES-256-GCM encryption for SnapTrade `userSecret` at rest.
//!
//! Storage layout:
//!
//! ```text
//! [ 12 byte nonce | ciphertext | 16 byte GCM tag ]
//! ```
//!
//! `aes-gcm`'s [`Aes256Gcm::encrypt`] returns `ciphertext || tag` already
//! concatenated; we prepend the nonce so the blob is fully self-describing.
//! The decrypted plaintext is returned as a [`SecretString`] — never `String`
//! — so it cannot be accidentally logged or formatted.
//!
//! The encryption key is loaded from `MIZAN_BROKER_SECRET_ENCRYPTION_KEY`
//! at startup (validated to be exactly 32 bytes) and stored on
//! [`crate::state::AppState`] inside [`EncryptionKey`].
//!
//! On startup we round-trip a known plaintext via [`EncryptionKey::self_test`]
//! to fail fast if the key was rotated incorrectly or the underlying crate
//! changed behavior.

use std::sync::Arc;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Errors that can arise during encryption / decryption.
#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error("encryption key must be exactly {KEY_LEN} bytes, got {0}")]
    InvalidKeyLength(usize),
    #[error("encryption failed")]
    EncryptFailed,
    #[error("decryption failed (truncated, wrong key, or tampered ciphertext)")]
    DecryptFailed,
    #[error("encryption self-test failed: round-trip mismatch")]
    SelfTestFailed,
    #[error("encrypted blob too short: need at least {} bytes, got {0}", NONCE_LEN + 16)]
    BlobTooShort(usize),
    #[error("plaintext is not valid UTF-8 — corruption suspected")]
    PlaintextNotUtf8,
}

/// 32-byte AES-256-GCM key. Cheap to clone (wraps `Arc`).
#[derive(Clone)]
pub struct EncryptionKey {
    inner: Arc<[u8; KEY_LEN]>,
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptionKey")
            .field("len", &KEY_LEN)
            .finish()
    }
}

impl EncryptionKey {
    /// Construct from raw key bytes. Length must be exactly 32.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EncryptionError> {
        if bytes.len() != KEY_LEN {
            return Err(EncryptionError::InvalidKeyLength(bytes.len()));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(bytes);
        Ok(Self {
            inner: Arc::new(key),
        })
    }

    fn cipher(&self) -> Aes256Gcm {
        let key = Key::<Aes256Gcm>::from_slice(self.inner.as_ref());
        Aes256Gcm::new(key)
    }

    /// Encrypt a plaintext string into a self-describing blob.
    ///
    /// Output layout: `nonce (12 bytes) || ciphertext || tag (16 bytes)`.
    /// A fresh random nonce is generated per call.
    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, EncryptionError> {
        let cipher = self.cipher();
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| EncryptionError::EncryptFailed)?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a blob produced by [`Self::encrypt`].
    ///
    /// Returns the plaintext as [`SecretString`] so it cannot be accidentally
    /// logged or formatted; callers expose it only at the moment of HTTP
    /// signing via [`SecretString::expose_secret`].
    pub fn decrypt(&self, blob: &[u8]) -> Result<SecretString, EncryptionError> {
        // 12 byte nonce + at least 16 byte tag = 28 byte minimum
        if blob.len() < NONCE_LEN + 16 {
            return Err(EncryptionError::BlobTooShort(blob.len()));
        }
        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = self.cipher();
        let mut plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| EncryptionError::DecryptFailed)?;
        let secret = String::from_utf8(plaintext.clone()).map_err(|_| {
            plaintext.zeroize();
            EncryptionError::PlaintextNotUtf8
        })?;
        plaintext.zeroize();
        Ok(SecretString::from(secret))
    }

    /// Round-trip a fixed plaintext to fail fast at startup if the key,
    /// crate, or layout assumption is broken.
    pub fn self_test(&self) -> Result<(), EncryptionError> {
        let plaintext = "mizan-encryption-self-test";
        let blob = self.encrypt(plaintext)?;
        let decrypted = self.decrypt(&blob)?;
        if decrypted.expose_secret() != plaintext {
            return Err(EncryptionError::SelfTestFailed);
        }
        // A second encrypt of the same plaintext must produce a different
        // blob (random nonce is functioning).
        let blob2 = self.encrypt(plaintext)?;
        if blob == blob2 {
            return Err(EncryptionError::SelfTestFailed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn fixed_key() -> EncryptionKey {
        // Deterministic 32-byte key for unit tests.
        EncryptionKey::from_bytes(&[7u8; KEY_LEN]).expect("32-byte key")
    }

    #[test]
    fn round_trip_returns_same_plaintext() {
        let key = fixed_key();
        let blob = key.encrypt("hello-snaptrade").expect("encrypt");
        let out = key.decrypt(&blob).expect("decrypt");
        assert_eq!(out.expose_secret(), "hello-snaptrade");
    }

    #[test]
    fn rejects_short_blob() {
        let key = fixed_key();
        let err = key.decrypt(&[0u8; 5]).unwrap_err();
        assert!(matches!(err, EncryptionError::BlobTooShort(_)));
    }

    #[test]
    fn tampered_ciphertext_fails_aead() {
        let key = fixed_key();
        let mut blob = key.encrypt("hello").expect("encrypt");
        // Flip a bit in the tag area.
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        let err = key.decrypt(&blob).unwrap_err();
        assert!(matches!(err, EncryptionError::DecryptFailed));
    }

    #[test]
    fn wrong_key_fails_aead() {
        let blob = fixed_key().encrypt("hello").expect("encrypt");
        let other = EncryptionKey::from_bytes(&[8u8; KEY_LEN]).expect("32-byte");
        let err = other.decrypt(&blob).unwrap_err();
        assert!(matches!(err, EncryptionError::DecryptFailed));
    }

    #[test]
    fn rejects_non_32_byte_key() {
        let err = EncryptionKey::from_bytes(&[0u8; 16]).unwrap_err();
        assert!(matches!(err, EncryptionError::InvalidKeyLength(16)));
    }

    #[test]
    fn nonce_is_unique_across_encryptions() {
        let key = fixed_key();
        let a = key.encrypt("same").expect("a");
        let b = key.encrypt("same").expect("b");
        assert_ne!(a, b, "fresh nonce per call");
        assert_eq!(&a[..NONCE_LEN], &a[..NONCE_LEN]);
        assert_ne!(&a[..NONCE_LEN], &b[..NONCE_LEN]);
    }

    #[test]
    fn self_test_passes() {
        fixed_key().self_test().expect("self-test");
    }
}
