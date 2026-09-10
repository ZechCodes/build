//! Device pairing — registering this bridge to a user account, then waiting for the
//! human to approve it.
//!
//! Flow (device-initiated, human-approved):
//! 1. The bridge generates a short, high-entropy **pairing code** and registers as
//!    *pending* with the api: it sends both public keys + a hash of the code +
//!    an Ed25519 signature over a challenge binding all of them (proof it holds the
//!    identity private key). The raw code is **never** sent — only its hash.
//! 2. The bridge prints the code + its key fingerprint and polls for approval.
//! 3. The user enters the code in the web app, compares the fingerprint out-of-band,
//!    and approves — which binds the device to their account.
//! 4. The bridge sees `approved`, persists it, and connects to the relay.
//!
//! Pure request/response *shaping* lives in free functions (unit-tested without a
//! network); the HTTP calls are thin wrappers around an injected [`reqwest::Client`]
//! so they can be pointed at a mock server in tests.
//!
//! TLS: reqwest is built with its `rustls` feature (see `Cargo.toml`), so
//! `https://getbuild.ing` pairing calls use rustls with the platform trust store
//! via `rustls-platform-verifier` — never native-tls/OpenSSL. (reqwest 0.13
//! dropped the `rustls-tls-webpki-roots` option; the platform verifier is its
//! rustls root story.)

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::{self, StoredIdentity};
use crate::transport;

/// Characters used in pairing codes — unambiguous (no 0/O, 1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Pairing code length (excluding the separating dash); ~8 chars ≈ 40 bits.
const CODE_LEN: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("http error: {0}")]
    Http(String),
    #[error("api rejected registration: {0}")]
    Rejected(String),
    #[error("identity error: {0}")]
    Identity(String),
}

type Result<T> = std::result::Result<T, PairingError>;

/// The registration payload POSTed to `/api/devices/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub device_id: String,
    pub name: String,
    pub identity_public_key_b64: String,
    pub transport_public_key_b64: String,
    /// SHA-256 hex of the pairing code — never the raw code.
    pub pairing_code_hash: String,
    /// Ed25519 signature (padded b64) over [`registration_challenge`], proving
    /// possession of the identity private key.
    pub signature_b64: String,
}

/// The approval-status response from `/api/devices/{id}/status`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatusResponse {
    pub approved: bool,
    #[serde(default)]
    pub owner_user_id: Option<String>,
}

// --- pure shaping -------------------------------------------------------------

/// Generate a high-entropy pairing code like `WXYZ-4F2K`, drawing from an
/// unambiguous alphabet. Uses the transport RNG.
pub fn generate_pairing_code() -> String {
    let bytes = transport::random_bytes(CODE_LEN);
    let mut code = String::with_capacity(CODE_LEN + 1);
    for (i, b) in bytes.iter().enumerate() {
        if i == CODE_LEN / 2 {
            code.push('-');
        }
        code.push(CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char);
    }
    code
}

/// SHA-256 hex of the pairing code. The api stores this and compares the hash of the
/// code the human types, so the raw code never leaves the operator's hands until then.
pub fn hash_pairing_code(code: &str) -> String {
    hex(&Sha256::digest(code.as_bytes()))
}

/// The canonical message signed during registration. Binds the device id, both public
/// keys, and the code hash so a captured signature cannot be replayed onto a different
/// registration. Pure string builder.
pub fn registration_challenge(
    device_id: &str,
    identity_public_key_b64: &str,
    transport_public_key_b64: &str,
    pairing_code_hash: &str,
) -> String {
    format!(
        "register.{device_id}.{identity_public_key_b64}.{transport_public_key_b64}.{pairing_code_hash}"
    )
}

/// Build a fully-signed [`RegisterRequest`] from a stored identity and a raw pairing
/// code. Signs [`registration_challenge`] with the identity private key. Does not
/// mutate its inputs.
pub fn build_register_request(
    identity: &StoredIdentity,
    pairing_code: &str,
) -> Result<RegisterRequest> {
    let pairing_code_hash = hash_pairing_code(pairing_code);
    let challenge = registration_challenge(
        &identity.device_id,
        &identity.identity_public_key_b64,
        &identity.transport.public_key_b64,
        &pairing_code_hash,
    );
    let signature_b64 =
        transport::sign_message_b64(&identity.identity_private_key_b64, challenge.as_bytes())
            .map_err(|e| PairingError::Identity(e.to_string()))?;
    Ok(RegisterRequest {
        device_id: identity.device_id.clone(),
        name: identity.name.clone(),
        identity_public_key_b64: identity.identity_public_key_b64.clone(),
        transport_public_key_b64: identity.transport.public_key_b64.clone(),
        pairing_code_hash,
        signature_b64,
    })
}

// --- I/O ----------------------------------------------------------------------

/// POST `{api}/api/devices/register`. 2xx → Ok; any other status → Rejected.
pub async fn register(
    client: &reqwest::Client,
    api_url: &str,
    req: &RegisterRequest,
) -> Result<()> {
    let url = format!("{}/api/devices/register", api_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(req)
        .send()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(PairingError::Rejected(format!("{status}: {body}")))
    }
}

