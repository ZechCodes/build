//! Sealed upload opening is the only browser-upload consumer of the device key.
//! Values and keys have no Debug or Serialize implementation.
use super::{GrantScope, Result, SecretContext, SecretError, SecretValue, StageContext};
use base64::{engine::general_purpose::STANDARD_NO_PAD as BASE64, Engine};
use dryoc::classic::crypto_box::crypto_box_seal_open;
use dryoc::classic::crypto_core::crypto_scalarmult_base;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

pub const PADDED_BYTES: usize = 32 * 1024;
pub const MAX_CREATE_REQUEST_BYTES: usize = 64 * 1024;
const SEAL_BYTES: usize = 48;
const ENCODED_BYTES: usize = (PADDED_BYTES + SEAL_BYTES).div_ceil(3) * 4 - 1;
const DOMAIN: &str = "build.secrets.upload";
const VERSION: u32 = 1;

/// Metadata authenticated inside the seal and compared with authorized input.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadContext {
    pub device_id: String,
    pub creator: String,
    pub request_id: String,
    pub name: String,
    pub ttl_seconds: Option<i64>,
    pub grant: GrantScope,
    pub stage: StageContext,
}

impl UploadContext {
    pub fn validate(&self) -> Result<()> {
        let uuid = uuid::Uuid::parse_str(&self.request_id).map_err(|_| SecretError::Invalid)?;
        if uuid.hyphenated().to_string() != self.request_id || !valid_name(&self.name) {
            return Err(SecretError::Invalid);
        }
        if self
            .ttl_seconds
            .is_some_and(|ttl| !(60..=2_592_000).contains(&ttl))
        {
            return Err(SecretError::Invalid);
        }
        self.secret_context(format!("sec_{}", uuid::Uuid::nil()))
            .validate()
    }

    pub fn secret_context(&self, id: String) -> SecretContext {
        SecretContext {
            id,
            device_id: self.device_id.clone(),
            creator: self.creator.clone(),
            name: self.name.clone(),
            ttl_seconds: self.ttl_seconds,
            grant: self.grant.clone(),
            stage: self.stage.clone(),
        }
    }
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(byte))
}

/// Injected at startup; application state never receives raw transport keys.
pub trait UploadOpener: Send + Sync {
    fn open(&self, sealed_payload: &str, expected: &UploadContext) -> Result<SecretValue>;
}

pub struct SealedUploadOpener {
    private: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl SealedUploadOpener {
    pub fn new(private_key_b64: &str) -> Result<Self> {
        let decoded = Zeroizing::new(
            BASE64
                .decode(private_key_b64)
                .map_err(|_| SecretError::Invalid)?,
        );
        let mut private = Zeroizing::new([0; 32]);
        if decoded.len() != private.len() {
            return Err(SecretError::Invalid);
        }
        private.copy_from_slice(&decoded);
        let mut public = [0; 32];
        crypto_scalarmult_base(&mut public, &private);
        Ok(Self { private, public })
    }
}

impl UploadOpener for SealedUploadOpener {
    fn open(&self, sealed_payload: &str, expected: &UploadContext) -> Result<SecretValue> {
        expected.validate()?;
        if sealed_payload.len() != ENCODED_BYTES {
            return Err(SecretError::Invalid);
        }
        let ciphertext = BASE64
            .decode(sealed_payload)
            .map_err(|_| SecretError::Invalid)?;
        if ciphertext.len() != PADDED_BYTES + SEAL_BYTES {
            return Err(SecretError::Invalid);
        }
        let mut plaintext = Zeroizing::new(vec![0; PADDED_BYTES]);
        crypto_box_seal_open(&mut plaintext, &ciphertext, &self.public, &self.private)
            .map_err(|_| SecretError::Invalid)?;
        open_padded(&plaintext, expected)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadEnvelope {
    domain: String,
    version: u32,
    context: UploadContext,
    value_b64: UploadValue,
}

#[derive(Deserialize)]
#[serde(transparent)]
struct UploadValue(String);

impl Drop for UploadValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn open_padded(plaintext: &[u8], expected: &UploadContext) -> Result<SecretValue> {
    let encoded_len = u32::from_be_bytes(plaintext[..4].try_into().unwrap()) as usize;
    if encoded_len == 0 || encoded_len > PADDED_BYTES - 4 {
        return Err(SecretError::Invalid);
    }
    let end = 4 + encoded_len;
    if plaintext[end..].iter().any(|byte| *byte != 0) {
        return Err(SecretError::Invalid);
    }
    let envelope: UploadEnvelope =
        serde_json::from_slice(&plaintext[4..end]).map_err(|_| SecretError::Invalid)?;
    if envelope.domain != DOMAIN || envelope.version != VERSION || &envelope.context != expected {
        return Err(SecretError::Invalid);
    }
    if envelope.value_b64.0.len() > (super::MAX_VALUE_BYTES * 4).div_ceil(3) {
        return Err(SecretError::Invalid);
    }
    let decoded = Zeroizing::new(
        BASE64
            .decode(&envelope.value_b64.0)
            .map_err(|_| SecretError::Invalid)?,
    );
    if decoded.is_empty() || std::str::from_utf8(&decoded).is_err() {
        return Err(SecretError::Invalid);
    }
    SecretValue::new(decoded.to_vec())
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
