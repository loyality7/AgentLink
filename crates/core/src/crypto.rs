use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

use crate::{
    identity::{decode_32, Identity, PublicIdentity},
    CoreError, Result,
};

/// End-to-end encrypted envelope routed by the relay.
/// The relay only sees routing metadata (`from`, `to`, `id`) and ciphertext.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    pub v: u8,
    pub id: String,
    pub from: String,
    pub to: String,
    /// Ephemeral X25519 public key (base64) used for forward-secret key agreement.
    pub ephemeral_pk: String,
    /// 24-byte XChaCha20-Poly1305 nonce (base64).
    pub nonce: String,
    /// Authenticated ciphertext (base64).
    pub ciphertext: String,
    /// Ed25519 signature over canonical envelope fields by `from`.
    pub signature: String,
}

fn derive_message_key(shared_secret: &[u8; 32], from: &str, to: &str, id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"agentlink:v1:msg-key:");
    hasher.update(shared_secret);
    hasher.update(from.as_bytes());
    hasher.update(b":");
    hasher.update(to.as_bytes());
    hasher.update(b":");
    hasher.update(id.as_bytes());
    hasher.finalize().into()
}

fn canonical_signing_bytes(
    v: u8,
    id: &str,
    from: &str,
    to: &str,
    ephemeral_pk: &str,
    nonce: &str,
    ciphertext: &str,
) -> Vec<u8> {
    format!("{v}|{id}|{from}|{to}|{ephemeral_pk}|{nonce}|{ciphertext}").into_bytes()
}

/// Encrypt and sign a plaintext payload for a verified peer.
pub fn encrypt_for_peer(
    sender: &Identity,
    recipient: &PublicIdentity,
    msg_id: impl Into<String>,
    plaintext: &[u8],
) -> Result<EncryptedEnvelope> {
    recipient.verify_peer_id()?;

    let id = msg_id.into();
    let eph_secret = EphemeralSecret::random_from_rng(OsRng);
    let eph_pk = X25519PublicKey::from(&eph_secret);
    let recipient_kx_pk = recipient.kx_public_key()?;

    let shared = eph_secret.diffie_hellman(&recipient_kx_pk);
    let key = derive_message_key(shared.as_bytes(), &sender.peer_id, &recipient.peer_id, &id);

    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&key.into());
    let ciphertext_bytes = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|e| CoreError::Encryption(e.to_string()))?;

    let ephemeral_pk = B64.encode(eph_pk.as_bytes());
    let nonce = B64.encode(nonce_bytes);
    let ciphertext = B64.encode(ciphertext_bytes);

    let to_sign = canonical_signing_bytes(
        1,
        &id,
        &sender.peer_id,
        &recipient.peer_id,
        &ephemeral_pk,
        &nonce,
        &ciphertext,
    );
    let signature = sender.sign(&to_sign)?;

    Ok(EncryptedEnvelope {
        v: 1,
        id,
        from: sender.peer_id.clone(),
        to: recipient.peer_id.clone(),
        ephemeral_pk,
        nonce,
        ciphertext,
        signature,
    })
}

/// Verify signature and decrypt an incoming envelope from a known peer.
pub fn decrypt_from_peer(
    recipient: &Identity,
    sender: &PublicIdentity,
    env: &EncryptedEnvelope,
) -> Result<Vec<u8>> {
    sender.verify_peer_id()?;
    if env.from != sender.peer_id || env.to != recipient.peer_id {
        return Err(CoreError::InvalidSignature);
    }

    let signed_bytes = canonical_signing_bytes(
        env.v,
        &env.id,
        &env.from,
        &env.to,
        &env.ephemeral_pk,
        &env.nonce,
        &env.ciphertext,
    );
    sender.verify_signature(&signed_bytes, &env.signature)?;

    let eph_pk_bytes = decode_32(&env.ephemeral_pk, "ephemeral_pk")?;
    let eph_pk = X25519PublicKey::from(eph_pk_bytes);
    let shared = recipient.kx_secret()?.diffie_hellman(&eph_pk);
    let key = derive_message_key(shared.as_bytes(), &env.from, &env.to, &env.id);

    let nonce_vec = B64.decode(&env.nonce).map_err(|_| CoreError::Decryption)?;
    let nonce_arr: [u8; 24] = nonce_vec.try_into().map_err(|_| CoreError::Decryption)?;
    let ciphertext = B64
        .decode(&env.ciphertext)
        .map_err(|_| CoreError::Decryption)?;

    let cipher = XChaCha20Poly1305::new(&key.into());
    cipher
        .decrypt(XNonce::from_slice(&nonce_arr), ciphertext.as_slice())
        .map_err(|_| CoreError::Decryption)
}