/// GET `{api}/api/devices/{device_id}/status` once.
pub async fn fetch_status(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
) -> Result<StatusResponse> {
    let url = format!(
        "{}/api/devices/{device_id}/status",
        api_url.trim_end_matches('/')
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PairingError::Rejected(resp.status().to_string()));
    }
    resp.json::<StatusResponse>()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))
}

/// Poll `fetch_status` every `interval` until the device is approved; returns the
/// owner user id once it is.
pub async fn poll_until_approved(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
    interval: Duration,
) -> Result<String> {
    loop {
        let status = fetch_status(client, api_url, device_id).await?;
        if status.approved {
            return Ok(status.owner_user_id.unwrap_or_default());
        }
        tokio::time::sleep(interval).await;
    }
}

/// Ensure the device is registered and approved. If `stored.approved`, returns it
/// unchanged with no network calls. Otherwise: register as pending, print the pairing
/// code + fingerprint + approve URL, poll until approved, persist `approved = true`,
/// and return the updated identity. Takes ownership and returns the approved copy.
///
/// `pairing_code_override` (env `BRIDGE_PAIRING_CODE`) replaces the random code so
/// dev/compose automation can complete the real approve flow with a known code;
/// leave it `None` (the default everywhere humans pair) for a fresh random code.
pub async fn ensure_paired(
    client: &reqwest::Client,
    api_url: &str,
    web_url: &str,
    identity_path: &Path,
    mut stored: StoredIdentity,
    poll_interval: Duration,
    pairing_code_override: Option<&str>,
) -> Result<StoredIdentity> {
    if stored.approved {
        return Ok(stored);
    }

    let pairing_code = match pairing_code_override {
        Some(code) => code.to_string(),
        None => generate_pairing_code(),
    };
    let req = build_register_request(&stored, &pairing_code)?;
    register(client, api_url, &req).await?;

    let fingerprint = transport::fingerprint_identity_key(&stored.identity_public_key_b64)
        .map_err(|e| PairingError::Identity(e.to_string()))?;
    eprintln!("\n  Pair this device to your account:");
    eprintln!("    pairing code: {pairing_code}");
    eprintln!("    fingerprint:  {fingerprint}");
    eprintln!(
        "    approve at:   {}/app/  →  Settings → Devices → Add a device\n",
        web_url.trim_end_matches('/')
    );

    let owner = poll_until_approved(client, api_url, &stored.device_id, poll_interval).await?;
    stored.approved = true;
    identity::save(identity_path, &stored).map_err(|e| PairingError::Identity(e.to_string()))?;
    // What happens next is the caller's business — `serve` connects to the
    // relay, `pair` exits — so pairing reports only the pairing it did.
    eprintln!("  Device approved (owner {owner})");
    Ok(stored)
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
    use crate::identity;

    #[test]
    fn pairing_code_is_high_entropy_and_distinct() {
        let a = generate_pairing_code();
        let b = generate_pairing_code();
        assert_ne!(a, b, "two codes should differ");
        assert_eq!(a.len(), CODE_LEN + 1, "code has a dash separator");
        assert!(a.contains('-'));
        // Every non-dash char is from the unambiguous alphabet.
        assert!(a
            .chars()
            .filter(|c| *c != '-')
            .all(|c| CODE_ALPHABET.contains(&(c as u8))));
    }

    #[test]
    fn hash_pairing_code_is_sha256_hex_known_answer() {
        // SHA-256 of "abc" is a well-known constant.
        assert_eq!(
            hash_pairing_code("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn registration_challenge_binds_all_fields() {
        let base = registration_challenge("d", "idpub", "tppub", "hash");
        assert_ne!(base, registration_challenge("D", "idpub", "tppub", "hash"));
        assert_ne!(base, registration_challenge("d", "OTHER", "tppub", "hash"));
        assert_ne!(base, registration_challenge("d", "idpub", "OTHER", "hash"));
        assert_ne!(base, registration_challenge("d", "idpub", "tppub", "OTHER"));
    }

    #[test]
    fn build_register_request_signature_verifies() {
        let id = identity::generate("my-box");
        let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
        let challenge = registration_challenge(
            &req.device_id,
            &req.identity_public_key_b64,
            &req.transport_public_key_b64,
            &req.pairing_code_hash,
        );
        transport::verify_message_b64(
            &req.identity_public_key_b64,
            challenge.as_bytes(),
            &req.signature_b64,
        )
        .expect("registration signature verifies against the device identity key");
    }

    #[test]
    fn build_register_request_tampered_code_hash_fails_verification() {
        let id = identity::generate("my-box");
        let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
        // Verify against a challenge built with a different code hash → must fail.
        let forged = registration_challenge(
            &req.device_id,
            &req.identity_public_key_b64,
            &req.transport_public_key_b64,
            "tampered-hash",
        );
        assert!(transport::verify_message_b64(
            &req.identity_public_key_b64,
            forged.as_bytes(),
            &req.signature_b64,
        )
        .is_err());
    }

    #[test]
    fn build_register_request_never_includes_raw_code() {
        let id = identity::generate("my-box");
        let code = "WXYZ-4F2K";
        let req = build_register_request(&id, code).unwrap();
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains(code),
            "raw pairing code must not be serialized"
        );
        assert!(json.contains(&hash_pairing_code(code)));
    }
}
