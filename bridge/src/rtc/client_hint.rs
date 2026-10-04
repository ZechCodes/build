//! LAN discovery is keyed by a client-held bearer hint, not an authenticated
//! client identity. The SPA's UUIDv4 has 122 random bits and travels only in
//! encrypted offers. A holder can query the remembered validated address or
//! replace it through fresh validated discovery, never supply an address or
//! bypass reply validation. Neither hints nor cached addresses are logged.

use serde_json::Value;
use uuid::Uuid;

/// A resolver may learn its hint after the first greeting, but cannot change it.
/// The session Opening also binds the first hint for the entire E2EE lifetime,
/// so closing and recreating a peer cannot rotate hints to fill the cache.
#[derive(Default)]
pub(super) struct Binding(std::sync::OnceLock<Uuid>);

impl Binding {
    pub(super) fn new(hint: Option<Uuid>) -> Self {
        let binding = Self::default();
        if let Some(hint) = hint {
            binding.bind(hint);
        }
        binding
    }
    pub(super) fn bind(&self, hint: Uuid) {
        let _ = self.0.set(hint);
    }
    pub(super) fn get(&self) -> Option<Uuid> {
        self.0.get().copied()
    }
}

pub(crate) fn from_params(params: &Value) -> Option<Uuid> {
    let supplied = params.get("client_id")?.as_str()?;
    if supplied.len() != 36 {
        return None;
    }
    let parsed = Uuid::parse_str(supplied).ok()?;
    (parsed.hyphenated().to_string() == supplied).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_post_greeting_hint_can_bind_once_without_rekeying_an_existing_peer() {
        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        let binding = Binding::new(None);
        assert_eq!(binding.get(), None);
        binding.bind(first);
        assert_eq!(binding.get(), Some(first));
        binding.bind(next);
        assert_eq!(binding.get(), Some(first));
        assert_eq!(Binding::new(Some(next)).get(), Some(next));
    }

    #[test]
    fn a_hint_requires_the_exact_uuid_shape_and_invalid_hints_are_absent() {
        let canonical = "4c14a372-9db5-4faa-bbf1-d93583114e89";
        assert_eq!(
            from_params(&json!({"client_id": canonical})),
            Uuid::parse_str(canonical).ok()
        );
        for value in [
            Value::Null,
            json!(12),
            json!({}),
            json!([]),
            json!("4c14a3729db54faabbf1d93583114e89"),
            json!("{4c14a372-9db5-4faa-bbf1-d93583114e89}"),
            json!("urn:uuid:4c14a372-9db5-4faa-bbf1-d93583114e89"),
            json!(" 4c14a372-9db5-4faa-bbf1-d93583114e89"),
            json!("4c14a372-9db5-4faa-bbf1-d93583114e89x"),
            json!("4c14a372-9db5-4faa-bbf1-d93583114e8z"),
        ] {
            assert_eq!(from_params(&json!({"client_id": value})), None);
        }
        assert_eq!(from_params(&json!({})), None);
    }
}
