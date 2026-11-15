use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519Secret};

use crate::{CoreError, Result};

/// Public identity shared with peers and the relay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicIdentity {
    pub peer_id: String,
    pub display_name: String,
    /// Base64-encoded 32-byte Ed25519 verifying key.
    pub sign_pk: String,
    /// Base64-encoded 32-byte X25519 public key.
    pub kx_pk: String,
}

/// Full local identity including private keys (stored only on the user's machine).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub peer_id: String,
    pub display_name: String,
    /// Base64-encoded 32-byte Ed25519 signing seed.
    pub sign_sk: String,
    /// Base64-encoded 32-byte Ed25519 verifying key.
    pub sign_pk: String,
    /// Base64-encoded 32-byte X25519 static secret.
    pub kx_sk: String,
    /// Base64-encoded 32-byte X25519 public key.
    pub kx_pk: String,
}

/// Compute a 32-hex-char fingerprint bound to both the signing and key-exchange public keys.
pub fn compute_peer_id(sign_pk: &[u8; 32], kx_pk: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentlink:v1:id:");
    hasher.update(sign_pk);
    hasher.update(kx_pk);
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

impl Identity {
    /// Generate a fresh Ed25519 + X25519 identity.
    pub fn generate(display_name: impl Into<String>) -> Self {
        let sign_key = SigningKey::generate(&mut OsRng);
        let sign_pk_bytes = sign_key.verifying_key().to_bytes();

        let kx_secret = X25519Secret::random_from_rng(OsRng);
        let kx_pk_bytes = X25519PublicKey::from(&kx_secret).to_bytes();

        let peer_id = compute_peer_id(&sign_pk_bytes, &kx_pk_bytes);

        Self {
            peer_id,
            display_name: display_name.into(),
            sign_sk: B64.encode(sign_key.to_bytes()),
            sign_pk: B64.encode(sign_pk_bytes),
            kx_sk: B64.encode(kx_secret.to_bytes()),
            kx_pk: B64.encode(kx_pk_bytes),
        }
    }

    pub fn to_public(&self) -> PublicIdentity {
        PublicIdentity {
            peer_id: self.peer_id.clone(),
            display_name: self.display_name.clone(),
            sign_pk: self.sign_pk.clone(),
            kx_pk: self.kx_pk.clone(),
        }
    }

    pub fn signing_key(&self) -> Result<SigningKey> {
        let bytes = decode_32(&self.sign_sk, "sign_sk")?;
        Ok(SigningKey::from_bytes(&bytes))
    }

    pub fn kx_secret(&self) -> Result<X25519Secret> {
        let bytes = decode_32(&self.kx_sk, "kx_sk")?;
        Ok(X25519Secret::from(bytes))
    }

    /// Sign an arbitrary byte slice with the Ed25519 signing key.
    pub fn sign(&self, message: &[u8]) -> Result<String> {
        let key = self.signing_key()?;
        let sig = key.sign(message);
        Ok(B64.encode(sig.to_bytes()))
    }
}

impl PublicIdentity {
    /// Verify that `peer_id` matches the SHA-256 fingerprint of `sign_pk` and `kx_pk`.
    pub fn verify_peer_id(&self) -> Result<()> {
        let sign_bytes = decode_32(&self.sign_pk, "sign_pk")?;
        let kx_bytes = decode_32(&self.kx_pk, "kx_pk")?;
        let expected = compute_peer_id(&sign_bytes, &kx_bytes);
        if expected != self.peer_id {
            return Err(CoreError::PeerIdMismatch {
                expected,
                actual: self.peer_id.clone(),
            });
        }
        Ok(())
    }

    pub fn kx_public_key(&self) -> Result<X25519PublicKey> {
        let bytes = decode_32(&self.kx_pk, "kx_pk")?;
        Ok(X25519PublicKey::from(bytes))
    }

    /// Verify a base64-encoded Ed25519 signature against this peer's verifying key.
    pub fn verify_signature(&self, message: &[u8], signature_b64: &str) -> Result<()> {
        let pk_bytes = decode_32(&self.sign_pk, "sign_pk")?;
        let verifying_key =
            VerifyingKey::from_bytes(&pk_bytes).map_err(|e| CoreError::InvalidKey(e.to_string()))?;

        let sig_bytes = B64
            .decode(signature_b64)
            .map_err(|_| CoreError::InvalidSignature)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| CoreError::InvalidSignature)?;
        let signature = Signature::from_bytes(&sig_arr);

        verifying_key
            .verify(message, &signature)
            .map_err(|_| CoreError::InvalidSignature)
    }
}

pub(crate) fn decode_32(b64: &str, label: &str) -> Result<[u8; 32]> {
    let bytes = B64
        .decode(b64)
        .map_err(|e| CoreError::InvalidKey(format!("{label}: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| CoreError::InvalidKey(format!("{label}: expected 32 bytes")))
}

