//! Rust binding for the Build secure-transport E2EE protocol (`E2E-MVP.md`).
//!
//! This is a *port of an audited protocol*, not new cryptography: it drives the
//! same libsodium primitives the Python/JS reference uses (X25519 transport keys,
//! Ed25519 identity, sealed box to wrap a per-session key, secretbox for inner
//! frames) via [`dryoc`], and is verified byte-for-byte against the reference by
//! the live cross-language interop test. The relay only ever sees the opaque outer
//! envelope; everything else is the peers' responsibility.

use base64::Engine;
use dryoc::classic::crypto_box::{crypto_box_seal, crypto_box_seal_open};
use dryoc::classic::crypto_core::crypto_scalarmult_base;
use dryoc::classic::crypto_secretbox::{crypto_secretbox_easy, crypto_secretbox_open_easy};
use dryoc::classic::crypto_sign::{
    crypto_sign_detached, crypto_sign_seed_keypair, crypto_sign_verify_detached,
};
use dryoc::rng::randombytes_buf;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Wire protocol version carried in every outer envelope.
pub const PROTOCOL_VERSION: u32 = 1;
const SESSION_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const SEAL_BYTES: usize = 48; // 32-byte ephemeral pubkey + 16-byte MAC
const MAC_BYTES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("decryption failed")]
    Decryption,
    #[error("signature verification failed")]
    Verification,
    #[error("protocol error: {0}")]
    Protocol(String),
}

type Result<T> = std::result::Result<T, TransportError>;

/// A base64 (unpadded) keypair, mirroring the reference's field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPairB64 {
    pub public_key_b64: String,
    pub private_key_b64: String,
}

/// The relay-visible outer envelope. Only these fields are ever in the clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub session_id: String,
    pub route_to: String,
    /// base64, 24 bytes.
    pub nonce: String,
    /// base64 secretbox output (MAC || ciphertext).
    pub ciphertext: String,
}

/// The session-bootstrap message a client sends to a device through the relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInit {
    pub session_id: String,
    pub device_id: String,
    /// base64 sealed box of the 32-byte session key, to the device transport key.
    pub wrapped_session_key: String,
}

/// The result of a device unwrapping a `SessionInit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenedSession {
    pub session_key_b64: String,
    pub session_id: String,
    pub device_id: String,
}

/// The peer-visible inner frame (encrypted inside an envelope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub session_id: String,
    pub message_id: String,
    pub frame_type: String,
    pub sender: String,
    pub created_at: String,
    pub payload: Value,
}

/// The outer routing fields a sender supplies for a frame.
#[derive(Debug, Clone)]
pub struct OuterFields {
    pub session_id: String,
    pub route_to: String,
}

/// The inner-frame fields a sender supplies; `message_id`/`created_at` default.
#[derive(Debug, Clone)]
pub struct FrameFields {
    pub frame_type: String,
    pub sender: String,
    pub payload: Value,
    pub message_id: Option<String>,
    pub created_at: Option<String>,
}

// --- key generation ----------------------------------------------------------

/// Generate a long-lived X25519 transport keypair (the device's durable key).
pub fn generate_transport_keypair() -> KeyPairB64 {
    let secret = randombytes_buf(SESSION_KEY_BYTES);
    let mut secret_arr = [0u8; 32];
    secret_arr.copy_from_slice(&secret);
    let public = x25519_public_from_secret(&secret_arr);
    KeyPairB64 {
        public_key_b64: b64encode(&public),
        private_key_b64: b64encode(&secret_arr),
    }
}

/// Generate a long-lived Ed25519 identity keypair (verified-device mode). The
/// private value is the 32-byte seed, matching the reference's representation.
pub fn generate_identity_keypair() -> KeyPairB64 {
    let seed = randombytes_buf(32);
    let mut seed_arr = [0u8; 32];
    seed_arr.copy_from_slice(&seed);
    let (public, _secret) = crypto_sign_seed_keypair(&seed_arr);
    KeyPairB64 {
        public_key_b64: b64encode(&public),
        private_key_b64: b64encode(&seed_arr),
    }
}

