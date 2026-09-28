//! Sealing push content to one subscription's notification key (#200).
//!
//! The scheme, exactly as `planning/v2/Push Content Security Checklist.md`
//! states it and `fixtures/push/sealed-v1.json` holds it:
//!
//! ```text
//! blob   = b64u(0x01 ‖ epk ‖ nonce ‖ ciphertext‖tag)
//! shared = ECDH(ephemeral private, recipient public), the x coordinate
//! key    = HKDF-SHA256(salt = empty, ikm = shared,
//!                      info = "build-push-v1" ‖ epk ‖ recipient public, L = 32)
//! aad    = "build-push-v1" ‖ 0 ‖ sid ‖ 0 ‖ kind ‖ 0 ‖ entity id
//! plaintext = the JSON, padded with trailing spaces to 1024 bytes
//! ```
//!
//! This is ECIES with an ephemeral sender key, so it is **not
//! sender-authenticated**: anyone holding a subscription's notification public
//! key can seal a blob that opens. Forgery by the api or the push service is
//! prevented only because that public key stays secret — it is made in the
//! browser and travels to the bridge only over the E2EE session.
//!
//! Errors name the kind of failure and nothing else: never a key, never the
//! words being sealed.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde::Serialize;
use sha2::Sha256;

use super::content::PushContent;

/// The label every derivation and every AAD starts with.
const LABEL: &[u8] = b"build-push-v1";
/// The blob's first byte.
const BLOB_VERSION: u8 = 0x01;
/// An uncompressed P-256 point: `0x04 ‖ x ‖ y`.
const UNCOMPRESSED_POINT_LEN: usize = 65;
const NONCE_LEN: usize = 12;
/// A subscription id is `b64u(SHA-256(endpoint))`: 32 bytes, 43 characters.
const SUBSCRIPTION_ID_LEN: usize = 43;
/// Every plaintext is padded to exactly 1 KiB.
pub const PLAINTEXT_MAX_BYTES: usize = 1024;
/// Nor the blob 2048 characters.
pub const BLOB_MAX_CHARS: usize = 2048;

/// Why a seal did not happen. Each names a kind of failure, never content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("the notification key is not an uncompressed P-256 point")]
    BadKey,
    #[error("the plaintext is over its size cap")]
    PlaintextTooLarge,
    #[error("the blob is over its size cap")]
    BlobTooLarge,
    #[error("the cipher refused")]
    Cipher,
}

/// The plaintext's JSON, fields in the scheme's order.
#[derive(Serialize)]
struct Plaintext<'a> {
    v: u8,
    title: &'a str,
    body: &'a str,
    url: &'a str,
    iat: i64,
}

/// What one blob is bound to: the subscription it is for, and the cleartext
/// kind and entity id the api wraps it in.
#[derive(Debug, Clone, Copy)]
pub struct Binding<'a> {
    pub subscription_id: &'a str,
    pub kind: &'a str,
    pub entity_id: &'a str,
}

impl Binding<'_> {
    fn aad(&self) -> Vec<u8> {
        [
            LABEL,
            self.subscription_id.as_bytes(),
            self.kind.as_bytes(),
            self.entity_id.as_bytes(),
        ]
        .join(&0u8)
    }
}

/// Whether `sid` is shaped like a subscription id: 43 base64url characters
/// that decode to 32 bytes.
pub fn is_subscription_id(sid: &str) -> bool {
    sid.len() == SUBSCRIPTION_ID_LEN
        && URL_SAFE_NO_PAD
            .decode(sid)
            .is_ok_and(|bytes| bytes.len() == 32)
}

/// The recipient's notification key out of its `b64u(uncompressed point)`,
/// refusing anything that is not a point on P-256 written uncompressed.
pub fn parse_public_key(public_key_b64u: &str) -> Result<PublicKey, SealError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(public_key_b64u)
        .map_err(|_| SealError::BadKey)?;
    if bytes.len() != UNCOMPRESSED_POINT_LEN || bytes[0] != 0x04 {
        return Err(SealError::BadKey);
    }
    PublicKey::from_sec1_bytes(&bytes).map_err(|_| SealError::BadKey)
}

