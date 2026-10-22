//! Patch accepted attention rows while retaining every unrelated payload.
//! Conversation and agent payloads are deliberately outside this walk.
use super::{Kind, SharedJson};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub(in crate::app) struct AttentionPatch<'a> {
    owner: &'a str,
    attention: Value,
    resume: Value,
    memo: HashMap<(bool, usize), (SharedJson, Option<SharedJson>)>,
}

impl<'a> AttentionPatch<'a> {
    pub(in crate::app) fn new(owner: &'a str, attention: Value) -> Self {
        Self {
            owner,
            resume: attention["resume_at"].clone(),
            attention,
            memo: HashMap::new(),
        }
    }

    pub(in crate::app) fn apply(&mut self, value: &SharedJson) -> Option<SharedJson> {
        let identity = match &value.0.kind {
            Kind::Leaf(leaf) => (true, Arc::as_ptr(leaf) as usize),
            _ => (false, Arc::as_ptr(&value.0) as usize),
        };
        if let Some((_, patched)) = self.memo.get(&identity) {
            return patched.clone();
        }
        let patched = match &value.0.kind {
            Kind::Leaf(leaf) => self.patch_value(leaf).map(SharedJson::value),
            Kind::Array(values) => self.patch_shared_array(values),
            Kind::Object(fields) => self.patch_shared_object(fields),
        };
        self.memo.insert(identity, (value.clone(), patched.clone()));
        patched
    }

    fn patch_shared_array(&mut self, values: &[SharedJson]) -> Option<SharedJson> {
        let mut next = None;
        for (index, child) in values.iter().enumerate() {
            if let Some(patched) = self.apply(child) {
                next.get_or_insert_with(|| values.to_vec())[index] = patched;
            }
        }
        next.map(SharedJson::array)
    }

    fn patch_shared_object(&mut self, fields: &BTreeMap<String, SharedJson>) -> Option<SharedJson> {
        let mut updates = BTreeMap::new();
        if row_owner(|key| fields.get(key)?.leaf()?.as_str()) == Some(self.owner) {
            for (key, replacement) in [("attention", &self.attention), ("resume_at", &self.resume)]
            {
                if fields
                    .get(key)
                    .is_some_and(|value| !value.equals_value(replacement))
                {
                    updates.insert(key.into(), SharedJson::value(replacement.clone()));
                }
            }
        }
        for key in ROW_CONTAINERS {
            if let Some(value) = fields.get(*key).and_then(|value| self.apply(value)) {
                updates.insert((*key).into(), value);
            }
        }
        (!updates.is_empty()).then(|| {
            let mut next = fields.clone();
            next.extend(updates);
            SharedJson::object(next)
        })
    }

    fn patch_value(&self, value: &Value) -> Option<Value> {
        match value {
            Value::Array(values) => self.patch_array(values),
            Value::Object(fields) => self.patch_object(fields),
            _ => None,
        }
    }

    fn patch_array(&self, values: &[Value]) -> Option<Value> {
        let mut next = None;
        for (index, value) in values.iter().enumerate() {
            if let Some(patched) = self.patch_value(value) {
                next.get_or_insert_with(|| values.to_vec())[index] = patched;
            }
        }
        next.map(Value::Array)
    }

    fn patch_object(&self, fields: &Map<String, Value>) -> Option<Value> {
        let mut updates = Map::new();
        if row_owner(|key| fields.get(key)?.as_str()) == Some(self.owner) {
            for (key, replacement) in [("attention", &self.attention), ("resume_at", &self.resume)]
            {
                if fields.get(key).is_some_and(|value| value != replacement) {
                    updates.insert(key.into(), replacement.clone());
                }
            }
        }
        for key in ROW_CONTAINERS {
            if let Some(value) = fields.get(*key).and_then(|value| self.patch_value(value)) {
                updates.insert((*key).into(), value);
            }
        }
        (!updates.is_empty()).then(|| {
            let mut next = fields.clone();
            next.extend(updates);
            Value::Object(next)
        })
    }
}

// A run row may also name its checkout; its own attention always wins.
fn row_owner<'a>(mut field: impl FnMut(&str) -> Option<&'a str>) -> Option<&'a str> {
    [
        "run_id",
        "entity_id",
        "worktree_id",
        "workspace_id",
        "plan_id",
    ]
    .into_iter()
    .find_map(&mut field)
}

const ROW_CONTAINERS: &[&str] = &[
    "runs",
    "items",
    "external_worktrees",
    "projects",
    "workspaces",
    "workspace_summaries",
    "run",
    "plan",
    "workspace",
    "branch",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn terminal_attention_patch_shares_other_owners_and_conversation_payloads() {
        let thread = SharedJson::value(json!({
            "run_id":"run", "attention":{"resume_at":"user-authored payload"},
            "items":["retained conversation"],
        }));
        let own = SharedJson::object([
            ("run_id".into(), SharedJson::value(json!("run"))),
            (
                "attention".into(),
                SharedJson::value(json!({
                    "resume_at":"old", "interacted":false, "seen":true,
                })),
            ),
            ("thread".into(), thread.clone()),
        ]);
        let other = SharedJson::value(json!({
            "run_id":"other", "attention":{"resume_at":"old", "interacted":false},
        }));
        let original = SharedJson::array([own, other.clone()]);
        let changed = AttentionPatch::new(
            "run",
            json!({"resume_at":"new", "interacted":true, "seen":true}),
        )
        .apply(&original)
        .unwrap();
        let rows = changed.elements().unwrap();
        assert!(rows[1].ptr_eq(&other));
        assert!(rows[0].field("thread").unwrap().ptr_eq(&thread));
        assert_eq!(
            rows[0].field("attention").unwrap().leaf(),
            Some(&json!({
                "resume_at":"new", "interacted":true, "seen":true,
            }))
        );
    }

    #[test]
    fn terminal_attention_patch_adds_anchor_to_first_external_interaction() {
        let row = SharedJson::value(json!({
            "worktree_id":"checkout",
            "attention":{"resume_at":null, "interacted":false, "seen":false},
        }));
        let attention = json!({
            "resume_at":"new", "interacted":true, "seen":false, "anchor":"created",
        });
        let changed = AttentionPatch::new("checkout", attention.clone())
            .apply(&row)
            .unwrap();
        assert_eq!(changed.leaf().unwrap()["attention"], attention);
    }

    #[test]
    fn terminal_checkout_stamp_does_not_replace_a_runs_own_attention() {
        let row = SharedJson::value(json!({
            "run_id":"run", "worktree_id":"checkout", "resume_at":"run-resume",
        }));
        assert!(
            AttentionPatch::new("checkout", json!({"resume_at":"checkout-resume"}))
                .apply(&row)
                .is_none()
        );
    }
}