/// A fresh random 32-byte session key (base64).
pub fn generate_session_key() -> String {
    b64encode(&randombytes_buf(SESSION_KEY_BYTES))
}

// --- identity / verified-device mode ----------------------------------------

/// Sign a transport public key with an identity private key (detached Ed25519).
pub fn sign_transport_key(
    identity_private_key_b64: &str,
    transport_public_key_b64: &str,
) -> Result<String> {
    let seed = fixed::<32>(
        &b64decode(identity_private_key_b64)?,
        "identity_private_key",
    )?;
    let message = b64decode(transport_public_key_b64)?;
    let (_public, secret) = crypto_sign_seed_keypair(&seed);
    let mut signature = [0u8; 64];
    crypto_sign_detached(&mut signature, &message, &secret)
        .map_err(|e| TransportError::Protocol(e.to_string()))?;
    Ok(b64encode(&signature))
}

/// Verify a transport-key signature against a pinned identity public key.
pub fn verify_transport_key_signature(
    identity_public_key_b64: &str,
    transport_public_key_b64: &str,
    signature_b64: &str,
) -> Result<()> {
    let public = fixed::<32>(&b64decode(identity_public_key_b64)?, "identity_public_key")?;
    let message = b64decode(transport_public_key_b64)?;
    let signature = fixed::<64>(&b64decode(signature_b64)?, "signature")?;
    crypto_sign_verify_detached(&signature, &message, &public)
        .map_err(|_| TransportError::Verification)
}

/// The SHA-256 fingerprint (hex) of an identity public key, for out-of-band
/// comparison during pairing.
pub fn fingerprint_identity_key(identity_public_key_b64: &str) -> Result<String> {
    let key = b64decode(identity_public_key_b64)?;
    Ok(hex(&Sha256::digest(&key)))
}

// --- session bootstrap -------------------------------------------------------

/// Wrap a session key to a device transport public key via sealed box (client side).
pub fn wrap_session_key(transport_public_key_b64: &str, session_key_b64: &str) -> Result<String> {
    let recipient = fixed::<32>(
        &b64decode(transport_public_key_b64)?,
        "transport_public_key",
    )?;
    let session_key = validate_session_key(session_key_b64)?;
    let mut sealed = vec![0u8; session_key.len() + SEAL_BYTES];
    crypto_box_seal(&mut sealed, &session_key, &recipient)
        .map_err(|e| TransportError::Protocol(e.to_string()))?;
    Ok(b64encode(&sealed))
}

/// Unwrap a `SessionInit` with the device transport private key (device side).
pub fn open_session_init(
    transport_private_key_b64: &str,
    session_init: &SessionInit,
) -> Result<OpenedSession> {
    let secret = fixed::<32>(
        &b64decode(transport_private_key_b64)?,
        "transport_private_key",
    )?;
    let public = x25519_public_from_secret(&secret);
    let wrapped = b64decode(&session_init.wrapped_session_key)?;
    if wrapped.len() < SEAL_BYTES {
        return Err(TransportError::InvalidInput(
            "wrapped_session_key too short".into(),
        ));
    }
    let mut session_key = vec![0u8; wrapped.len() - SEAL_BYTES];
    crypto_box_seal_open(&mut session_key, &wrapped, &public, &secret)
        .map_err(|_| TransportError::Decryption)?;
    if session_key.len() != SESSION_KEY_BYTES {
        return Err(TransportError::Protocol(
            "wrapped session key did not decrypt to 32 bytes".into(),
        ));
    }
    Ok(OpenedSession {
        session_key_b64: b64encode(&session_key),
        session_id: nonempty(&session_init.session_id, "session_id")?,
        device_id: nonempty(&session_init.device_id, "device_id")?,
    })
}