/// Seal `content` for one subscription at `iat` (unix seconds), with a fresh
/// ephemeral key and nonce from the OS.
pub fn seal(
    recipient_b64u: &str,
    binding: Binding,
    content: &PushContent,
    iat: i64,
) -> Result<String, SealError> {
    let ephemeral = SecretKey::random(&mut OsRng);
    let nonce: [u8; NONCE_LEN] = rand_nonce();
    seal_with(recipient_b64u, binding, content, iat, &ephemeral, nonce)
}

fn rand_nonce() -> [u8; NONCE_LEN] {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    nonce.into()
}

/// [`seal`] with the ephemeral key and nonce given: the seam the vector test
/// reproduces `fixtures/push/sealed-v1.json` through.
pub(crate) fn seal_with(
    recipient_b64u: &str,
    binding: Binding,
    content: &PushContent,
    iat: i64,
    ephemeral: &SecretKey,
    nonce: [u8; NONCE_LEN],
) -> Result<String, SealError> {
    let recipient = parse_public_key(recipient_b64u)?;
    let plaintext = plaintext_json(content, iat)?;
    let epk = uncompressed(&ephemeral.public_key());
    let key = derive_key(
        &shared_x(ephemeral, &recipient),
        &epk,
        &uncompressed(&recipient),
    );
    let sealed = Aes256Gcm::new(&key.into())
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &binding.aad(),
            },
        )
        .map_err(|_| SealError::Cipher)?;
    let blob = URL_SAFE_NO_PAD.encode([&[BLOB_VERSION][..], &epk, &nonce, &sealed].concat());
    if blob.len() > BLOB_MAX_CHARS {
        return Err(SealError::BlobTooLarge);
    }
    Ok(blob)
}

fn plaintext_json(content: &PushContent, iat: i64) -> Result<Vec<u8>, SealError> {
    let plaintext = serde_json::to_vec(&Plaintext {
        v: 1,
        title: &content.title,
        body: &content.body,
        url: &content.url,
        iat,
    })
    .map_err(|_| SealError::Cipher)?;
    pad(plaintext)
}

/// `plaintext` with trailing spaces to exactly [`PLAINTEXT_MAX_BYTES`], so
/// every blob is the same length and its size says nothing about the event.
/// JSON allows the whitespace; serde and `JSON.parse` both skip it.
fn pad(mut plaintext: Vec<u8>) -> Result<Vec<u8>, SealError> {
    if plaintext.len() > PLAINTEXT_MAX_BYTES {
        return Err(SealError::PlaintextTooLarge);
    }
    plaintext.resize(PLAINTEXT_MAX_BYTES, b' ');
    Ok(plaintext)
}

fn uncompressed(key: &PublicKey) -> Vec<u8> {
    key.to_encoded_point(false).as_bytes().to_vec()
}

/// The ECDH shared secret's x coordinate between `secret` and `peer`.
fn shared_x(secret: &SecretKey, peer: &PublicKey) -> p256::ecdh::SharedSecret {
    p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine())
}

/// HKDF-SHA256 over the shared x coordinate, bound to the ephemeral and the
/// recipient public keys.
fn derive_key(shared: &p256::ecdh::SharedSecret, epk: &[u8], recipient: &[u8]) -> [u8; 32] {
    let info = [LABEL, epk, recipient].concat();
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&[]), shared.raw_secret_bytes())
        .expand(&info, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    key
}

/// Open a blob with the recipient's secret: what the service worker does, for
/// the tests to hold the seal to.
#[cfg(test)]
pub(crate) fn open(
    recipient: &SecretKey,
    binding: Binding,
    blob: &str,
) -> Result<Vec<u8>, SealError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(blob)
        .map_err(|_| SealError::Cipher)?;
    let header = 1 + UNCOMPRESSED_POINT_LEN + NONCE_LEN;
    if bytes.len() < header || bytes[0] != BLOB_VERSION {
        return Err(SealError::Cipher);
    }
    let epk = &bytes[1..1 + UNCOMPRESSED_POINT_LEN];
    let nonce = &bytes[1 + UNCOMPRESSED_POINT_LEN..header];
    let ephemeral = PublicKey::from_sec1_bytes(epk).map_err(|_| SealError::BadKey)?;
    let key = derive_key(
        &shared_x(recipient, &ephemeral),
        epk,
        &uncompressed(&recipient.public_key()),
    );
    Aes256Gcm::new(&key.into())
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: &bytes[header..],
                aad: &binding.aad(),
            },
        )
        .map_err(|_| SealError::Cipher)
}

#[cfg(test)]
mod tests;
