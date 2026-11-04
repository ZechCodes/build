//! Purpose-separated, authenticated finite recovery positions.
use super::{schema, Scope, SyncError};
use crate::store::ReadTransaction;
use aws_lc_rs::hmac;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload<T> {
    version: u8,
    purpose: String,
    incarnation: String,
    scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<String>,
    data: T,
}

pub enum Continuation<T> {
    Current(T),
    Rebound(T),
    Foreign(T),
}

impl ReadTransaction<'_> {
    pub fn sync_encode_continuation<T: Serialize>(
        &self,
        scope: &Scope,
        purpose: &str,
        data: T,
    ) -> Result<String, SyncError> {
        super::device::ensure_scope_ready(self.connection(),scope)?;
        let (incarnation, key) = schema::identity(self.connection())?;
        let payload = Payload {
            version: 1,
            purpose: purpose.into(),
            incarnation,
            scope: scope.digest().into(),
            binding: scope.path_binding(&key),
            data,
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| SyncError::MalformedCursor)?;
        let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &key), &bytes);
        let token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(bytes),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        );
        if token.len() > token_bound(purpose) {
            return Err(SyncError::MalformedCursor);
        }
        Ok(token)
    }
    pub fn sync_decode_continuation<T: DeserializeOwned>(
        &self,
        scope: &Scope,
        purpose: &str,
        token: &str,
    ) -> Result<Continuation<T>, SyncError> {
        super::device::ensure_scope_ready(self.connection(),scope)?;
        if token.len() > token_bound(purpose) {
            return Err(SyncError::MalformedCursor);
        }
        let (raw, tag) = token.split_once('.').ok_or(SyncError::MalformedCursor)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| SyncError::MalformedCursor)?;
        let tag = URL_SAFE_NO_PAD
            .decode(tag)
            .map_err(|_| SyncError::MalformedCursor)?;
        let payload: Payload<T> =
            serde_json::from_slice(&bytes).map_err(|_| SyncError::MalformedCursor)?;
        validate(&payload, purpose, &tag)?;
        super::device::ensure_scope_ready(self.connection(),scope)?;
        let (incarnation, key) = schema::identity(self.connection())?;
        validate_binding_shape(&payload.binding, scope)?;
        if incarnation == payload.incarnation {
            hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, &key), &bytes, &tag)
                .map_err(|_| SyncError::MalformedCursor)?;
        }
        if payload.scope != scope.digest() {
            return Err(SyncError::ScopeMismatch);
        }
        if incarnation != payload.incarnation {
            return Ok(Continuation::Foreign(payload.data));
        }
        if let Some(expected) = scope.path_binding(&key) {
            if payload.binding.as_deref() != Some(expected.as_str()) {
                return Ok(Continuation::Rebound(payload.data));
            }
        }
        Ok(Continuation::Current(payload.data))
    }
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

fn validate<T>(payload: &Payload<T>, purpose: &str, tag: &[u8]) -> Result<(), SyncError> {
    let canonical = uuid::Uuid::parse_str(&payload.incarnation)
        .ok()
        .map(|id| id.to_string());
    if payload.version != 1
        || payload.purpose != purpose
        || canonical.as_deref() != Some(payload.incarnation.as_str())
        || payload.scope.len() != 64
        || !payload
            .scope
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || tag.len() != 32
    {
        return Err(SyncError::MalformedCursor);
    }
    Ok(())
}

fn token_bound(purpose:&str)->usize {if purpose=="device_snapshot" {65536} else {2048}}