/// Build the encrypted `session_accept` (device side): proof the device unwrapped
/// the session key. The client verifies it with [`verify_session_accept`].
pub fn build_session_accept(
    session_key_b64: &str,
    session_id: &str,
    route_to: &str,
    nonce_b64: Option<&str>,
) -> Result<Envelope> {
    let plaintext = canonical_json(&json!({ "session_id": session_id }));
    secretbox_envelope(session_key_b64, session_id, route_to, &plaintext, nonce_b64)
}

/// Verify a `session_accept` (client side): decrypt and confirm the embedded
/// `session_id` matches what we expect.
pub fn verify_session_accept(
    session_key_b64: &str,
    envelope: &Envelope,
    expected_session_id: &str,
) -> Result<()> {
    let plaintext = secretbox_open_envelope(session_key_b64, envelope)?;
    let value: Value = serde_json::from_slice(&plaintext)
        .map_err(|_| TransportError::Protocol("session_accept was not valid JSON".into()))?;
    let got = value
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if got != expected_session_id || got != envelope.session_id {
        return Err(TransportError::Protocol(
            "session_accept session_id did not match".into(),
        ));
    }
    Ok(())
}

// --- message frames ----------------------------------------------------------

/// Encrypt an inner frame into an outer envelope.
pub fn encrypt_frame(
    session_key_b64: &str,
    outer: &OuterFields,
    frame: &FrameFields,
    nonce_b64: Option<&str>,
) -> Result<Envelope> {
    let session_id = nonempty(&outer.session_id, "session_id")?;
    let route_to = nonempty(&outer.route_to, "route_to")?;
    validate_enum(&frame.frame_type, &["data", "close"], "frame_type")?;
    validate_enum(&frame.sender, &["client", "device"], "sender")?;

    let inner = json!({
        "session_id": session_id,
        "message_id": frame.message_id.clone().unwrap_or_else(new_message_id),
        "frame_type": frame.frame_type,
        "sender": frame.sender,
        "created_at": frame.created_at.clone().unwrap_or_else(now_rfc3339),
        "payload": frame.payload,
    });
    let plaintext = canonical_json(&inner);
    secretbox_envelope(
        session_key_b64,
        &session_id,
        &route_to,
        &plaintext,
        nonce_b64,
    )
}

/// Decrypt an outer envelope to its validated inner frame.
pub fn decrypt_envelope(session_key_b64: &str, envelope: &Envelope) -> Result<Frame> {
    if envelope.version != PROTOCOL_VERSION {
        return Err(TransportError::Protocol(format!(
            "unsupported envelope version: {}",
            envelope.version
        )));
    }
    let plaintext = secretbox_open_envelope(session_key_b64, envelope)?;
    let value: Value = serde_json::from_slice(&plaintext)
        .map_err(|_| TransportError::Protocol("ciphertext was not valid JSON".into()))?;
    let frame: Frame = serde_json::from_value(value)
        .map_err(|e| TransportError::Protocol(format!("invalid frame: {e}")))?;

    if frame.session_id != envelope.session_id {
        return Err(TransportError::Protocol(
            "inner and outer session_id did not match".into(),
        ));
    }
    validate_enum(&frame.frame_type, &["data", "close"], "frame_type")?;
    validate_enum(&frame.sender, &["client", "device"], "sender")?;
    Ok(frame)
}

/// Build an authenticated `close` frame.
pub fn build_close_frame(
    session_key_b64: &str,
    session_id: &str,
    route_to: &str,
    sender: &str,
    payload: Value,
) -> Result<Envelope> {
    encrypt_frame(
        session_key_b64,
        &OuterFields {
            session_id: session_id.to_string(),
            route_to: route_to.to_string(),
        },
        &FrameFields {
            frame_type: "close".to_string(),
            sender: sender.to_string(),
            payload,
            message_id: None,
            created_at: None,
        },
        None,
    )
}

// --- internals ---------------------------------------------------------------

