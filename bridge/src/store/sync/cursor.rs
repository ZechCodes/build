//! Versioned authenticated resume tokens, independent of process epochs.
use super::{Scope, Sequence, SyncError};
use aws_lc_rs::hmac;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

const MAX_CURSOR_BYTES: usize = 2048;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    purpose: String,
    incarnation: String,
    scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<String>,
    sequence: Sequence,
}

pub(super) enum Position {
    Retained(Sequence),
    Rebound(Sequence),
    Foreign,
}

pub(super) fn encode(incarnation: &str, key: &[u8], scope: &Scope, sequence: Sequence) -> String {
    let payload = Payload {
        version: 1,
        purpose: "resume".into(),
        incarnation: incarnation.into(),
        scope: scope.digest().into(),
        binding: scope.path_binding(key),
        sequence,
    };
    let bytes = serde_json::to_vec(&payload).expect("cursor fields serialize");
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let signature = hmac::sign(&key, &bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

pub(super) fn decode(
    token: &str,
    incarnation: &str,
    key: &[u8],
    scope: &Scope,
) -> Result<Position, SyncError> {
    if token.len() > MAX_CURSOR_BYTES {
        return Err(SyncError::MalformedCursor);
    }
    let (payload, signature) = token
        .split_once('.')
        .filter(|_| token.len() <= MAX_CURSOR_BYTES)
        .ok_or(SyncError::MalformedCursor)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| SyncError::MalformedCursor)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| SyncError::MalformedCursor)?;
    let payload: Payload =
        serde_json::from_slice(&bytes).map_err(|_| SyncError::MalformedCursor)?;
    validate_structure(&payload, &signature)?;
    validate_binding_shape(&payload.binding, scope)?;
    // A foreign store has a different secret. Recognizing its well-formed
    // incarnation can only request a reset; it never grants cursor progress.
    if payload.incarnation != incarnation {
        validate_scope(&payload.scope, scope)?;
        return Ok(Position::Foreign);
    }
    let signing_key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::verify(&signing_key, &bytes, &signature).map_err(|_| SyncError::MalformedCursor)?;
    validate_scope(&payload.scope, scope)?;
    if let Some(expected) = scope.path_binding(key) {
        if payload.binding.as_deref() != Some(expected.as_str()) {
            return Ok(Position::Rebound(payload.sequence));
        }
    }
    Ok(Position::Retained(payload.sequence))
}

fn validate_binding_shape(binding: &Option<String>, scope: &Scope) -> Result<(), SyncError> {
    if binding.is_some() != scope.path_binding_required()
        || binding
            .as_ref()
            .is_some_and(|binding| binding.len() != 43 || URL_SAFE_NO_PAD.decode(binding).is_err())
    {
        return Err(SyncError::MalformedCursor);
    }
    Ok(())
}

fn validate_scope(digest: &str, scope: &Scope) -> Result<(), SyncError> {
    if digest != scope.digest() {
        return Err(SyncError::ScopeMismatch);
    }
    Ok(())
}

fn validate_structure(payload: &Payload, signature: &[u8]) -> Result<(), SyncError> {
    let canonical_uuid = uuid::Uuid::parse_str(&payload.incarnation)
        .ok()
        .map(|id| id.to_string());
    if payload.version != 1
        || payload.purpose != "resume"
        || signature.len() != 32
        || canonical_uuid.as_deref() != Some(&payload.incarnation)
        || payload.scope.len() != 64
        || !payload
            .scope
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(SyncError::MalformedCursor);
    }
    Ok(())
}
