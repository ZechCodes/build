//! Metadata invalidations are independent of conversation fact publication.
use super::*;

impl ChangeBus {
    pub fn note_secret_records(&self, records: &[crate::secrets::SecretRecord]) {
        for record in records {
            self.note_secret_change(payload(record));
        }
    }

    pub(crate) fn note_secret_change(&self, payload: Value) {
        let now = Instant::now();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        for sub in subscriptions.iter_mut() {
            sub.note_secret_change(payload.clone(), now);
        }
        drop(subscriptions);
        self.wake.notify_one();
    }
}

fn payload(record: &crate::secrets::SecretRecord) -> Value {
    use crate::secrets::GrantScope;
    let mut value = json!({"type":SECRETS_CHANGED_EVENT,"revision":record.revision,"secret_ids":[record.context.id]});
    match &record.context.grant {
        GrantScope::Project { project_id } => {
            value["scope"] = json!("project");
            value["project_id"] = json!(project_id);
        }
        GrantScope::Agent {
            conversation_id,
            generation,
            ..
        } => {
            value["scope"] = json!("agent");
            value["entity_id"] = json!(record.context.stage.owner_id);
            value["conversation_id"] = json!(conversation_id);
            value["thread_id"] = json!(generation);
            value["generation"] = json!(generation);
        }
    }
    value
}

pub(super) fn covered(scope: &Scope, payload: &Value) -> bool {
    scope.covers(route(payload))
}

fn route(payload: &Value) -> &str {
    payload["project_id"]
        .as_str()
        .or_else(|| payload["entity_id"].as_str())
        .unwrap_or("")
}

impl Subscription {
    pub(super) fn note_secret_change(&mut self, payload: Value, now: Instant) {
        if !self.spec.kinds.contains(Kind::SecretsChanged)
            || !self.spec.scope.covers(route(&payload))
        {
            return;
        }
        if self.pending.is_empty() {
            self.first_pending_at = Some(now);
        }
        let item = self
            .pending
            .entry(SECRETS_CHANGED_EVENT.into())
            .or_default();
        item.kinds.insert(Kind::SecretsChanged);
        merge(&mut item.secrets, payload);
    }
}

fn identity(payload: &Value) -> Value {
    let mut context = payload.clone();
    if let Some(object) = context.as_object_mut() {
        object.remove("revision");
        object.remove("secret_ids");
        object.remove("truncated");
    }
    context
}

fn merge(events: &mut Vec<Value>, payload: Value) {
    let Some(existing) = events
        .iter_mut()
        .find(|event| identity(event) == identity(&payload))
    else {
        // There cannot be more scope contexts than retained blobs (1,000).
        if events.len() < crate::secrets::MAX_RETAINED_BLOBS {
            events.push(payload);
        }
        return;
    };
    existing["revision"] = json!(existing["revision"]
        .as_i64()
        .unwrap_or(0)
        .max(payload["revision"].as_i64().unwrap_or(0)));
    if existing["truncated"] == true || payload["truncated"] == true {
        existing["secret_ids"] = json!([]);
        existing["truncated"] = json!(true);
        return;
    }
    let ids: BTreeSet<String> = existing["secret_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(payload["secret_ids"].as_array().into_iter().flatten())
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    if ids.len() > 100 {
        existing["secret_ids"] = json!([]);
        existing["truncated"] = json!(true);
    } else {
        existing["secret_ids"] = json!(ids);
    }
}