fn secretbox_envelope(
    session_key_b64: &str,
    session_id: &str,
    route_to: &str,
    plaintext: &str,
    nonce_b64: Option<&str>,
) -> Result<Envelope> {
    let key = validate_session_key(session_key_b64)?;
    let nonce = match nonce_b64 {
        Some(n) => fixed::<24>(&b64decode(n)?, "nonce")?,
        None => {
            let mut n = [0u8; 24];
            n.copy_from_slice(&randombytes_buf(NONCE_BYTES));
            n
        }
    };
    let mut ciphertext = vec![0u8; plaintext.len() + MAC_BYTES];
    crypto_secretbox_easy(&mut ciphertext, plaintext.as_bytes(), &nonce, &key)
        .map_err(|e| TransportError::Protocol(e.to_string()))?;
    Ok(Envelope {
        version: PROTOCOL_VERSION,
        session_id: session_id.to_string(),
        route_to: route_to.to_string(),
        nonce: b64encode(&nonce),
        ciphertext: b64encode(&ciphertext),
    })
}

fn secretbox_open_envelope(session_key_b64: &str, envelope: &Envelope) -> Result<Vec<u8>> {
    let key = validate_session_key(session_key_b64)?;
    let nonce = fixed::<24>(&b64decode(&envelope.nonce)?, "nonce")?;
    let ciphertext = b64decode(&envelope.ciphertext)?;
    if ciphertext.len() < MAC_BYTES {
        return Err(TransportError::InvalidInput("ciphertext too short".into()));
    }
    let mut plaintext = vec![0u8; ciphertext.len() - MAC_BYTES];
    crypto_secretbox_open_easy(&mut plaintext, &ciphertext, &nonce, &key)
        .map_err(|_| TransportError::Decryption)?;
    Ok(plaintext)
}

fn x25519_public_from_secret(secret: &[u8; 32]) -> [u8; 32] {
    let mut public = [0u8; 32];
    crypto_scalarmult_base(&mut public, secret);
    public
}

/// Canonical JSON: sorted keys, compact separators, UTF-8 — `serde_json`'s default
/// `Map` is a `BTreeMap`, so `to_string` already sorts and compacts.
fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("Value is always serializable")
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("0 nanoseconds is valid")
        .format(&Rfc3339)
        .expect("RFC3339 formatting cannot fail for a UTC instant")
}

fn b64encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

fn b64decode(value: &str) -> Result<Vec<u8>> {
    if value.is_empty() {
        return Err(TransportError::InvalidInput(
            "expected non-empty base64".into(),
        ));
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value.trim_end_matches('='))
        .map_err(|_| TransportError::InvalidInput("invalid base64".into()))
}

