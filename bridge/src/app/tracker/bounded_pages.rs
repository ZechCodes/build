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
    pub fn parse(params: &Value, key: &'a str, project: &'a str, binding: Value) -> Result<(Self, Option<String>), String> {
        let _ = (params, key, project, binding);
        Err("not implemented".into())
    }
    pub fn cursor(&self, position: &str) -> String {
        let _ = (self, position);
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_defaults_limits_and_invalid_limits() {
        assert_eq!(BoundedPage::parse(&json!({}), "store", "project", json!([])).unwrap().0.limit,100);
        assert_eq!(BoundedPage::parse(&json!({"limit":500}), "store", "project", json!([])).unwrap().0.limit,500);
        for limit in [json!(0),json!(501),json!(-1),json!(1.5),json!("5"),json!(null)] {
            assert!(BoundedPage::parse(&json!({"limit":limit}), "store", "project", json!([])).is_err(),"{limit}");
        }
    }
    #[test]
    fn bounded_cursors_are_scoped_and_authenticated() {
        let (page, _) = BoundedPage::parse(&json!({}),"store","project",json!(["summary",["api"]])).unwrap();
        let cursor = page.cursor("125");
        let (_, position) = BoundedPage::parse(&json!({"cursor":cursor}),"store","project",json!(["summary",["api"]])).unwrap();
        assert_eq!(position.as_deref(), Some("125"));
        for (key,project,binding) in [("other","project",json!(["summary",["api"]])),("store","other",json!(["summary",["api"]])),("store","project",json!(["summary",["ui"]])),("store","project",json!(["choices","label"]))] {
            assert!(BoundedPage::parse(&json!({"cursor":cursor}),key,project,binding).is_err());
        }
        let raw = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value[0] = json!("124");
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap());
        assert!(BoundedPage::parse(&json!({"cursor":forged}),"store","project",json!(["summary",["api"]])).is_err());
        for cursor in ["".into(),"invalid".into(),"x".repeat(2049)] {
            assert!(BoundedPage::parse(&json!({"cursor":cursor}),"store","project",json!([])).is_err());
        }
    }
}
