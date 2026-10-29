//! Bounded reads share strict limits and authenticated, scope-bound cursors.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) const MAX_RESPONSE_BYTES: usize = 1_048_576;
pub(super) const MAX_ROW_BYTES: usize = 65_536;
pub(super) const RESPONSE_OVERHEAD: usize = 16_384;

pub(super) struct BoundedPage<'a> {
    pub limit: usize,
    key: &'a str,
    project: &'a str,
    binding: Value,
}

impl<'a> BoundedPage<'a> {
    pub fn parse(
        params: &Value,
        key: &'a str,
        project: &'a str,
        binding: Value,
    ) -> Result<(Self, Option<String>), String> {
        let limit = match params.get("limit") {
            None => 100,
            Some(value) => value
                .as_u64()
                .filter(|limit| (1..=500).contains(limit))
                .ok_or("limit must be 1 to 500")? as usize,
        };
        let page = Self {
            limit,
            key,
            project,
            binding,
        };
        let position = params
            .get("cursor")
            .map(|cursor| page.position(cursor))
            .transpose()?;
        Ok((page, position))
    }
    pub fn cursor(&self, position: &str) -> String {
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!([position, self.signature(position)]))
                .expect("cursor serializes"),
        )
    }

    fn position(&self, cursor: &Value) -> Result<String, String> {
        let unreadable = "invalid cursor: restart this list with its original filters";
        let raw = cursor
            .as_str()
            .filter(|cursor| !cursor.is_empty() && cursor.len() <= 2048)
            .and_then(|cursor| URL_SAFE_NO_PAD.decode(cursor).ok())
            .ok_or(unreadable)?;
        let parts: Vec<String> = serde_json::from_slice(&raw).map_err(|_| unreadable)?;
        if parts.len() != 2 || parts[0].len() > 1024 || parts[1] != self.signature(&parts[0]) {
            return Err(unreadable.into());
        }
        Ok(parts[0].clone())
    }

    fn signature(&self, position: &str) -> String {
        // HMAC-SHA256. The persisted per-store cursor secret never crosses the wire.
        let hashed_key = Sha256::digest(self.key.as_bytes());
        let mut inner_pad = [0x36_u8; 64];
        let mut outer_pad = [0x5c_u8; 64];
        for (index, byte) in hashed_key.iter().enumerate() {
            inner_pad[index] ^= byte;
            outer_pad[index] ^= byte;
        }
        let payload = json!(["bounded-tasks-v1", self.project, self.binding, position]);
        let mut inner = Sha256::new();
        inner.update(inner_pad);
        inner.update(payload.to_string().as_bytes());
        let mut outer = Sha256::new();
        outer.update(outer_pad);
        outer.update(inner.finalize());
        URL_SAFE_NO_PAD.encode(outer.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_defaults_limits_and_invalid_limits() {
        assert_eq!(
            BoundedPage::parse(&json!({}), "store", "project", json!([]))
                .unwrap()
                .0
                .limit,
            100
        );
        assert_eq!(
            BoundedPage::parse(&json!({"limit":500}), "store", "project", json!([]))
                .unwrap()
                .0
                .limit,
            500
        );
        for limit in [
            json!(0),
            json!(501),
            json!(-1),
            json!(1.5),
            json!("5"),
            json!(null),
        ] {
            assert!(
                BoundedPage::parse(&json!({"limit":limit}), "store", "project", json!([])).is_err(),
                "{limit}"
            );
        }
    }
    #[test]
    fn bounded_cursors_are_scoped_and_authenticated() {
        let (page, _) =
            BoundedPage::parse(&json!({}), "store", "project", json!(["summary", ["api"]]))
                .unwrap();
        let cursor = page.cursor("125");
        let (_, position) = BoundedPage::parse(
            &json!({"cursor":cursor}),
            "store",
            "project",
            json!(["summary", ["api"]]),
        )
        .unwrap();
        assert_eq!(position.as_deref(), Some("125"));
        for (key, project, binding) in [
            ("other", "project", json!(["summary", ["api"]])),
            ("store", "other", json!(["summary", ["api"]])),
            ("store", "project", json!(["summary", ["ui"]])),
            ("store", "project", json!(["choices", "label"])),
        ] {
            assert!(BoundedPage::parse(&json!({"cursor":cursor}), key, project, binding).is_err());
        }
        let raw = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value[0] = json!("124");
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap());
        assert!(BoundedPage::parse(
            &json!({"cursor":forged}),
            "store",
            "project",
            json!(["summary", ["api"]])
        )
        .is_err());
        for cursor in ["".into(), "invalid".into(), "x".repeat(2049)] {
            assert!(
                BoundedPage::parse(&json!({"cursor":cursor}), "store", "project", json!([]))
                    .is_err()
            );
        }
    }
}