fn fixed<const N: usize>(bytes: &[u8], field: &str) -> Result<[u8; N]> {
    if bytes.len() != N {
        return Err(TransportError::InvalidInput(format!(
            "{field} must be {N} bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
}

fn validate_session_key(session_key_b64: &str) -> Result<[u8; 32]> {
    fixed::<32>(&b64decode(session_key_b64)?, "session_key")
}

fn nonempty(value: &str, field: &str) -> Result<String> {
    if value.is_empty() {
        return Err(TransportError::InvalidInput(format!(
            "{field} must be a non-empty string"
        )));
    }
    Ok(value.to_string())
}

fn validate_enum(value: &str, allowed: &[&str], field: &str) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(TransportError::Protocol(format!(
            "unsupported {field}: {value}"
        )))
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_keys_are_32_bytes() {
        let kp = generate_transport_keypair();
        assert_eq!(b64decode(&kp.public_key_b64).unwrap().len(), 32);
        assert_eq!(b64decode(&kp.private_key_b64).unwrap().len(), 32);
    }

    #[test]
    fn identity_private_is_the_32_byte_seed() {
        let kp = generate_identity_keypair();
        assert_eq!(b64decode(&kp.private_key_b64).unwrap().len(), 32);
        assert_eq!(b64decode(&kp.public_key_b64).unwrap().len(), 32);
    }

    #[test]
    fn sign_then_verify_roundtrips_and_rejects_tampering() {
        let identity = generate_identity_keypair();
        let transport = generate_transport_keypair();
        let sig = sign_transport_key(&identity.private_key_b64, &transport.public_key_b64).unwrap();

        verify_transport_key_signature(&identity.public_key_b64, &transport.public_key_b64, &sig)
            .expect("valid signature verifies");

        // A different transport key must not verify under the same signature.
        let other = generate_transport_keypair();
        assert_eq!(
            verify_transport_key_signature(&identity.public_key_b64, &other.public_key_b64, &sig),
            Err(TransportError::Verification)
        );
    }

    #[test]
    fn fingerprint_is_sha256_hex_known_answer() {
        // SHA-256 of 32 zero bytes is a well-known constant.
        let zero_pub = b64encode(&[0u8; 32]);
        assert_eq!(
            fingerprint_identity_key(&zero_pub).unwrap(),
            "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
        );
    }

    #[test]
    fn wrap_then_open_recovers_the_session_key() {
        let device = generate_transport_keypair();
        let session_key = generate_session_key();
        let wrapped = wrap_session_key(&device.public_key_b64, &session_key).unwrap();

        let opened = open_session_init(
            &device.private_key_b64,
            &SessionInit {
                session_id: "s1".into(),
                device_id: "d1".into(),
                wrapped_session_key: wrapped,
            },
        )
        .unwrap();
        assert_eq!(opened.session_key_b64, session_key);
        assert_eq!(opened.session_id, "s1");
    }

    #[test]
    fn session_accept_roundtrips() {
        let key = generate_session_key();
        let accept = build_session_accept(&key, "s1", "device:d1", None).unwrap();
        assert_eq!(accept.version, PROTOCOL_VERSION);
        verify_session_accept(&key, &accept, "s1").expect("accept verifies");
        assert!(verify_session_accept(&key, &accept, "wrong").is_err());
    }

    #[test]
    fn frame_encrypt_decrypt_roundtrip_preserves_payload() {
        let key = generate_session_key();
        let outer = OuterFields {
            session_id: "s1".into(),
            route_to: "device:d1".into(),
        };
        let frame = FrameFields {
            frame_type: "data".into(),
            sender: "client".into(),
            payload: json!({"hello": "world", "n": 7}),
            message_id: None,
            created_at: None,
        };
        let env = encrypt_frame(&key, &outer, &frame, None).unwrap();
        let got = decrypt_envelope(&key, &env).unwrap();
        assert_eq!(got.session_id, "s1");
        assert_eq!(got.frame_type, "data");
        assert_eq!(got.sender, "client");
        assert_eq!(got.payload, json!({"hello": "world", "n": 7}));
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key = generate_session_key();
        let other = generate_session_key();
        let env = encrypt_frame(
            &key,
            &OuterFields {
                session_id: "s1".into(),
                route_to: "d1".into(),
            },
            &FrameFields {
                frame_type: "data".into(),
                sender: "device".into(),
                payload: json!("hi"),
                message_id: None,
                created_at: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            decrypt_envelope(&other, &env),
            Err(TransportError::Decryption)
        );
    }

    #[test]
    fn explicit_nonce_is_deterministic() {
        let key = generate_session_key();
        let nonce = b64encode(&[7u8; 24]);
        let mk = || {
            encrypt_frame(
                &key,
                &OuterFields {
                    session_id: "s1".into(),
                    route_to: "d1".into(),
                },
                &FrameFields {
                    frame_type: "data".into(),
                    sender: "client".into(),
                    payload: json!("same"),
                    message_id: Some("fixed-id".into()),
                    created_at: Some("2026-06-11T00:00:00Z".into()),
                },
                Some(&nonce),
            )
            .unwrap()
        };
        assert_eq!(
            mk().ciphertext,
            mk().ciphertext,
            "same inputs → same ciphertext"
        );
    }

    #[test]
    fn bad_version_is_rejected() {
        let key = generate_session_key();
        let mut env = build_session_accept(&key, "s1", "d1", None).unwrap();
        env.version = 99;
        assert!(matches!(
            decrypt_envelope(&key, &env),
            Err(TransportError::Protocol(_))
        ));
    }
}
